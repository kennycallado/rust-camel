use super::*;

#[test]
fn components_config_deserializes_raw_toml_block() {
    let toml_str = r#"
            [kafka]
            brokers = ["localhost:9092"]

            [redis]
            host = "redis.local"
        "#;
    let cfg: ComponentsConfig = toml::from_str(toml_str).unwrap();
    assert!(cfg.raw.contains_key("kafka"));
    assert!(cfg.raw.contains_key("redis"));
}
