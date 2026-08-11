// Without #[uri_config(metadata(..))], no inherent `metadata()` fn is
// generated — calling it must fail to compile.
use camel_endpoint_macros::UriConfig;

#[derive(UriConfig)]
#[uri_scheme = "nometa"]
#[allow(dead_code)]
struct NoMetaConfig {
    path: String,
    #[uri_param(default = "1")]
    count: u32,
}

fn main() {
    let _meta = NoMetaConfig::metadata();
}
