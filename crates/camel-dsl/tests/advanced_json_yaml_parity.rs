//! Integration test: the advanced EIP showcase must lower to the same
//! `DeclarativeRoute` set whether the source is YAML or JSON.
//!
//! This exercises Slice B3's DoD: the JSON mirror at
//! `examples/json-dsl/config/routes-eip-advanced.json` and the YAML original at
//! `examples/yaml-dsl/config/routes-eip-advanced.yaml` produce structurally
//! identical declarative models. Compared via Debug-string of
//! `Vec<DeclarativeRoute>` (same trick as `parity_tests.rs` from B1 —
//! avoids any derive work and matches the established pattern).

use std::path::{Path, PathBuf};

use camel_dsl::{parse_json_to_declarative, parse_yaml_to_declarative};

/// Locate the workspace `examples/` dir relative to this crate's manifest.
///
/// Returns an owned `PathBuf` (NOT `&'static Path`) because the path is
/// computed at runtime by joining `env!("CARGO_MANIFEST_DIR")` with a
/// relative suffix — `join()` produces a temporary `PathBuf` whose
/// borrow cannot escape the function.
fn examples_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points at crates/camel-dsl/ at compile time.
    // examples/ lives at the workspace root, two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn load(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

#[test]
fn advanced_example_yaml_json_parity() {
    let yaml_path = examples_dir().join("yaml-dsl/config/routes-eip-advanced.yaml");
    let json_path = examples_dir().join("json-dsl/config/routes-eip-advanced.json");

    let yaml_src = load(&yaml_path);
    let json_src = load(&json_path);

    let yaml_routes =
        parse_yaml_to_declarative(&yaml_src).expect("YAML advanced example must parse");
    let json_routes =
        parse_json_to_declarative(&json_src).expect("JSON advanced example must parse");

    assert_eq!(
        yaml_routes.len(),
        json_routes.len(),
        "route count mismatch: YAML={} JSON={}",
        yaml_routes.len(),
        json_routes.len()
    );

    let yaml_debug = format!("{:#?}", yaml_routes);
    let json_debug = format!("{:#?}", json_routes);
    assert_eq!(
        yaml_debug,
        json_debug,
        "advanced EIP example lowers to different DeclarativeRoute sets\n\
         --- YAML debug (first 2k) ---\n{}\n\
         --- JSON debug (first 2k) ---\n{}",
        &yaml_debug.chars().take(2048).collect::<String>(),
        &json_debug.chars().take(2048).collect::<String>(),
    );
}
