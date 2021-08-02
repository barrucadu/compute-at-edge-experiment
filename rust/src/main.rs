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
    match req.get_header("cookie") {
        Some(cookies) => match cookies.to_str() {
            Ok(cookie_values) => cookie_values.contains(ACCOUNT_COOKIE_NAME),
            _ => false,
        },
        None => false,
    }
}
