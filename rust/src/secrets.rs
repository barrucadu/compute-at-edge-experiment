// This is a possible way to implement secret VCL
// See https://github.com/alphagov/govuk-cdn-config/blob/master/vcl_templates/www.vcl.erb#L227

use fastly::{Request, Response};

pub fn recv(req: &Request) -> Option<Response> {
    None
}
