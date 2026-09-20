//! Manifest-contract guards for the `binstall-metadata` change.
//!
//! `cargo binstall camel-cli` resolves its download URL from
//! `[package.metadata.binstall]` in this crate's `Cargo.toml`. These tests
//! pin that contract: the flat tarball asset names published to GitHub
//! releases, the flavor selection per target (full everywhere it exists,
//! regular on musl via a `cfg(target)` override — binstall's template
//! language has no feature variable), and the single-URL guarantee that
//! keeps binstall from iterating format extensions.

use std::fs;

/// Parse this crate's `Cargo.toml` into a `toml::Value`.
///
/// Shared by every test below; the manifest is the single source of truth
/// for the binstall contract, so each test re-reads it fresh.
fn manifest() -> toml::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let text = fs::read_to_string(path).expect("read camel-cli Cargo.toml");
    toml::from_str::<toml::Value>(&text).expect("camel-cli Cargo.toml parses as TOML")
}

#[test]
fn metadata_block_fields() {
    let binstall = &manifest()["package"]["metadata"]["binstall"];
    assert_eq!(
        binstall["pkg-url"].as_str(),
        Some("{repo}/releases/download/v{version}/camel-full-{target}.tar.gz")
    );
    assert_eq!(binstall["pkg-fmt"].as_str(), Some("tgz"));
    assert_eq!(binstall["bin-dir"].as_str(), Some("{bin}{binary-ext}"));
    assert_eq!(
        binstall["disabled-strategies"].as_array(),
        Some(&vec![toml::Value::String("quick-install".to_string())])
    );
}

#[test]
fn musl_override_targets_regular() {
    let overrides = &manifest()["package"]["metadata"]["binstall"]["overrides"];
    let table = overrides.as_table().expect("binstall overrides is a table");
    let keys: Vec<&str> = table.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec!["cfg(all(target_os = \"linux\", target_env = \"musl\"))"]
    );
    let musl = &overrides["cfg(all(target_os = \"linux\", target_env = \"musl\"))"];
    assert_eq!(
        musl["pkg-url"].as_str(),
        Some("{repo}/releases/download/v{version}/camel-{target}.tar.gz")
    );
    assert_eq!(musl.as_table().expect("musl override is a table").len(), 1);
}

#[test]
fn pkg_url_has_single_url_guarantee() {
    let binstall = &manifest()["package"]["metadata"]["binstall"];
    let default_url = binstall["pkg-url"]
        .as_str()
        .expect("default pkg-url is a string");
    let musl_url = binstall["overrides"]["cfg(all(target_os = \"linux\", target_env = \"musl\"))"]
        ["pkg-url"]
        .as_str()
        .expect("musl override pkg-url is a string");
    for url in [default_url, musl_url] {
        for placeholder in ["{archive-suffix}", "{archive-format}", "{format}"] {
            assert!(
                !url.contains(placeholder),
                "pkg-url {url:?} must not contain {placeholder:?}"
            );
        }
    }
}

#[test]
fn default_features_still_regular() {
    let default = &manifest()["features"]["default"];
    assert_eq!(
        default.as_array(),
        Some(&vec![toml::Value::String("flavor-regular".to_string())])
    );
}
