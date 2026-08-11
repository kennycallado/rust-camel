// A field cannot be both secret and carry a default.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "bad"]
#[allow(dead_code)]
struct BadConfig {
    path: String,
    #[uri_param(secret, default = "x")]
    key: String,
}

fn main() {}
