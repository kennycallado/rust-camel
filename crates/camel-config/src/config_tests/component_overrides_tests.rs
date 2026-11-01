use super::*;

fn write_temp_config(contents: &str) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    f.write_all(contents.as_bytes()).expect("write config");
    f
}

fn path(file: &tempfile::NamedTempFile) -> &str {
    file.path().to_str().expect("utf-8 temp path")
}

#[test]
fn component_overrides_reject_empty_key() {
    let file = write_temp_config(
        r#"
[observability.metrics.component_overrides]
"" = true
"#,
    );

    let err = CamelConfig::from_file(path(&file)).expect_err("empty key must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("observability.metrics.component_overrides"),
        "error must name the override path: {msg}"
    );
}

#[test]
fn component_overrides_reject_invalid_identity() {
    for key in ["HTTP", "http.v2"] {
        let file = write_temp_config(&format!(
            r#"
[observability.metrics.component_overrides]
"{key}" = true
"#
        ));

        let err = CamelConfig::from_file(path(&file))
            .expect_err("invalid component identity must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains(key),
            "error must name the invalid key {key:?}: {msg}"
        );
    }
}

#[test]
fn component_overrides_accept_valid_identities() {
    let file = write_temp_config(
        r#"
[observability.metrics.component_overrides]
http = true
kafka = true
"camel-https" = true
"a_b-9" = true
"#,
    );

    CamelConfig::from_file(path(&file)).expect("valid component identities must load");
}

#[test]
fn component_overrides_profile_merges_key_by_key() {
    let file = write_temp_config(
        r#"
[default.observability.metrics.component_overrides]
http = true
kafka = true

[prod.observability.metrics.component_overrides]
http = false
"#,
    );

    let cfg = CamelConfig::from_file_with_profile(path(&file), Some("prod"))
        .expect("profile config must load");
    let overrides = &cfg.observability.metrics.component_overrides;
    assert_eq!(overrides.get("http"), Some(&false));
    assert_eq!(overrides.get("kafka"), Some(&true));
}

#[test]
fn component_overrides_include_contributes() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("root.toml"), "include = [\"extra.toml\"]\n")
        .expect("write root config");
    std::fs::write(
        dir.path().join("extra.toml"),
        r#"
[observability.metrics.component_overrides]
kafka = true
"#,
    )
    .expect("write include config");

    let root = dir.path().join("root.toml");
    let cfg =
        CamelConfig::from_file(root.to_str().expect("utf-8 root path")).expect("config must load");
    assert_eq!(
        cfg.observability.metrics.component_overrides.get("kafka"),
        Some(&true)
    );
}

#[test]
fn component_overrides_unregistered_component_is_inert() {
    let file = write_temp_config(
        r#"
[observability.metrics.component_overrides]
not_a_component = true
"#,
    );

    let cfg = CamelConfig::from_file(path(&file)).expect("unknown component must be inert");
    let levers = cfg.observability.metrics.component_metrics_levers();
    assert!(
        levers.enabled_for("not_a_component"),
        "the override must be honoured by the scoped levers"
    );
    assert!(
        !levers.enabled_for("http"),
        "an unlisted component inherits the global default (off)"
    );
}
