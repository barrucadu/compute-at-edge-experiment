mod ab_tests;
mod accounts;

use crate::cdn_config::Config;

use fastly::http::header;
use fastly::http::request::SendError;
use fastly::{Request, Response};
use httpdate::fmt_http_date;
use ipnet::Ipv4Net;
use iprange::IpRange;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::SystemTime;
use uuid::Uuid;

const BACKEND_ORIGIN_NAME: &str = "origin";
const BACKEND_FALLBACK1_NAME: &str = "mirrorS3";
const BACKEND_FALLBACK2_NAME: &str = "mirrorS3Replica";
const BACKEND_FALLBACK3_NAME: &str = "mirrorGCS";

/// HTML for a synthetic 404 response
const SYNTHETIC_NOT_FOUND_RESPONSE: &str = r#"<!DOCTYPE html>
<html>
  <head>
    <title>Welcome to GOV.UK</title>
    <style>
      body { font-family: Arial, sans-serif; margin: 0; }
      header { background: black; }
      h1 { color: white; font-size: 29px; margin: 0 auto; padding: 10px; max-width: 990px; }
      p { color: black; margin: 30px auto; max-width: 990px; }
    </style>
  </head>
  <body>
    <header><h1>GOV.UK</h1></header>
    <p>We cannot find the page you're looking for. Please try searching on <a href="https://www.gov.uk/">GOV.UK</a>.</p>
  </body>
</html>
"#;

/// HTML for a synthetic 503 response
const SYNTHETIC_SERVER_ERROR_RESPONSE: &str = r#"
<!DOCTYPE html>
<html>
  <head>
    <title>Welcome to GOV.UK</title>
    <style>
      body { font-family: Arial, sans-serif; margin: 0; }
      header { background: black; }
      h1 { color: white; font-size: 29px; margin: 0 auto; padding: 10px; max-width: 990px; }
      p { color: black; margin: 30px auto; max-width: 990px; }
    </style>
  </head>
  <body>
    <header><h1>GOV.UK</h1></header>
    <p>We're experiencing technical difficulties. Please try again later.</p>
    <p>You can <a href="/coronavirus">find coronavirus information</a> on GOV.UK.</p>
  </body>
</html>
"#;

/// When falling back to the mirrors, if a path doesn't have one of
/// these suffixes, add ".html"
const SUFFIXES: &[&str] = &[
    "atom", "chm", "css", "csv", "diff", "doc", "docx", "dot", "dxf", "eps", "gif", "gml", "html",
    "ico", "ics", "jpeg", "jpg", "JPG", "js", "json", "kml", "odp", "ods", "odt", "pdf", "PDF",
    "png", "ppt", "pptx", "ps", "rdf", "rtf", "sch", "txt", "wsdl", "xls", "xlsm", "xlsx", "xlt",
    "xml", "xsd", "xslt", "zip",
];

/// Produce a synthetic response to this request, if appropriate.
pub fn synthetic_response(settings: &Config, req: &Request) -> Option<Response> {
    if let Some(client_ip) = req.get_client_ip_addr().clone() {
        if !ip_is_on_acl(&settings.acl_allowlist, &client_ip, true) {
            return Some(Response::from_status(403));
        }

        if ip_is_on_acl(&settings.acl_denylist, &client_ip, false) {
            return Some(Response::from_status(403));
        }
    }

    if !authorized(&settings, &req) {
        return Some(Response::from_status(401).with_header("WWW-Authenticate", "Basic"));
    }

    if !req.contains_header("fastly-ssl") {
        let mut url = req.get_url().clone();
        url.set_scheme("https");
        return Some(
            Response::from_status(301)
                .with_header("Location", url.to_string())
                .with_header("Fastly-Backend-Name", "force_ssl"),
        );
    }

    if is_special_not_found(&settings, req.get_url().path()) {
        return Some(
            Response::from_status(404)
                .with_header("Fastly-Backend-Name", "force_not_found")
                .with_body(SYNTHETIC_NOT_FOUND_RESPONSE),
        );
    }

    if let Some(destination) = is_special_redirect(&settings, req.get_url().path()) {
        return Some(Response::from_status(302).with_header("Location", destination));
    }

    None
}

