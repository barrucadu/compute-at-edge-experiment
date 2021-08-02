mod secrets;

use config::{Config, ConfigError, FileFormat};
use fastly::http::header;
use fastly::{Body, Error, Request, Response};
use ipnet::{AddrParseError, Ipv4Net};
use iprange::IpRange;
use std::io::BufRead;
use std::io::Write;
use std::net::IpAddr;

const ACCOUNT_COOKIE_NAME: &str = "govuk_account_session";
const BACKEND_NAME: &str = "origin";

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

#[fastly::main]
fn main(mut req: Request) -> Result<Response, Error> {
    let mut settings = Config::new();
    settings
        .merge(config::File::from_str(
            include_str!("../config.yaml"),
            FileFormat::Yaml,
        ))
        .unwrap();

    if let Some(client_ip) = req.get_client_ip_addr() {
        if req.get_method() == "PURGE" && !ip_is_on_purge_allowlist(&settings, &client_ip) {
            req.set_header("Fastly-Purge-Requires-Auth", "1");
        }

        if !ip_is_on_allowlist_or_allowlist_is_empty(&settings, &client_ip) {
            return Ok(Response::from_status(403));
        }

        if ip_is_on_denylist(&settings, &client_ip) {
            return Ok(Response::from_status(403));
        }
    }

    if !authorized(&settings, &req) {
        return Ok(Response::from_status(401).with_header("WWW-Authenticate", "Basic"));
    }

    if get_header(&req, "fastly-ssl").is_none() {
        let url = req.get_url_mut();
        url.set_scheme("https");
        return Ok(Response::from_status(301)
            .with_header("Location", url.to_string())
            .with_header("Fastly-Backend-Name", "force_ssl"));
    }

    if let Some(response) = secrets::recv(&req) {
        return Ok(response);
    }

    if is_special_not_found(&settings, req.get_url().path()).unwrap() {
        return Ok(Response::from_status(404)
            .with_header("Fastly-Backend-Name", "force_not_found")
            .with_body(SYNTHETIC_NOT_FOUND_RESPONSE));
    }

    if let Some(destination) = is_special_redirect(&settings, req.get_url().path()).unwrap() {
        return Ok(Response::from_status(302).with_header("Location", destination));
    }

    let bereq = req.clone_with_body();
    let beresp = fetch_beresp(bereq)?;
    let resp = transform_beresp(&req, beresp);
    Ok(resp)
}

fn fetch_beresp(mut bereq: Request) -> Result<Response, Error> {
    bereq.remove_header(header::ACCEPT_ENCODING);
    Ok(bereq.send(BACKEND_NAME)?)
}

/// Check if an IP is on the PURGE allowlist.
fn ip_is_on_purge_allowlist(settings: &Config, client_ip: &IpAddr) -> bool {
    if let IpAddr::V4(client_ipv4) = client_ip {
        let acl = acl_from_settings(settings, "acl.fastlypurge").unwrap();
        acl.contains(client_ipv4)
    } else {
        false
    }
}

/// Check if an IP is on the general allowlist or if the allowlist is
/// empty.
fn ip_is_on_allowlist_or_allowlist_is_empty(settings: &Config, client_ip: &IpAddr) -> bool {
    let acl = acl_from_settings(settings, "acl.allowlist").unwrap();
    if acl.is_empty() {
        return true;
    }

    if let IpAddr::V4(client_ipv4) = client_ip {
        acl.contains(client_ipv4)
    } else {
        false
    }
}

/// Check if an IP is on the general denylist.
fn ip_is_on_denylist(settings: &Config, client_ip: &IpAddr) -> bool {
    if let IpAddr::V4(client_ipv4) = client_ip {
        let acl = acl_from_settings(settings, "acl.denylist").unwrap();
        acl.contains(client_ipv4)
    } else {
        false
    }
}

/// Read an ACL from the configuration.  Aborts on first error.
fn acl_from_settings(settings: &Config, key: &str) -> Result<IpRange<Ipv4Net>, ConfigError> {
    let array = settings.get_array(key)?;

    let values = array
        .into_iter()
        .map(|s| s.clone().into_str())
        .collect::<Result<Vec<String>, ConfigError>>()?;

    let networks = values
        .iter()
        .map(|s| s.parse())
        .collect::<Result<Vec<Ipv4Net>, AddrParseError>>();

    match networks {
        Ok(networks) => Ok(networks.into_iter().collect()),
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
fn transform_beresp(req: &Request, mut beresp: Response) -> Response {
    let mut resp = beresp.clone_with_body();

    if has_mime_type(&resp, "text/html") {
        let (show_if_cookie, show_if_not_cookie) = if has_session_cookie(&req) {
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

fn has_mime_type(resp: &Response, mimetype: &str) -> bool {
    match resp.get_content_type() {
        Some(mime) if mime.essence_str() == mimetype => true,
        _ => false,
    }
}

fn has_session_cookie(req: &Request) -> bool {
    if let Some(cookies) = get_header(req, "cookie") {
        cookies.contains(ACCOUNT_COOKIE_NAME)
    } else {
        false
    }
}

/// Get the value of a header, if it can be represented as text.
fn get_header<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.get_header(name).and_then(|value| value.to_str().ok())
}
