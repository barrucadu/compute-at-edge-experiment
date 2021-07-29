use fastly::http::header;
use fastly::{Body, Error, Request, Response};
use std::io::BufRead;
use std::io::Write;

const ACCOUNT_COOKIE_NAME: &str = "govuk_account_session";
const BACKEND_NAME: &str = "origin";

#[fastly::main]
fn main(mut req: Request) -> Result<Response, Error> {
    if req.get_method() == "PURGE" {
        return Ok(req.send(BACKEND_NAME)?);
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