/// Build the backend request.
///
/// Returns `None` if the `Request` parameter is not a client request.
pub fn build_bereq(settings: &Config, req: &mut Request) -> Option<Request> {
    if let Some(client_ip) = req.get_client_ip_addr() {
        let ip = client_ip.to_string();
        let method: String = req.get_method_str().to_string();
        let cookies: HashMap<String, String> = get_cookies(req.get_header_str("cookie"));
        let mut bereq = req.clone_with_body();

        bereq.remove_header("Client-IP");
        bereq.set_header("Fastly-Client-IP", ip.clone());
        bereq.set_header("True-Client-IP", ip.clone());
        bereq.set_header("X-Forwarded-For", ip.clone());

        if method == "PURGE" && !ip_is_on_acl(&settings.acl_fastlypurge, &client_ip, false) {
            bereq.set_header("Fastly-Purge-Requires-Auth", "1");
        }

        bereq.set_query(&normalise_querystring(&req));

        // https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L246
        // not sure how to do this - is this `req.set_stale_while_revalidate()` ?

        bereq.set_header("Govuk-Use-Recommended-Related-Links", "true");

        bereq.set_header(
            "GOVUK-Request-Id",
            Uuid::new_v4()
                .to_hyphenated()
                .encode_lower(&mut Uuid::encode_buffer())
                .to_string(),
        );

        if let Some(expected) = &settings.basic_authorization {
            bereq.set_header("Authorization", format!("Basic {}", expected));
        }

        // todo https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L354

        if method != "HEAD" && method != "GET" && method != "PURGE" {
            bereq.set_pass(true);
        }

        ab_tests::transform_bereq(&settings, &cookies, &mut bereq);
        accounts::transform_bereq(&cookies, &mut bereq);

        Some(bereq)
    } else {
        None
    }
}

/// Fetch the backend response, falling back to the mirrors if the
/// origin is unavailable.
///
/// Returns `None` if the origin and all the mirrors fail.
pub fn fetch_beresp(settings: &Config, mut bereq: Request) -> Option<Response> {
    // fetch an uncompressed response, so that `transform_beresp` can handle it.
    bereq.remove_header(header::ACCEPT_ENCODING);

    let original_bereq = bereq.clone_without_body();

    let mut fallback_path = bereq
        .get_path()
        .split("/")
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if fallback_path == "" || fallback_path == "/" {
        fallback_path = "/index.html".to_string();
    }

    match bereq.send(BACKEND_ORIGIN_NAME) {
        Ok(beresp) if !beresp.get_status().is_server_error() => Some(beresp),
        _ => {
            if !SUFFIXES.iter().any(|suff| fallback_path.ends_with(suff)) {
                fallback_path = format!("{}.html", fallback_path);
            }

            // todo https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L604
            match fetch_beresp_fallback(
                settings,
                &original_bereq,
                &fallback_path,
                BACKEND_FALLBACK1_NAME,
            ) {
                Ok(beresp_fallback) if !beresp_fallback.get_status().is_server_error() => {
                    Some(beresp_fallback)
                }
                _ => match fetch_beresp_fallback(
                    settings,
                    &original_bereq,
                    &fallback_path,
                    BACKEND_FALLBACK2_NAME,
                ) {
                    Ok(beresp_fallback) if !beresp_fallback.get_status().is_server_error() => {
                        Some(beresp_fallback)
                    }
                    _ => match fetch_beresp_fallback(
                        settings,
                        &original_bereq,
                        &fallback_path,
                        BACKEND_FALLBACK3_NAME,
                    ) {
                        Ok(beresp_fallback) if !beresp_fallback.get_status().is_server_error() => {
                            Some(beresp_fallback)
                        }
                        _ => None,
                    },
                },
            }
        }
    }
}

