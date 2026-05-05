use camel_component_cxf::config::CxfPoolConfig;

#[test]
fn test_valid_full_config_with_security() {
    let toml_str = r#"
max_bridges = 12
bridge_start_timeout_ms = 45000
health_check_interval_ms = 9000
bridge_cache_dir = "/tmp/cxf-cache"
version = "9.9.9"

[[profiles]]
name = "orders"
address = "http://localhost:8080/ws"
wsdl_path = "classpath:OrderService.wsdl"
service_name = "OrderService"
port_name = "OrderPort"

[profiles.security]
keystore_path = "/etc/certs/keystore.jks"
keystore_password = "keystore-pass"
truststore_path = "/etc/certs/truststore.jks"
truststore_password = "truststore-pass"
sig_username = "soap-user"
sig_password = "soap-pass"
"#;

    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("config should parse");

    assert_eq!(cfg.max_bridges, 12);
    assert_eq!(cfg.bridge_start_timeout_ms, 45_000);
    assert_eq!(cfg.health_check_interval_ms, 9_000);
    assert_eq!(
        cfg.bridge_cache_dir.as_deref(),
        Some(std::path::Path::new("/tmp/cxf-cache"))
    );
    assert_eq!(cfg.version, "9.9.9");
    assert_eq!(cfg.profiles.len(), 1);

    let p = &cfg.profiles[0];
    assert_eq!(p.name, "orders");
    assert_eq!(p.address.as_deref(), Some("http://localhost:8080/ws"));
    assert_eq!(p.wsdl_path, "classpath:OrderService.wsdl");
    assert_eq!(p.service_name, "OrderService");
    assert_eq!(p.port_name, "OrderPort");
    assert_eq!(
        p.security.keystore_path.as_deref(),
        Some("/etc/certs/keystore.jks")
    );
    assert_eq!(
        p.security.keystore_password.as_deref(),
        Some("keystore-pass")
    );
    assert_eq!(
        p.security.truststore_path.as_deref(),
        Some("/etc/certs/truststore.jks")
    );
    assert_eq!(
        p.security.truststore_password.as_deref(),
        Some("truststore-pass")
    );
    assert_eq!(p.security.sig_username.as_deref(), Some("soap-user"));
    assert_eq!(p.security.sig_password.as_deref(), Some("soap-pass"));
}

#[test]
fn test_minimal_config_only_required_fields() {
    let toml_str = r#"
[[profiles]]
name = "myservice"
wsdl_path = "service.wsdl"
service_name = "MyService"
port_name = "MyPort"
"#;

    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("config should parse");

    assert_eq!(cfg.profiles.len(), 1);
    assert_eq!(cfg.max_bridges, 4);
    assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
    assert_eq!(cfg.health_check_interval_ms, 5_000);
    assert!(cfg.bridge_cache_dir.is_none());
    assert_eq!(cfg.version, camel_component_cxf::BRIDGE_VERSION);

    let p = &cfg.profiles[0];
    assert_eq!(p.name, "myservice");
    assert!(p.address.is_none());
    assert_eq!(p.wsdl_path, "service.wsdl");
    assert_eq!(p.service_name, "MyService");
    assert_eq!(p.port_name, "MyPort");
    assert!(p.security.keystore_path.is_none());
    assert!(p.security.keystore_password.is_none());
    assert!(p.security.truststore_path.is_none());
    assert!(p.security.truststore_password.is_none());
}

#[test]
fn test_invalid_config_missing_wsdl_path() {
    let toml_str = r#"
[[profiles]]
name = "bad"
service_name = "MyService"
port_name = "MyPort"
"#;

    let err = toml::from_str::<CxfPoolConfig>(toml_str).expect_err("config must fail");
    assert!(err.to_string().contains("wsdl_path"));
}

#[test]
fn test_invalid_config_missing_service_name() {
    let toml_str = r#"
[[profiles]]
name = "bad"
wsdl_path = "service.wsdl"
port_name = "MyPort"
"#;

    let err = toml::from_str::<CxfPoolConfig>(toml_str).expect_err("config must fail");
    assert!(err.to_string().contains("service_name"));
}

#[test]
fn test_invalid_config_missing_port_name() {
    let toml_str = r#"
[[profiles]]
name = "bad"
wsdl_path = "service.wsdl"
service_name = "MyService"
"#;

    let err = toml::from_str::<CxfPoolConfig>(toml_str).expect_err("config must fail");
    assert!(err.to_string().contains("port_name"));
}

#[test]
fn test_env_interpolation_preserved_in_passwords() {
    let toml_str = r#"
[[profiles]]
name = "envservice"
wsdl_path = "service.wsdl"
service_name = "EnvService"
port_name = "EnvPort"

[profiles.security]
sig_password = "${env:SOAP_PASSWORD}"
keystore_password = "${env:KEYSTORE_PASSWORD}"
truststore_password = "${env:TRUSTSTORE_PASSWORD}"
"#;

    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("config should parse");
    let sec = &cfg.profiles[0].security;

    assert_eq!(sec.sig_password.as_deref(), Some("${env:SOAP_PASSWORD}"));
    assert_eq!(
        sec.keystore_password.as_deref(),
        Some("${env:KEYSTORE_PASSWORD}")
    );
    assert_eq!(
        sec.truststore_password.as_deref(),
        Some("${env:TRUSTSTORE_PASSWORD}")
    );
}

#[test]
fn test_multiple_profiles_config() {
    let toml_str = r#"
max_bridges = 6

[[profiles]]
name = "one"
address = "http://localhost:8080/one"
wsdl_path = "one.wsdl"
service_name = "ServiceOne"
port_name = "PortOne"

[[profiles]]
name = "two"
address = "http://localhost:8080/two"
wsdl_path = "two.wsdl"
service_name = "ServiceTwo"
port_name = "PortTwo"
"#;

    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("config should parse");

    assert_eq!(cfg.max_bridges, 6);
    assert_eq!(cfg.profiles.len(), 2);
    assert_eq!(cfg.profiles[0].name, "one");
    assert_eq!(
        cfg.profiles[0].address.as_deref(),
        Some("http://localhost:8080/one")
    );
    assert_eq!(cfg.profiles[0].wsdl_path, "one.wsdl");
    assert_eq!(cfg.profiles[0].service_name, "ServiceOne");
    assert_eq!(cfg.profiles[0].port_name, "PortOne");
    assert_eq!(cfg.profiles[1].name, "two");
    assert_eq!(
        cfg.profiles[1].address.as_deref(),
        Some("http://localhost:8080/two")
    );
    assert_eq!(cfg.profiles[1].wsdl_path, "two.wsdl");
    assert_eq!(cfg.profiles[1].service_name, "ServiceTwo");
    assert_eq!(cfg.profiles[1].port_name, "PortTwo");
}

#[test]
fn test_empty_profiles_list_is_valid() {
    let toml_str = r#"
max_bridges = 3
"#;

    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("config should parse");

    assert_eq!(cfg.max_bridges, 3);
    assert!(cfg.profiles.is_empty());
    assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
    assert_eq!(cfg.health_check_interval_ms, 5_000);
}
