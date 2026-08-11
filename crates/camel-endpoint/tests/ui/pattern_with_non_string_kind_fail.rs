// `kind` on a pattern field must be `string` or omitted.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "bad"]
#[allow(dead_code)]
struct BadConfig {
    path: String,
    #[uri_param(pattern = "param.", kind = "duration")]
    bad: Vec<(String, String)>,
}

fn main() {}
