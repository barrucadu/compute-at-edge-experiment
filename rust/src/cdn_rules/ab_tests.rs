use crate::cdn_config::Config;

use fastly::{Request, Response};
use rand::Rng;
use std::collections::HashMap;

/// User-Agent header of the crawler worker.
const CRAWLER_WORKER_USER_AGENT: &str = "GOV.UK Crawler Worker";

/// Name of the example A/B test
const EXAMPLE_AB_TEST_NAME: &str = "Example";

/// Path to always set the example A/B test variant on, even without a
/// consent cookie
const EXAMPLE_AB_TEST_PATH: &str = "/help/ab-testing";

/// Assign the user to A/B test variants.
///
/// If the user has a cookie, or a ?ABTest-<Name>=<Variant> query
/// param, they are put in that variant; otherwise one is chosen at
/// random.
pub fn transform_bereq(settings: &Config, cookies: &HashMap<String, String>, bereq: &mut Request) {
    if has_consented_to_ab_tests(cookies) {
        for (name, ab_test) in settings.ab_tests.iter() {
            if !ab_test.active {
                continue;
            }

            let header_name: String = format!("GOVUK-ABTest-{}", name);
            let param_name: String = format!("ABTest-{}", name);

            if bereq.get_header_str("user-agent") == Some(CRAWLER_WORKER_USER_AGENT) {
                bereq.set_header(header_name, ab_test.crawler_variant.clone());
                continue;
            }

            let qs: Vec<(String, String)> = bereq.get_query().unwrap();
            let qs_map: HashMap<String, String> = qs.into_iter().collect();
            if let Some(variant) = qs_map.get(&param_name) {
                if ab_test.variants.get(variant).is_some() {
                    bereq.set_header(header_name, variant);
                    continue;
                }
            }

            if let Some(variant) = cookies.get(&param_name) {
                if ab_test.variants.get(variant).is_some() {
                    bereq.set_header(header_name, variant);
                    continue;
                }
            }

            let total_freq = ab_test.variants.values().sum();
            let mut index = rand::thread_rng().gen_range(0..total_freq);
            for (variant, freq) in ab_test.variants.iter() {
                if index <= *freq {
                    bereq.set_header(header_name, variant);
                    break;
                } else {
                    index = index - freq;
                }
            }
        }
    }
}

/// Set the response headers / cookies to keep the user in the same
/// variant when they return.
pub fn transform_beresp(
    settings: &Config,
    bereq: &Request,
    mut beresp: Response,
    bereq_cookies: &HashMap<String, String>,
) -> Response {
    let mut resp = beresp.clone_with_body();

    for (name, ab_test) in settings.ab_tests.iter() {
        if !ab_test.active {
            continue;
        }

        if bereq.get_header_str("User-Agent") == Some(CRAWLER_WORKER_USER_AGENT) {
            continue;
        }

        let header_name: String = format!("GOVUK-ABTest-{}", name);
        let requested_variant: Option<&str> = bereq.get_header_str(header_name);
        let param_name: String = format!("ABTest-{}", name);

        if has_consented_to_ab_tests(&bereq_cookies)
            || (name == EXAMPLE_AB_TEST_NAME && bereq.get_path() == EXAMPLE_AB_TEST_PATH)
        {
            if let Some(variant) = requested_variant {
                resp.append_header(
                    "Set-Cookie",
                    format!(
                        "{}={}; secure; max-age={}; path=/",
                        param_name, variant, ab_test.expires
                    ),
                );
            }
        }
    }

    resp
}

/// Check if the user has consented to A/B tests
fn has_consented_to_ab_tests(cookies: &HashMap<String, String>) -> bool {
    if let Some(policy) = cookies.get("cookies_policy") {
        if policy.contains("%22usage%22:true") {
            return true;
        }
    }

    false
}
