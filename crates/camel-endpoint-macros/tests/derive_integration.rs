//! Integration tests for `#[derive(UriConfig)]` codegen of `uri_options()` and
//! `metadata()`. These exercise the macro end-to-end (derive a struct, call the
//! generated inherent fn) and complement the pure parse/infer unit tests in
//! `src/uri_config.rs`.
//!
//! NOTE: `infer_duration` and `infer_vec_string` for bare `Duration`/`Vec<T>`
//! fields are covered by the unit tests in `src/uri_config.rs`, because the
//! macro's existing parse codegen requires `FromStr` (bare `Duration` and
//! `Vec<T>` do not implement it). Inference of those kinds is exercised there.

use camel_api::component_metadata::{ComponentMetadata, OptionKind, UriOption};
use camel_endpoint_macros::UriConfig;

// ---------------------------------------------------------------------------
// OptionKind inference (task 1.2)
// ---------------------------------------------------------------------------

#[derive(Default, Clone, PartialEq, Eq, Debug)]
#[allow(dead_code)]
enum InferMode {
    #[default]
    A,
    B,
}

impl std::str::FromStr for InferMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            _ => Err(format!("unknown mode: {s}")),
        }
    }
}

#[derive(UriConfig)]
#[uri_scheme = "infer"]
#[allow(dead_code)]
struct InferConfig {
    path: String,
    #[uri_param]
    active: bool,
    #[uri_param]
    name: String,
    #[uri_param]
    val: Option<u32>,
    #[uri_param]
    opt_str: Option<String>,
    #[uri_param]
    mode: InferMode,
}

fn find_opt<'a>(opts: &'a [UriOption], name: &str) -> &'a UriOption {
    opts.iter()
        .find(|o| o.name == name)
        .unwrap_or_else(|| panic!("option '{name}' not found"))
}

#[test]
fn infer_bool() {
    let opts = InferConfig::uri_options();
    let o = find_opt(&opts, "active");
    assert_eq!(o.kind, OptionKind::Bool);
}

#[test]
fn infer_string() {
    let opts = InferConfig::uri_options();
    let o = find_opt(&opts, "name");
    assert_eq!(o.kind, OptionKind::String);
}

#[test]
fn infer_option_inner_kind() {
    // Option<u32> unwraps to Int.
    let opts = InferConfig::uri_options();
    let o = find_opt(&opts, "val");
    assert_eq!(o.kind, OptionKind::Int);
}

#[test]
fn infer_option_required_false() {
    // Option<String> with no `required` attr => required == false.
    let opts = InferConfig::uri_options();
    let o = find_opt(&opts, "opt_str");
    assert!(!o.required);
    assert_eq!(o.kind, OptionKind::String);
}

#[test]
fn infer_enum_is_string() {
    // An enum-typed field infers to String — NEVER Enum.
    let opts = InferConfig::uri_options();
    let o = find_opt(&opts, "mode");
    assert_eq!(o.kind, OptionKind::String);
}

#[test]
fn infer_required_non_option_no_default() {
    // `active: bool` is non-Option with no default => required.
    let opts = InferConfig::uri_options();
    let o = find_opt(&opts, "active");
    assert!(o.required);
}

#[test]
fn kind_override_enum() {
    #[derive(UriConfig)]
    #[uri_scheme = "kindov"]
    #[allow(dead_code)]
    struct C {
        path: String,
        #[uri_param(kind = "enum:A,B")]
        mode: String,
    }
    let opts = C::uri_options();
    let o = find_opt(&opts, "mode");
    assert_eq!(
        o.kind,
        OptionKind::Enum(vec!["A".to_string(), "B".to_string()])
    );
}

#[test]
fn kind_override_duration_overrides_string_type() {
    // kind override wins over inference even when the type would infer to
    // something else.
    #[derive(UriConfig)]
    #[uri_scheme = "kd"]
    #[allow(dead_code)]
    struct C {
        path: String,
        #[uri_param(default = "1000", kind = "int")]
        n: String,
    }
    let opts = C::uri_options();
    let o = find_opt(&opts, "n");
    assert_eq!(o.kind, OptionKind::Int);
}

// ---------------------------------------------------------------------------
// uri_options() generation (task 1.3)
// ---------------------------------------------------------------------------

