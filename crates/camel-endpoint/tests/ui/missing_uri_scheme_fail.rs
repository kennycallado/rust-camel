// Missing #[uri_scheme] attribute is a compile error.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[allow(dead_code)]
struct MissingScheme {
    path: String,
}

fn main() {}
