use config::{Config, ConfigError};
use fastly::http::header;
use fastly::http::request::SendError;
use fastly::{Body, Request, Response};
use httpdate::fmt_http_date;
use ipnet::{AddrParseError, Ipv4Net};
use iprange::IpRange;
use std::collections::HashMap;
use std::io::BufRead;
use std::io::Write;
use std::net::IpAddr;
use std::time::SystemTime;
use uuid::Uuid;

const BACKEND_ORIGIN_NAME: &str = "origin";
const BACKEND_FALLBACK1_NAME: &str = "mirrorS3";
const BACKEND_FALLBACK2_NAME: &str = "mirrorS3Replica";
const BACKEND_FALLBACK3_NAME: &str = "mirrorGCS";

/// Session cookie used by the GOV.UK account
const ACCOUNT_COOKIE_NAME: &str = "govuk_account_session";

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
        if !ip_is_on_acl(settings, "allowlist", &client_ip, true).unwrap() {
            return Some(Response::from_status(403));
        }

        if ip_is_on_acl(settings, "denylist", &client_ip, false).unwrap() {
            return Some(Response::from_status(403));
        }
    }

    if !authorized(&settings, &req) {
        return Some(Response::from_status(401).with_header("WWW-Authenticate", "Basic"));
    }

    if get_header(&req, "fastly-ssl").is_none() {
        let mut url = req.get_url().clone();
        url.set_scheme("https");
        return Some(
            Response::from_status(301)
                .with_header("Location", url.to_string())
                .with_header("Fastly-Backend-Name", "force_ssl"),
        );
    }

    if is_special_not_found(&settings, req.get_url().path()).unwrap() {
        return Some(
            Response::from_status(404)
                .with_header("Fastly-Backend-Name", "force_not_found")
                .with_body(SYNTHETIC_NOT_FOUND_RESPONSE),
        );
    }

    if let Some(destination) = is_special_redirect(&settings, req.get_url().path()).unwrap() {
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
        let cookies: HashMap<String, String> = get_cookies(&req);
        let mut bereq = req.clone_with_body();

        bereq.remove_header("Client-IP");
        bereq.set_header("Fastly-Client-IP", ip.clone());
        bereq.set_header("True-Client-IP", ip.clone());
        bereq.set_header("X-Forwarded-For", ip.clone());

        if method == "PURGE" && !ip_is_on_acl(&settings, "fastlypurge", &client_ip, false).unwrap()
        {
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

        if let Ok(expected) = settings.get_str("basic_authorization") {
            bereq.set_header("Authorization", format!("Basic {}", expected));
        }

        if let Some(session_id) = cookies.get(ACCOUNT_COOKIE_NAME) {
            bereq.set_header("GOVUK-Account-Session", session_id);
        }

        // todo https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L354

        if method != "HEAD" && method != "GET" && method != "PURGE" {
            bereq.set_pass(true);
        }

        // todo https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L377

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
pub fn transform_beresp(bereq: &Request, beresp: Response) -> Response {
    transform_account_css(bereq, beresp)
}

/// Transforms the body through simple textual replacement
///
/// There are three special strings, intended to be used as CSS
/// classes, and replaced with the appropriate value:
///
/// - `compute_at_edge--show-if-mirrored` - a CSS class which is
///    visible by default, turned into `compute_at_edge--hide` in all
///    cases.  This is so we can have something which is visible only
///    when we fall back to the static mirrors
///
/// - `compute_at_edge--show-if-cookie` - a CSS class which is hidden
///    by default, turned into `compute_at_edge--show` if the session
///    cookie is present, and `compute_at_edge--hide` otherwise.  This
///    is so we can have something which is visible only when the user
///    has a session cookie.
///
/// - `compute_at_edge--show-if-not-cookie` - a CSS class which is
///    hidden by default, turned into `compute_at_edge--show` if the
///    session cookie is not present, and `compute_at_edge--hide`
///    otherwise.  This is so we can have something which is visible
///    only when the user has a session cookie.
///
/// The classes `compute_at_edge--show` and `compute_at_edge--hide`
/// control visibility of elements in the way you'd expect.
fn transform_account_css(bereq: &Request, mut beresp: Response) -> Response {
    let mut resp = beresp.clone_with_body();

    if has_mime_type(&resp, "text/html") {
        let (show_if_cookie, show_if_not_cookie) =
            if bereq.get_header("GOVUK-Account-Session").is_some() {
                ("compute_at_edge--show", "compute_at_edge--hide")
            } else {
                ("compute_at_edge--hide", "compute_at_edge--show")
            };

        let mut transformed_body = Body::new();
        for line in resp.take_body().lines() {
            write!(
                &mut transformed_body,
                "{}\n",
                line.unwrap()
                    .replace("compute_at_edge--show-if-mirrored", "compute_at_edge--hide")
                    .replace("compute_at_edge--show-if-cookie", show_if_cookie)
                    .replace("compute_at_edge--show-if-not-cookie", show_if_not_cookie),
            )
            .unwrap();
        }

        resp.with_body(transformed_body)
    } else {
        resp
    }
}

/// Check if a response has a given MIME type.
fn has_mime_type(resp: &Response, mimetype: &str) -> bool {
    match resp.get_content_type() {
        Some(mime) if mime.essence_str() == mimetype => true,
        _ => false,
    }
}

/// Check if an IP is on an ACL.
fn ip_is_on_acl(
    settings: &Config,
    acl_name: &str,
    client_ip: &IpAddr,
    on_empty_acl: bool,
) -> Result<bool, ConfigError> {
    let array = settings.get_array(format!("acl.{}", acl_name).as_str())?;

    let values = array
        .into_iter()
        .map(|s| s.clone().into_str())
        .collect::<Result<Vec<String>, ConfigError>>()?;

    let networks = values
        .iter()
        .map(|s| s.parse())
        .collect::<Result<Vec<Ipv4Net>, AddrParseError>>();

    match networks {
        Ok(networks) => {
            let acl: IpRange<Ipv4Net> = networks.into_iter().collect();
            Ok(if acl.is_empty() {
                on_empty_acl
            } else if let IpAddr::V4(client_ipv4) = client_ip {
                acl.contains(client_ipv4)
            } else {
                false
            })
        }
        Err(err) => Err(ConfigError::Message(format!("{:?}", err))),
    }
}
/// Check if the correct Authorization header has been supplied (if
/// needed).
fn authorized(settings: &Config, request: &Request) -> bool {
    if let Ok(expected) = settings.get_str("basic_authorization") {
        if let Some(actual) = get_header(request, "authorization") {
            actual == format!("Basic {}", expected)
        } else {
            false
        }
    } else {
        true
    }
}

/// Check if a path is a special-cased 404
fn is_special_not_found(settings: &Config, path: &str) -> Result<bool, ConfigError> {
    let array = settings.get_array("special_paths.not_found")?;

    let paths = array
        .into_iter()
        .map(|s| s.clone().into_str())
        .collect::<Result<Vec<String>, ConfigError>>()?;

    Ok(paths.contains(&path.to_string()))
}

/// Check if a path is a special-cased redirect and return the redirect if so.
fn is_special_redirect(settings: &Config, path: &str) -> Result<Option<String>, ConfigError> {
    let redirects = settings.get_table("special_paths.redirect")?;

    Ok(redirects
        .get(&path.to_string())
        .and_then(|value| value.clone().into_str().ok()))
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

/// Parse cookies into key/value pairs
fn get_cookies(req: &Request) -> HashMap<String, String> {
    get_header(req, "cookie")
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

/// Get the value of a header, if it can be represented as text.
fn get_header<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.get_header(name).and_then(|value| value.to_str().ok())
}

/// Union of different backend error types.
enum BackendError {
    Config(ConfigError),
    Fastly(SendError),
}

/// Fetch from one of the mirrors.
fn fetch_beresp_fallback(
    settings: &Config,
    bereq: &Request,
    path: &str,
    backend_name: &str,
) -> Result<Response, BackendError> {
    let prefix = settings
        .get_str(format!("mirrors.{}.prefix", backend_name).as_str())
        .map_err(|e| BackendError::Config(e))?;

    // todo https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L330

    bereq
        .clone_without_body()
        .with_header("Fastly-Failover", "1")
        .with_header("Fastly-Backend-Name", backend_name)
        .with_header("Date", fmt_http_date(SystemTime::now()))
        .with_path(format!("{}{}", prefix, path).as_str())
        .send(backend_name)
        .map_err(|e| BackendError::Fastly(e))
}