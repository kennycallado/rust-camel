// A truly-unknown #[uri_param] key is a compile error.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "bad"]
#[allow(dead_code)]
struct BadConfig {
    path: String,
    #[uri_param(bogus = 1)]
    n: u32,
}

fn main() {}
