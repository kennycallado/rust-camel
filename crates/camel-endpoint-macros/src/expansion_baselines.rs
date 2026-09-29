//! Macro expansion baselines for the syn 2 -> 3 migration (openspec
//! change `syn3`, Task 1.2).
//!
//! These tests lock the PRE-pin-flip behavior of `camel-endpoint-macros`:
//! exact `TokenStream::to_string()` bytes for the success path and exact
//! `syn::Error` Display strings for the reject paths are compared
//! byte-for-byte against committed goldens under `src/baselines/`.
//! After the `syn = "3"` pin flip the same tests must pass
//! byte-identical — that is the behavior-neutrality proof for this
//! crate.
//!
//! Regenerate: `UPDATE_GOLDENS=1 cargo test -p camel-endpoint-macros --lib`.

use crate::uri_config::impl_uri_config;
use syn::{DeriveInput, parse_quote};

/// Regeneration switch: when `UPDATE_GOLDENS=1` is set, write the
/// baseline files instead of comparing them.
fn update_goldens() -> bool {
    std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v == "1")
}

fn baseline_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/baselines")
        .join(name)
}

/// Byte-for-byte baseline lock for one expansion artifact (generated
/// token stream or error diagnostic).
fn check(name: &str, produced: String) {
    let path = baseline_path(name);
    if update_goldens() {
        let parent = path
            .parent()
            .unwrap_or_else(|| panic!("baseline {name} has no parent dir"));
        std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create baselines dir: {e}"));
        std::fs::write(&path, &produced).unwrap_or_else(|e| panic!("write baseline {name}: {e}"));
    } else {
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read baseline {name}: {e} (capture: UPDATE_GOLDENS=1)"));
        assert_eq!(expected, produced, "baseline {name} drifted");
    }
}

/// Representative ok case: named struct with a `#[uri_scheme]` attribute,
/// a path field (`name: String`) and three `#[uri_param]` fields
/// (`Option`/`Vec`/`bool` with bare, `name =`, and `default =` shapes)
/// fed to `impl_uri_config`; the full expansion bytes are locked.
#[test]
fn uri_config_expansion_baseline_ok() {
    let input: DeriveInput = parse_quote! {
        #[uri_scheme = "timer"]
        struct TimerConfig {
            name: String,
            #[uri_param(name = "optName")]
            optional: Option<String>,
            #[uri_param]
            multi: Vec<String>,
            #[uri_param(default = "true")]
            flag: bool,
        }
    };
    match impl_uri_config(&input) {
        Ok(tokens) => check("uri_config_ok.txt", tokens.to_string()),
        Err(e) => panic!("impl_uri_config should succeed for valid input, got: {e}"),
    }
}

/// Structs without a `#[uri_scheme]` attribute are rejected.
#[test]
fn uri_config_baseline_missing_scheme_reject() {
    let input: DeriveInput = parse_quote! {
        struct TimerConfig {
            name: String,
            optional: Option<String>,
            multi: Vec<String>,
            flag: bool,
        }
    };
    match impl_uri_config(&input) {
        Ok(_) => panic!("impl_uri_config should reject structs without #[uri_scheme]"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("missing #[uri_scheme"),
                "unexpected diagnostic: {msg}"
            );
            check("uri_config_missing_scheme_reject.txt", msg);
        }
    }
}

/// Unrecognized `#[uri_param]` keys are rejected.
#[test]
fn uri_config_baseline_bad_param_reject() {
    let input: DeriveInput = parse_quote! {
        #[uri_scheme = "timer"]
        struct TimerConfig {
            #[uri_param(unknown = "x")]
            name: String,
            optional: Option<String>,
            multi: Vec<String>,
            flag: bool,
        }
    };
    match impl_uri_config(&input) {
        Ok(_) => panic!("impl_uri_config should reject unrecognized uri_param keys"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("unknown attribute key"),
                "unexpected diagnostic: {msg}"
            );
            check("uri_config_bad_param_reject.txt", msg);
        }
    }
}
