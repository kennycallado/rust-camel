//! Cross-loader configuration parity (openspec change `multidoc`,
//! Task 3.1 phase-review follow-up).
//!
//! The compiler embeds `Camel.toml`, includes, and selected profile
//! fragments as store entries, and `camel_dsl::discover_virtual_store`
//! re-assembles them into one merged TOML tree — a deliberate mirror of
//! camel-config's `load_includes` + `build_from_toml_value_inner`
//! ordering (see the SYNC notes in `camel-dsl/src/discovery.rs`). These
//! tests pin that mirror to the real filesystem loader: one fixture
//! tree is loaded once through `CamelConfig::from_file_sealed` and once
//! through the virtual store (`discover_virtual_store` +
//! `CamelConfig::from_toml_value_with_env`), and the resulting typed
//! values must agree.
//!
//! The comparison is field-by-field on a representative fixture:
//! `CamelConfig` has no `PartialEq` (its closure includes camel-core's
//! `TracerConfig`), so full-struct equality is not cheaply available.

use super::*;
use camel_dsl::{StoreDocument, StoreEntryKind, VirtualDocumentStore, discover_virtual_store};

/// Deployment lookup both paths receive (identical, so any difference
/// would come from the merge, not resolution).
fn lookup(name: &str) -> Option<String> {
    match name {
        "VS_PARITY_LOG" => Some("debug".to_string()),
        _ => None,
    }
}

/// The shared fixture. `conf/base.toml` carries an include-only key and
/// a component leaf; `[prod]` overlays `[default]` in the configuration
/// document, the `routes` array must REPLACE (never concatenate), and
/// `[prod].log_level` is a placeholder both paths resolve through the
/// same injected deployment lookup.
const INCLUDE_TEXT: &str = "log_level = \"trace\"\n\n[components.http]\nmax_connections = 11\n";

const CONFIG_TEXT: &str = r#"
include = ["conf/base.toml"]

[default]
timeout_ms = 30000
routes = ["routes/main.yaml"]

[prod]
timeout_ms = 10000
routes = ["routes/main.yaml", "routes/orders.yaml"]
log_level = "${env:VS_PARITY_LOG}"
watch = true
"#;

/// The `[prod]` fragment exactly as the compiler synthesizes it
/// (`compile::sources` serializes the selected section under its name).
const PROFILE_FRAGMENT: &str = r#"
[prod]
timeout_ms = 10000
routes = ["routes/main.yaml", "routes/orders.yaml"]
log_level = "${env:VS_PARITY_LOG}"
watch = true
"#;

const MAIN_ROUTE: &str =
    "routes:\n  - id: parity-main\n    from: \"direct:start\"\n    steps: []\n";
const ORDERS_ROUTE: &str =
    "routes:\n  - id: parity-orders\n    from: \"direct:orders\"\n    steps: []\n";

/// Path A: the real filesystem loader (sealed variant — explicit
/// profile, injected lookup, no `CAMEL_*` merge), the exact merge order
/// the virtual assembly mirrors.
fn load_filesystem(root: &std::path::Path) -> CamelConfig {
    let config_path = root.join("Camel.toml");
    CamelConfig::from_file_sealed(
        config_path.to_str().expect("utf-8 temp path"),
        "prod",
        &lookup,
    )
    .expect("filesystem load must succeed")
}

/// Path B: the same fixture packed as a virtual store and assembled the
/// way the compiled-artifact runtime does.
fn load_virtual() -> CamelConfig {
    let store = VirtualDocumentStore::build(
        "routes/main.yaml",
        &[
            StoreDocument {
                path: "Camel.toml".to_string(),
                kind: StoreEntryKind::Config,
                bytes: CONFIG_TEXT.as_bytes().to_vec(),
            },
            StoreDocument {
                path: "conf/base.toml".to_string(),
                kind: StoreEntryKind::Include,
                bytes: INCLUDE_TEXT.as_bytes().to_vec(),
            },
            StoreDocument {
                path: "prod.profile.toml".to_string(),
                kind: StoreEntryKind::Profile,
                bytes: PROFILE_FRAGMENT.as_bytes().to_vec(),
            },
            StoreDocument {
                path: "routes/main.yaml".to_string(),
                kind: StoreEntryKind::Route,
                bytes: MAIN_ROUTE.as_bytes().to_vec(),
            },
            StoreDocument {
                path: "routes/orders.yaml".to_string(),
                kind: StoreEntryKind::Route,
                bytes: ORDERS_ROUTE.as_bytes().to_vec(),
            },
        ],
        &[
            "Camel.toml".to_string(),
            "conf/base.toml".to_string(),
            "prod.profile.toml".to_string(),
        ],
        &[
            "routes/main.yaml".to_string(),
            "routes/orders.yaml".to_string(),
        ],
    )
    .expect("fixture store must build");

    let discovered =
        discover_virtual_store(&store, &lookup).expect("virtual-store discovery must succeed");
    CamelConfig::from_toml_value_with_env(discovered.config, &lookup)
        .expect("typed load of the merged tree must succeed")
}

/// Assert both loaders agree on every field the fixture exercises.
fn assert_parity(fs: &CamelConfig, vs: &CamelConfig) {
    assert_eq!(
        fs.log_level, vs.log_level,
        "configuration [prod] must outrank the include-only value on both paths"
    );
    assert_eq!(fs.log_level, "debug");
    assert_eq!(
        fs.timeout_ms, vs.timeout_ms,
        "[prod] must overlay [default] on both paths"
    );
    assert_eq!(fs.timeout_ms, 10000);
    assert_eq!(
        fs.routes, vs.routes,
        "the [prod] routes array must replace the [default] array on both paths"
    );
    assert_eq!(
        fs.routes,
        vec![
            "routes/main.yaml".to_string(),
            "routes/orders.yaml".to_string()
        ]
    );
    assert_eq!(fs.watch, vs.watch, "the [prod] watch flag must agree");
    assert!(fs.watch);
    assert_eq!(
        fs.components
            .raw
            .get("http")
            .and_then(|v| v.get("max_connections")),
        Some(&toml::Value::Integer(11)),
        "the include-only component leaf must survive on both paths"
    );
    assert_eq!(
        fs.components.raw, vs.components.raw,
        "component leaves must agree"
    );
    // `${env:}` resolution: the deployment lookup is the only source on
    // both paths (asserted through the log_level equality above, whose
    // `[prod]` value is a placeholder).
}

#[test]
fn virtual_store_config_matches_filesystem_loader() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("conf")).expect("conf dir");
    std::fs::create_dir_all(dir.path().join("routes")).expect("routes dir");
    std::fs::write(dir.path().join("Camel.toml"), CONFIG_TEXT).expect("write Camel.toml");
    std::fs::write(dir.path().join("conf/base.toml"), INCLUDE_TEXT).expect("write include");
    std::fs::write(dir.path().join("routes/main.yaml"), MAIN_ROUTE).expect("write main route");
    std::fs::write(dir.path().join("routes/orders.yaml"), ORDERS_ROUTE)
        .expect("write orders route");

    let fs = load_filesystem(dir.path());
    let vs = load_virtual();
    assert_parity(&fs, &vs);
}
