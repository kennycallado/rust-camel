use camel_component_cxf::config::CxfPoolConfig;

#[test]
fn test_valid_full_config_with_security() {
    let toml_str = r#"
max_bridges = 12
bridge_start_timeout_ms = 45000
health_check_interval_ms = 9000
bridge_cache_dir = "/tmp/cxf-cache"
version = "9.9.9"

[[services]]
address = "http://localhost:8080/ws"
wsdl_path = "classpath:OrderService.wsdl"
service_name = "OrderService"
port_name = "OrderPort"

[services.security]
username = "soap-user"
password = "soap-pass"
keystore_path = "/etc/certs/keystore.jks"
keystore_password = "keystore-pass"
truststore_path = "/etc/certs/truststore.jks"
truststore_password = "truststore-pass"
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
    assert_eq!(cfg.services.len(), 1);

    let svc = &cfg.services[0];
    assert_eq!(svc.address.as_deref(), Some("http://localhost:8080/ws"));
    assert_eq!(svc.wsdl_path, "classpath:OrderService.wsdl");
    assert_eq!(svc.service_name, "OrderService");
    assert_eq!(svc.port_name, "OrderPort");
    assert_eq!(svc.security.username.as_deref(), Some("soap-user"));
    assert_eq!(svc.security.password.as_deref(), Some("soap-pass"));
    assert_eq!(
        svc.security.keystore_path.as_deref(),
        Some("/etc/certs/keystore.jks")
    );
    assert_eq!(
        svc.security.keystore_password.as_deref(),
        Some("keystore-pass")
    );
    assert_eq!(
        svc.security.truststore_path.as_deref(),
        Some("/etc/certs/truststore.jks")
    );
    assert_eq!(
        svc.security.truststore_password.as_deref(),
        Some("truststore-pass")
    );
}

#[test]
fn test_minimal_config_only_required_fields() {
    let toml_str = r#"
[[services]]
wsdl_path = "service.wsdl"
service_name = "MyService"
port_name = "MyPort"
"#;

    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("config should parse");

    assert_eq!(cfg.services.len(), 1);
    assert_eq!(cfg.max_bridges, 4);
    assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
    assert_eq!(cfg.health_check_interval_ms, 5_000);
    assert!(cfg.bridge_cache_dir.is_none());
    assert_eq!(cfg.version, camel_component_cxf::BRIDGE_VERSION);

    let svc = &cfg.services[0];
    assert!(svc.address.is_none());
    assert_eq!(svc.wsdl_path, "service.wsdl");
    assert_eq!(svc.service_name, "MyService");
    assert_eq!(svc.port_name, "MyPort");
    assert!(svc.security.username.is_none());
    assert!(svc.security.password.is_none());
    assert!(svc.security.keystore_path.is_none());
    assert!(svc.security.keystore_password.is_none());
    assert!(svc.security.truststore_path.is_none());
    assert!(svc.security.truststore_password.is_none());
}

#[test]
fn test_invalid_config_missing_wsdl_path() {
    let toml_str = r#"
[[services]]
service_name = "MyService"
port_name = "MyPort"
"#;

    let err = toml::from_str::<CxfPoolConfig>(toml_str).expect_err("config must fail");
    assert!(err.to_string().contains("wsdl_path"));
}

#[test]
fn test_invalid_config_missing_service_name() {
    let toml_str = r#"
[[services]]
wsdl_path = "service.wsdl"
port_name = "MyPort"
"#;

    let err = toml::from_str::<CxfPoolConfig>(toml_str).expect_err("config must fail");
    assert!(err.to_string().contains("service_name"));
}

#[test]
fn test_invalid_config_missing_port_name() {
    let toml_str = r#"
[[services]]
wsdl_path = "service.wsdl"
service_name = "MyService"
"#;

    let err = toml::from_str::<CxfPoolConfig>(toml_str).expect_err("config must fail");
    assert!(err.to_string().contains("port_name"));
}

#[test]
fn test_env_interpolation_preserved_in_passwords() {
    let toml_str = r#"
[[services]]
wsdl_path = "service.wsdl"
service_name = "EnvService"
port_name = "EnvPort"

[services.security]
password = "${env:SOAP_PASSWORD}"
keystore_password = "${env:KEYSTORE_PASSWORD}"
truststore_password = "${env:TRUSTSTORE_PASSWORD}"
"#;

    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("config should parse");
    let sec = &cfg.services[0].security;

    assert_eq!(sec.password.as_deref(), Some("${env:SOAP_PASSWORD}"));
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
fn test_multiple_services_config() {
    let toml_str = r#"
max_bridges = 6

[[services]]
address = "http://localhost:8080/one"
wsdl_path = "one.wsdl"
service_name = "ServiceOne"
port_name = "PortOne"

[[services]]
address = "http://localhost:8080/two"
wsdl_path = "two.wsdl"
service_name = "ServiceTwo"
port_name = "PortTwo"
"#;

    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("config should parse");

    assert_eq!(cfg.max_bridges, 6);
    assert_eq!(cfg.services.len(), 2);
    assert_eq!(
        cfg.services[0].address.as_deref(),
        Some("http://localhost:8080/one")
    );
    assert_eq!(cfg.services[0].wsdl_path, "one.wsdl");
    assert_eq!(cfg.services[0].service_name, "ServiceOne");
    assert_eq!(cfg.services[0].port_name, "PortOne");
    assert_eq!(
        cfg.services[1].address.as_deref(),
        Some("http://localhost:8080/two")
    );
    assert_eq!(cfg.services[1].wsdl_path, "two.wsdl");
    assert_eq!(cfg.services[1].service_name, "ServiceTwo");
    assert_eq!(cfg.services[1].port_name, "PortTwo");
}

#[test]
fn test_empty_services_list_is_valid() {
    let toml_str = r#"
max_bridges = 3
"#;

    let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("config should parse");

    assert_eq!(cfg.max_bridges, 3);
    assert!(cfg.services.is_empty());
    assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
    assert_eq!(cfg.health_check_interval_ms, 5_000);
}
