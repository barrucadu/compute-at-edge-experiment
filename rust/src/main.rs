mod cdn_config;
mod cdn_rules;
mod cdn_secrets;

use fastly::{Error, Request, Response};

#[fastly::main]
fn main(mut req: Request) -> Result<Response, Error> {
    if let Ok(settings) = cdn_config::parse_config(include_str!("../config.yaml")) {
        if let Some(response) = cdn_rules::synthetic_response(&settings, &req) {
            return Ok(response);
        }
        if let Some(response) = cdn_secrets::recv(&settings, &req) {
            return Ok(response);
        }

        match cdn_rules::build_bereq(&settings, &mut req) {
            Some(bereq) => {
                let original_bereq = bereq.clone_without_body();
                match cdn_rules::fetch_beresp(&settings, bereq) {
                    Some(beresp) => Ok(cdn_rules::transform_beresp(
                        &settings,
                        &original_bereq,
                        beresp,
                    )),
                    None => Ok(cdn_rules::synthetic_error_response()),
                }
            }
            None => Ok(cdn_rules::synthetic_error_response()),
        }
    } else {
        Ok(cdn_rules::synthetic_error_response())
    }
}
