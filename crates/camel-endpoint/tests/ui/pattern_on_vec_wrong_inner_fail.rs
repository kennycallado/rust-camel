// `pattern` is only valid on fields of type `Vec<(String, String)>`.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "bad"]
#[allow(dead_code)]
struct BadConfig {
    path: String,
    #[uri_param(pattern = "param.")]
    bad: Vec<String>,
}

fn main() {}
