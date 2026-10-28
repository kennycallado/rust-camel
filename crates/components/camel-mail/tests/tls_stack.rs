//! Scenario "ring-only TLS stack", checked hermetically on the manifests:
//! the TLS-relevant dependency declarations of this crate and their
//! workspace entries select ring and never an aws-lc backend or rustls'
//! default features. No network, no cargo subprocess.

use std::path::Path;

use toml::Table;
use toml::Value;

const TLS_DEPS: [&str; 3] = ["lettre", "tokio-rustls", "webpki-roots"];

fn manifest(path: &Path) -> Table {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn table<'a>(t: &'a Table, key: &str) -> &'a Table {
    t.get(key)
        .and_then(Value::as_table)
        .unwrap_or_else(|| panic!("missing table [{key}]"))
}

fn features(entry: &Value) -> Vec<String> {
    entry
        .get("features")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn default_features(entry: &Value) -> bool {
    entry
        .get("default-features")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn assert_no_aws(dep: &str, entry: &Value) {
    let f = features(entry);
    assert!(
        !f.iter().any(|f| f.contains("aws")),
        "{dep} selects an aws-lc feature: {f:?}"
    );
}

#[test]
fn ring_only_tls_stack() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let own = manifest(&crate_dir.join("Cargo.toml"));
    let ws = manifest(&crate_dir.join("../../../Cargo.toml"));
    let own_deps = table(&own, "dependencies");
    let ws_deps = table(table(&ws, "workspace"), "dependencies");

    assert!(
        !own_deps.contains_key("rustls"),
        "rustls is reached through tokio-rustls only"
    );
    for (dep, entry) in own_deps {
        assert_no_aws(dep, entry);
    }
    for dep in TLS_DEPS {
        let own_entry = own_deps
            .get(dep)
            .unwrap_or_else(|| panic!("{dep} is a dependency"));
        assert_eq!(
            own_entry.get("workspace").and_then(Value::as_bool),
            Some(true),
            "{dep} must come from the workspace entry"
        );
        let ws_entry = ws_deps
            .get(dep)
            .unwrap_or_else(|| panic!("workspace entry for {dep}"));
        assert_no_aws(dep, ws_entry);
    }
    for dep in ["lettre", "tokio-rustls"] {
        let entry = &ws_deps[dep];
        assert!(
            !default_features(entry),
            "{dep} must set default-features = false"
        );
        assert!(
            features(entry).iter().any(|f| f == "ring"),
            "{dep} must select ring: {:?}",
            features(entry)
        );
    }
    let webpki = &ws_deps["webpki-roots"];
    assert!(
        webpki.is_str() || features(webpki).is_empty(),
        "webpki-roots needs no features"
    );
}
