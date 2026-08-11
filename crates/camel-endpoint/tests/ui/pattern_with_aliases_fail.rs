// `pattern` and `aliases` are mutually exclusive.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "bad"]
#[allow(dead_code)]
struct BadConfig {
    path: String,
    #[uri_param(pattern = "param.", aliases = ["x"])]
    bad: Vec<(String, String)>,
}

fn main() {}
