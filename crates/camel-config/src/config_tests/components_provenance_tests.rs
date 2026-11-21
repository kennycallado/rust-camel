use super::*;

/// A default `ComponentsConfig` carries no provenance: the block accessor
/// hands back an empty set.
#[test]
fn provenance_for_empty_default() {
    let cfg = ComponentsConfig::default();
    assert!(
        cfg.provenance_for("redis").paths().is_empty(),
        "a default config has no recorded paths"
    );
}

/// Equality is over `raw` only: provenance is metadata about how values
/// arrived, not a value, so a default-vs-loaded comparison still holds.
#[test]
fn components_equality_ignores_provenance() {
    let raw = HashMap::from([("redis".to_string(), toml::Value::String("keep".to_string()))]);
    let mut with_provenance = ComponentsConfig {
        raw: raw.clone(),
        provenance: HashMap::new(),
    };
    with_provenance.provenance.insert(
        "redis".to_string(),
        ProvenanceSet::from_paths(vec![vec![ConfigSeg::Key("port".to_string())]]),
    );
    let without_provenance = ComponentsConfig {
        raw,
        provenance: HashMap::new(),
    };

    assert_eq!(without_provenance, with_provenance);
}

/// Exact structural paths, traversal order pinned: the loader records the
/// block-relative token leaf of `redis.port`.
#[test]
fn component_block_provenance_paths_exact() {
    let value: toml::Value = toml::from_str(
        r#"[components.redis]
port = "${env:CFG_REDIS_PORT:-6379}"
"#,
    )
    .expect("test fixture must be valid TOML");
    let cfg = CamelConfig::from_toml_value_with_env(value, &|_| None)
        .expect("virtual-store load must succeed");

    assert_eq!(
        cfg.components.provenance_for("redis").paths().to_vec(),
        vec![vec![ConfigSeg::Key("port".to_string())]],
        "only the block-relative token leaf is recorded"
    );
}
