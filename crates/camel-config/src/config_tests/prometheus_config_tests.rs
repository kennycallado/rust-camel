use super::*;

fn parse(toml: &str) -> CamelConfig {
    let cfg = config::Config::builder()
        .add_source(config::File::from_str(toml, config::FileFormat::Toml))
        .build()
        .unwrap();
    cfg.try_deserialize().unwrap()
}

#[test]
fn test_prometheus_absent_is_none() {
    let cfg = parse("");
    assert!(cfg.observability.prometheus.is_none());
}

#[test]
fn test_prometheus_defaults() {
    let cfg = parse(
        r#"
[observability.prometheus]
enabled = true
"#,
    );
    let p = cfg.observability.prometheus.unwrap();
    assert!(p.enabled);
    assert_eq!(p.host, "0.0.0.0");
    assert_eq!(p.port, 9090);
}

#[test]
fn test_prometheus_full() {
    let cfg = parse(
        r#"
[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = 9091
"#,
    );
    let p = cfg.observability.prometheus.unwrap();
    assert_eq!(p.host, "127.0.0.1");
    assert_eq!(p.port, 9091);
}

#[test]
fn test_health_config_defaults() {
    let cfg = parse(
        r#"
[observability.health]
enabled = true
"#,
    );
    let h = cfg.observability.health.unwrap();
    assert!(h.enabled);
    assert_eq!(h.host, "0.0.0.0");
    assert_eq!(h.port, 8081);
}

#[test]
fn test_health_config_custom_port() {
    let cfg = parse(
        r#"
[observability.health]
enabled = true
port = 9091
"#,
    );
    let h = cfg.observability.health.unwrap();
    assert_eq!(h.port, 9091);
    assert_eq!(h.host, "0.0.0.0");
}
