// Deriving UriConfig on an enum is a compile error.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "bad"]
enum NonStruct {
    Variant,
}

fn main() {}
