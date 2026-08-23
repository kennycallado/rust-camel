use camel_component_redis::{RedisCommand, RedisEndpointConfig};

fn cfg_with(
    host: &str,
    port: u16,
    password: Option<&str>,
    db: u8,
    ssl: bool,
) -> RedisEndpointConfig {
    use camel_component_api::NetworkRetryPolicy;
    use camel_component_redis::sentinel_config::TopologyKind;
    RedisEndpointConfig {
        host: Some(host.to_string()),
        port: Some(port),
        command: RedisCommand::Set,
        channels: vec![],
        key: None,
        timeout: 1,
        password: password.map(|s| s.to_string()),
        db,
        ssl: Some(ssl),
        reconnect: NetworkRetryPolicy::default(),
        connection_timeout_secs: 10,
        topology_kind: TopologyKind::Standalone,
    }
}

fn assert_roundtrip(cfg: &RedisEndpointConfig) {
    let url = cfg.redis_url();
    let parsed = RedisEndpointConfig::from_uri(&url)
        .unwrap_or_else(|e| panic!("from_uri failed for url '{}': {}", url, e));
    assert_eq!(parsed.host, cfg.host, "host mismatch for url '{}'", url);
    assert_eq!(parsed.port, cfg.port, "port mismatch for url '{}'", url);
    assert_eq!(parsed.db, cfg.db, "db mismatch for url '{}'", url);
    assert_eq!(
        parsed.password, cfg.password,
        "password mismatch for url '{}'",
        url
    );
    // ssl: from_uri returns None for plain redis:// (absent param), cfg has Some(false)
    // They are equivalent when resolved to effective value.
    assert_eq!(
        parsed.ssl.unwrap_or(false),
        cfg.ssl.unwrap_or(false),
        "ssl mismatch for url '{}' parsed {:?} vs cfg {:?}",
        url,
        parsed.ssl,
        cfg.ssl
    );
    if cfg.db != 0 {
        assert!(
            url.contains("?db="),
            "expected ?db param for db={} in url '{}'",
            cfg.db,
            url
        );
        assert!(
            !url.contains(&format!("/{}", cfg.db)),
            "url should not contain db-in-path /{} but got '{}'",
            cfg.db,
            url
        );
    } else {
        assert!(
            !url.contains("?db="),
            "db=0 should not emit ?db param, got '{}'",
            url
        );
    }
}

#[test]
fn roundtrip_default_no_db() {
    let cfg = cfg_with("localhost", 6379, None, 0, false);
    assert_roundtrip(&cfg);
    assert_eq!(cfg.redis_url(), "redis://localhost:6379");
}

#[test]
fn roundtrip_db_zero_explicit() {
    // db=0 is default, should render without ?db
    let cfg = cfg_with("localhost", 6379, None, 0, false);
    assert_roundtrip(&cfg);
}

#[test]
fn roundtrip_db_nonzero() {
    let cfg = cfg_with("localhost", 6379, None, 5, false);
    assert_roundtrip(&cfg);
    assert_eq!(cfg.redis_url(), "redis://localhost:6379?db=5");
}

#[test]
fn roundtrip_db_max() {
    let cfg = cfg_with("localhost", 6379, None, 255, false);
    assert_roundtrip(&cfg);
}

#[test]
fn roundtrip_with_password() {
    let cfg = cfg_with("localhost", 6379, Some("secret"), 2, false);
    assert_roundtrip(&cfg);
    assert_eq!(cfg.redis_url(), "redis://:secret@localhost:6379?db=2");
}

#[test]
fn roundtrip_with_password_and_no_db() {
    let cfg = cfg_with("localhost", 6379, Some("secret"), 0, false);
    assert_roundtrip(&cfg);
    assert_eq!(cfg.redis_url(), "redis://:secret@localhost:6379");
}

#[test]
fn roundtrip_with_ssl() {
    let cfg = cfg_with("localhost", 6379, None, 0, true);
    assert_roundtrip(&cfg);
    assert_eq!(cfg.redis_url(), "rediss://localhost:6379");
}

#[test]
fn roundtrip_with_ssl_and_db() {
    let cfg = cfg_with("localhost", 6379, None, 3, true);
    assert_roundtrip(&cfg);
    assert_eq!(cfg.redis_url(), "rediss://localhost:6379?db=3");
}

#[test]
fn roundtrip_with_password_ssl_db() {
    let cfg = cfg_with("redis.example.com", 6380, Some("p@ss"), 7, true);
    // password with special chars is encoded
    let url = cfg.redis_url();
    // must contain %40 not raw @
    assert!(url.contains("%40"), "encoded @ expected: {}", url);
    let parsed = RedisEndpointConfig::from_uri(&url).unwrap();
    // from_uri decodes percent-encoded userinfo password back to original
    assert_eq!(parsed.password, Some("p@ss".to_string()));
    assert_eq!(parsed.db, 7);
    assert_eq!(parsed.ssl, Some(true));
}

#[test]
fn roundtrip_custom_host_port() {
    let cfg = cfg_with("redis-prod.example.com", 6381, None, 1, false);
    assert_roundtrip(&cfg);
}

#[test]
fn roundtrip_bare_username_no_password() {
    let parsed = RedisEndpointConfig::from_uri("redis://alice@localhost:6379").unwrap();
    assert_eq!(parsed.password, None);
    assert_eq!(parsed.host, Some("localhost".to_string()));
}