#[derive(UriConfig)]
#[uri_scheme = "opts"]
#[allow(dead_code)]
struct OptsConfig {
    // First field without #[uri_param] is the path field — excluded from
    // uri_options().
    path: String,
    #[uri_param(default = "100")]
    count: u32,
    #[uri_param(secret)]
    api_key: String,
    #[uri_param(desc = "the period", default = "1000")]
    period: u64,
    #[uri_param(deprecated = "use newParam", aliases = ["old", "legacy"])]
    new_param: Option<String>,
}

#[test]
fn uri_options_excludes_path_field() {
    let opts = OptsConfig::uri_options();
    // 4 #[uri_param] fields, path field excluded.
    assert_eq!(opts.len(), 4);
    assert!(opts.iter().all(|o| o.name != "path"));
}

#[test]
fn uri_options_has_secret_flag() {
    let opts = OptsConfig::uri_options();
    let o = find_opt(&opts, "api_key");
    assert!(o.secret);
}

#[test]
fn uri_options_has_default() {
    let opts = OptsConfig::uri_options();
    let o = find_opt(&opts, "count");
    assert_eq!(o.default_value.as_deref(), Some("100"));
    assert!(!o.required);
}

#[test]
fn uri_options_carries_desc() {
    let opts = OptsConfig::uri_options();
    let o = find_opt(&opts, "period");
    assert_eq!(o.description, "the period");
}

#[test]
fn uri_options_deprecated_and_aliases() {
    let opts = OptsConfig::uri_options();
    let o = find_opt(&opts, "new_param");
    assert_eq!(o.deprecated.as_deref(), Some("use newParam"));
    assert_eq!(o.aliases, vec!["old".to_string(), "legacy".to_string()]);
    // Option<String> => not required.
    assert!(!o.required);
}

#[test]
fn uri_options_empty_for_path_only_struct() {
    #[derive(UriConfig)]
    #[uri_scheme = "pathonly"]
    #[allow(dead_code)]
    struct P {
        path: String,
    }
    assert!(P::uri_options().is_empty());
}

// ---------------------------------------------------------------------------
// metadata() generation (task 1.4)
// ---------------------------------------------------------------------------

#[derive(UriConfig)]
#[uri_scheme = "md1"]
#[uri_config(metadata(scheme = "md1", description = "a test component", producer, consumer))]
#[allow(dead_code)]
struct MetaConfig {
    path: String,
    #[uri_param(default = "100")]
    count: u32,
    #[uri_param]
    label: String,
}

#[test]
fn metadata_optin_generates_uri_options() {
    let meta: ComponentMetadata = MetaConfig::metadata();
    assert_eq!(meta.scheme, "md1");
    assert_eq!(meta.description, "a test component");
    // 2 #[uri_param] fields.
    assert_eq!(meta.uri_options.len(), 2);
}

#[test]
fn metadata_optin_capabilities() {
    let meta = MetaConfig::metadata();
    assert!(meta.capabilities.supports_producer);
    assert!(meta.capabilities.supports_consumer);
    assert!(!meta.capabilities.supports_polling_consumer);
    assert!(!meta.capabilities.supports_streaming);
}

#[test]
fn metadata_all_capability_flags() {
    #[derive(UriConfig)]
    #[uri_scheme = "md2"]
    #[uri_config(metadata(scheme = "md2", producer, consumer, polling_consumer, streaming))]
    #[allow(dead_code)]
    struct C {
        path: String,
    }
    let meta = C::metadata();
    assert!(meta.capabilities.supports_producer);
    assert!(meta.capabilities.supports_consumer);
    assert!(meta.capabilities.supports_polling_consumer);
    assert!(meta.capabilities.supports_streaming);
}

#[test]
fn metadata_falls_back_to_uri_scheme() {
    // metadata(..) without explicit scheme falls back to #[uri_scheme].
    #[derive(UriConfig)]
    #[uri_scheme = "fallback"]
    #[uri_config(metadata(description = "d"))]
    #[allow(dead_code)]
    struct C {
        path: String,
    }
    let meta = C::metadata();
    assert_eq!(meta.scheme, "fallback");
}

#[test]
fn metadata_skip_impl_combinable() {
    #[derive(UriConfig)]
    #[uri_scheme = "skipmd"]
    #[uri_config(skip_impl, metadata(scheme = "skipmd", description = "skip", consumer))]
    #[allow(dead_code)]
    struct C {
        path: String,
        #[uri_param(default = "1")]
        n: u32,
    }
    // skip_impl still generates uri_options() + metadata().
    let meta = C::metadata();
    assert_eq!(meta.uri_options.len(), 1);
    assert!(meta.capabilities.supports_consumer);
}
