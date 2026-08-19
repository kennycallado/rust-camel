// `pattern` separator must yield a non-empty name: a bare `.` strips to empty.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "bad"]
#[allow(dead_code)]
struct BadConfig {
    path: String,
    #[uri_param(pattern = ".")]
    bad: Vec<(String, String)>,
}

fn main() {}
