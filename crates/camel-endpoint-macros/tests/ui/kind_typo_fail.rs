// An unrecognized `kind = "..."` override is a compile error.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "bad"]
#[allow(dead_code)]
struct BadConfig {
    path: String,
    #[uri_param(kind = "duraton")]
    timeout: std::time::Duration,
}

fn main() {}
