use fastly::{Body, Request, Response};
use std::collections::HashMap;
use std::io::BufRead;
use std::io::Write;

/// Request / response header for the session ID
const ACCOUNT_SESSION_HEADER_NAME: &str = "GOVUK-Account-Session";

/// Response header for ending the session
const ACCOUNT_END_SESSION_HEADER_NAME: &str = "GOVUK-Account-End-Session";

/// Session cookie for the session ID
const ACCOUNT_COOKIE_NAME: &str = "govuk_account_session";

/// Add the account request header if the cookie is set.
pub fn transform_bereq(cookies: &HashMap<String, String>, bereq: &mut Request) {
    if let Some(session_id) = cookies.get(ACCOUNT_COOKIE_NAME) {
        bereq.set_header(ACCOUNT_SESSION_HEADER_NAME, session_id);
    }
}

/// Transform the response: handle the special response headers and
/// transform the body.
pub fn transform_beresp(bereq: &Request, beresp: Response) -> Response {
    transform_css(bereq, transform_header(beresp))
}

/// Handle the special account response headers: updating cookies or
/// caching rules.
fn transform_header(mut beresp: Response) -> Response {
    let mut resp = beresp.clone_with_body();

    if resp.contains_header(ACCOUNT_END_SESSION_HEADER_NAME) {
        resp.append_header(
            "Set-Cookie",
            format!(
                "{}=; secure; httponly; samesite=lax; path=/; max-age=0",
                ACCOUNT_COOKIE_NAME
            ),
        );
    } else if let Some(session_id) = resp.get_header_str("GOVUK-Account-Session") {
        let value = format!(
            "{}={}; secure; httponly; samesite=lax; path=/",
            ACCOUNT_COOKIE_NAME, session_id
        );
        resp.append_header("Set-Cookie", value);
    }

    let varies = beresp.get_header_all_str("Vary");
    let varies_by_account_session = varies
        .iter()
        .any(|value| *value == ACCOUNT_SESSION_HEADER_NAME);
    if varies_by_account_session {
        resp.remove_header("Vary");
        for vary in varies.into_iter() {
            if vary == ACCOUNT_SESSION_HEADER_NAME {
                continue;
            }
            resp.append_header("Vary", vary);
        }
    }

    resp.remove_header(ACCOUNT_SESSION_HEADER_NAME);
    resp.remove_header(ACCOUNT_END_SESSION_HEADER_NAME);

    resp
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
fn transform_css(bereq: &Request, mut beresp: Response) -> Response {
    let mut resp = beresp.clone_with_body();

    if has_mime_type(&resp, "text/html") {
        let (show_if_cookie, show_if_not_cookie) =
            if bereq.contains_header(ACCOUNT_SESSION_HEADER_NAME) {
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
