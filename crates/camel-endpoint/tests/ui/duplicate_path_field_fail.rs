// Two fields without #[uri_param] (both become path fields) is a compile error.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "dup"]
#[allow(dead_code)]
struct DupPath {
    first: String,
    second: String,
}

fn main() {}
