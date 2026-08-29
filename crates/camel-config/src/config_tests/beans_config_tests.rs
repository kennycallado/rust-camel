use super::*;

#[test]
fn beans_default_empty() {
    let config: CamelConfig = toml::from_str("").unwrap();
    assert!(config.beans.is_empty());
}

#[test]
fn beans_parsed_from_config() {
    let toml_str = r#"
[beans.auth]
plugin = "my-auth"

[beans.cache]
plugin = "my-cache"
"#;
    let config: CamelConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.beans.len(), 2);
    assert_eq!(config.beans.get("auth").unwrap().plugin, "my-auth");
    assert_eq!(config.beans.get("cache").unwrap().plugin, "my-cache");
}

#[test]
fn beans_config_parsed_from_toml() {
    let toml_str = r#"
[beans.auth]
plugin = "my-auth"
[beans.auth.config]
api_key = "${API_KEY}"
base_url = "https://api.example.com"
"#;
    let config: CamelConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.beans.len(), 1);
    let auth = config.beans.get("auth").unwrap();
    assert_eq!(auth.plugin, "my-auth");
    assert_eq!(auth.config.get("api_key").unwrap(), "${API_KEY}");
    assert_eq!(
        auth.config.get("base_url").unwrap(),
        "https://api.example.com"
    );
}

#[test]
fn beans_config_defaults_to_empty_map() {
    let toml_str = r#"
[beans.auth]
plugin = "my-auth"
"#;
    let config: CamelConfig = toml::from_str(toml_str).unwrap();
    assert!(config.beans.get("auth").unwrap().config.is_empty());
}

#[test]
fn beans_config_with_profiles_merges() {
    let toml_str = r#"
[default.beans.auth]
plugin = "my-auth"
[default.beans.auth.config]
base_url = "https://dev.example.com"

[production.beans.auth.config]
base_url = "https://prod.example.com"
"#;
    let config_value: toml::Value = toml::from_str(toml_str).unwrap();
    let default_val = config_value.get("default").cloned().unwrap();
    let prod_overlay = config_value.get("production").cloned().unwrap();
    let mut merged = default_val;
    super::merge_toml_values(&mut merged, &prod_overlay);
    let config: CamelConfig = merged.try_into().unwrap();
    let auth = config.beans.get("auth").unwrap();
    assert_eq!(
        auth.config.get("base_url").unwrap(),
        "https://prod.example.com"
    );
}