/// Generate a synthetic 503 response.  Used if all else fails.
pub fn synthetic_error_response() -> Response {
    Response::from_status(503)
        .with_header("Fastly-Backend-Name", "error")
        .with_body(SYNTHETIC_SERVER_ERROR_RESPONSE)
}

/// Transform the response body.
pub fn transform_beresp(settings: &Config, bereq: &Request, beresp: Response) -> Response {
    let bereq_cookies = get_cookies(bereq.get_header_str("cookie"));
    accounts::transform_beresp(
        bereq,
        ab_tests::transform_beresp(settings, bereq, beresp, &bereq_cookies),
    )
}

/// Check if an IP is on an ACL.
fn ip_is_on_acl(acl: &IpRange<Ipv4Net>, client_ip: &IpAddr, on_empty_acl: bool) -> bool {
    if acl.is_empty() {
        on_empty_acl
    } else if let IpAddr::V4(client_ipv4) = client_ip {
        acl.contains(client_ipv4)
    } else {
        false
    }
}

/// Check if the correct Authorization header has been supplied (if
/// needed).
fn authorized(settings: &Config, request: &Request) -> bool {
    match (
        &settings.basic_authorization,
        request.get_header_str("authorization"),
    ) {
        (Some(expected), Some(actual)) if actual == format!("Basic {}", expected) => true,
        (Some(_), _) => false,
        (None, _) => true,
    }
}

/// Check if a path is a special-cased 404
fn is_special_not_found(settings: &Config, path: &str) -> bool {
    settings.synthetic_not_found.contains(&path.to_string())
}

/// Check if a path is a special-cased redirect and return the redirect if so.
fn is_special_redirect<'a>(settings: &'a Config, path: &'a str) -> Option<&'a String> {
    settings.synthetic_redirect.get(&path.to_string())
}

/// Sort the querystring, remove UTM params, and drop some params on
/// certain pages.
fn normalise_querystring(req: &Request) -> Vec<(String, String)> {
    let mut qs: Vec<(String, String)> = req.get_query().unwrap();

    match req.get_url().path() {
        // https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L266
        "/" => qs = vec![],
        // https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L261
        "/find-coronavirus-local-restrictions" => qs.retain(|param| param.0 == "postcode"),
        // https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L243
        _ => qs.retain(|param| !param.0.starts_with("utm_")),
    }

    qs.sort_by(|(a, _), (b, _)| a.cmp(b));
    qs
}

/// Parse cookies header into key/value pairs
fn get_cookies(header_str: Option<&str>) -> HashMap<String, String> {
    header_str
        .unwrap_or("")
        .split(";")
        .filter_map(|kv| {
            kv.find("=").map(|index| {
                let (key, value) = kv.split_at(index);
                let key = key.trim().to_string();
                let value = value[1..].to_string();
                (key, value)
            })
        })
        .collect()
}

/// Union of different backend error types.
enum BackendError {
    MissingConfig,
    Fastly(SendError),
}

/// Fetch from one of the mirrors.
fn fetch_beresp_fallback(
    settings: &Config,
    bereq: &Request,
    path: &str,
    backend_name: &str,
) -> Result<Response, BackendError> {
    if let Some(mirror_config) = settings.mirrors.get(backend_name) {
        // todo https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L330

        let new_path = if let Some(prefix) = &mirror_config.prefix {
            format!("{}{}", prefix.clone(), path)
        } else {
            path.to_string()
        };

        bereq
            .clone_without_body()
            .with_header("Fastly-Failover", "1")
            .with_header("Fastly-Backend-Name", backend_name)
            .with_header("Date", fmt_http_date(SystemTime::now()))
            .with_path(&new_path)
            .send(backend_name)
            .map_err(|e| BackendError::Fastly(e))
    } else {
        Err(BackendError::MissingConfig)
    }
}
