use std::fmt;
use std::path::PathBuf;

use crate::BRIDGE_VERSION;

fn default_max_bridges() -> usize {
    4
}

fn default_bridge_start_timeout_ms() -> u64 {
    30_000
}

fn default_health_check_interval_ms() -> u64 {
    5_000
}

#[derive(Clone, Default, serde::Deserialize)]
pub struct CxfSecurityConfig {
    pub username: Option<String>,
    pub password: Option<String>,
    pub keystore_path: Option<String>,
    pub keystore_password: Option<String>,
    pub truststore_path: Option<String>,
    pub truststore_password: Option<String>,
}

impl fmt::Debug for CxfSecurityConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CxfSecurityConfig")
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("keystore_path", &self.keystore_path)
            .field(
                "keystore_password",
                &self.keystore_password.as_ref().map(|_| "<redacted>"),
            )
            .field("truststore_path", &self.truststore_path)
            .field(
                "truststore_password",
                &self.truststore_password.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct CxfServiceConfig {
    pub address: Option<String>,
    pub wsdl_path: String,
    pub service_name: String,
    pub port_name: String,
    #[serde(default)]
    pub security: CxfSecurityConfig,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct CxfPoolConfig {
    #[serde(default)]
    pub services: Vec<CxfServiceConfig>,
    #[serde(default = "default_max_bridges")]
    pub max_bridges: usize,
    #[serde(default = "default_bridge_start_timeout_ms")]
    pub bridge_start_timeout_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
    pub bridge_cache_dir: Option<PathBuf>,
    #[serde(default = "default_bridge_version")]
    pub version: String,
}

fn default_bridge_version() -> String {
    BRIDGE_VERSION.to_string()
}

impl Default for CxfPoolConfig {
    fn default() -> Self {
        Self {
            services: Vec::new(),
            max_bridges: default_max_bridges(),
            bridge_start_timeout_ms: default_bridge_start_timeout_ms(),
            health_check_interval_ms: default_health_check_interval_ms(),
            bridge_cache_dir: None,
            version: default_bridge_version(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CxfEndpointConfig {
    pub address: String,
    pub wsdl_path: String,
    pub service_name: String,
    pub port_name: String,
    pub operation: Option<String>,
}

impl CxfEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, camel_component_api::CamelError> {
        let rest = uri.strip_prefix("cxf://").ok_or_else(|| {
            camel_component_api::CamelError::ProcessorError("expected scheme 'cxf://'".to_string())
        })?;

        let (path, query) = match rest.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (rest, None),
        };

        let address = if path.is_empty() {
            return Err(camel_component_api::CamelError::ProcessorError(
                "cxf URI must include an address after 'cxf://'".to_string(),
            ));
        } else {
            path.to_string()
        };

        let mut wsdl_path = None;
        let mut service_name = None;
        let mut port_name = None;
        let mut operation = None;

        if let Some(q) = query {
            for kv in q.split('&') {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "wsdl" => wsdl_path = Some(v.to_string()),
                        "service" => service_name = Some(v.to_string()),
                        "port" => port_name = Some(v.to_string()),
                        "operation" => operation = Some(v.to_string()),
                        _ => {}
                    }
                }
            }
        }

        let wsdl_path = wsdl_path.ok_or_else(|| {
            camel_component_api::CamelError::ProcessorError(
                "cxf URI requires 'wsdl' query parameter".to_string(),
            )
        })?;
        let service_name = service_name.ok_or_else(|| {
            camel_component_api::CamelError::ProcessorError(
                "cxf URI requires 'service' query parameter".to_string(),
            )
        })?;
        let port_name = port_name.ok_or_else(|| {
            camel_component_api::CamelError::ProcessorError(
                "cxf URI requires 'port' query parameter".to_string(),
            )
        })?;

        Ok(CxfEndpointConfig {
            address,
            wsdl_path,
            service_name,
            port_name,
            operation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cxf_minimal() {
        let err = CxfEndpointConfig::from_uri("cxf://http://localhost:8080/service").unwrap_err();
        assert!(err.to_string().contains("requires 'wsdl'"));
    }

    #[test]
    fn parse_cxf_with_params() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://localhost:8080/service?wsdl=service.wsdl&service=MyService&port=MyPort&operation=doSomething",
        )
        .unwrap();
        assert_eq!(cfg.address, "http://localhost:8080/service");
        assert_eq!(cfg.wsdl_path, "service.wsdl".to_string());
        assert_eq!(cfg.service_name, "MyService".to_string());
        assert_eq!(cfg.port_name, "MyPort".to_string());
        assert_eq!(cfg.operation, Some("doSomething".to_string()));
    }

    #[test]
    fn parse_cxf_missing_service_param() {
        let err = CxfEndpointConfig::from_uri(
            "cxf://http://localhost:8080/service?wsdl=service.wsdl&port=MyPort",
        )
        .unwrap_err();
        assert!(err.to_string().contains("requires 'service'"));
    }

    #[test]
    fn parse_cxf_missing_port_param() {
        let err = CxfEndpointConfig::from_uri(
            "cxf://http://localhost:8080/service?wsdl=service.wsdl&service=MyService",
        )
        .unwrap_err();
        assert!(err.to_string().contains("requires 'port'"));
    }

    #[test]
    fn parse_cxf_wrong_scheme() {
        let err = CxfEndpointConfig::from_uri("http://localhost:8080/service").unwrap_err();
        assert!(err.to_string().contains("cxf://"));
    }

    #[test]
    fn parse_cxf_empty_address() {
        let err = CxfEndpointConfig::from_uri("cxf://").unwrap_err();
        assert!(err.to_string().contains("address"));
    }

    #[test]
    fn default_pool_config() {
        let cfg = CxfPoolConfig::default();
        assert_eq!(cfg.max_bridges, 4);
        assert!(cfg.services.is_empty());
        assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
        assert_eq!(cfg.health_check_interval_ms, 5_000);
        assert_eq!(cfg.version, BRIDGE_VERSION);
    }

    #[test]
    fn parse_pool_config_from_toml() {
        let toml_str = r#"
            max_bridges = 8
            bridge_start_timeout_ms = 60_000
            health_check_interval_ms = 10_000

            [[services]]
            address = "http://localhost:9090/ws"
            wsdl_path = "service.wsdl"
            service_name = "MyService"
            port_name = "MyPort"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        assert_eq!(cfg.max_bridges, 8);
        assert_eq!(cfg.bridge_start_timeout_ms, 60_000);
        assert_eq!(cfg.health_check_interval_ms, 10_000);
        assert_eq!(cfg.services.len(), 1);
        assert_eq!(
            cfg.services[0].address,
            Some("http://localhost:9090/ws".to_string())
        );
        assert_eq!(cfg.services[0].wsdl_path, "service.wsdl");
        assert_eq!(cfg.services[0].service_name, "MyService");
        assert_eq!(cfg.services[0].port_name, "MyPort");
    }

    #[test]
    fn parse_pool_config_defaults() {
        let cfg: CxfPoolConfig = toml::from_str("").expect("empty toml");
        assert_eq!(cfg.max_bridges, 4);
        assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
        assert_eq!(cfg.health_check_interval_ms, 5_000);
        assert!(cfg.services.is_empty());
        assert!(cfg.bridge_cache_dir.is_none());
    }

    #[test]
    fn parse_security_config() {
        let toml_str = r#"
            [[services]]
            address = "http://localhost:8080/ws"
            wsdl_path = "service.wsdl"
            service_name = "S"
            port_name = "P"

            [services.security]
            username = "user"
            password = "pass"
            keystore_path = "/path/keystore.jks"
            keystore_password = "ks-pass"
            truststore_path = "/path/truststore.jks"
            truststore_password = "ts-pass"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        let sec = &cfg.services[0].security;
        assert_eq!(sec.username, Some("user".to_string()));
        assert_eq!(sec.password, Some("pass".to_string()));
        assert_eq!(sec.keystore_path, Some("/path/keystore.jks".to_string()));
        assert_eq!(sec.keystore_password, Some("ks-pass".to_string()));
        assert_eq!(
            sec.truststore_path,
            Some("/path/truststore.jks".to_string())
        );
        assert_eq!(sec.truststore_password, Some("ts-pass".to_string()));
    }

    #[test]
    fn parse_security_config_empty() {
        let toml_str = r#"
            [[services]]
            address = "http://localhost:8080/ws"
            wsdl_path = "service.wsdl"
            service_name = "S"
            port_name = "P"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        let sec = &cfg.services[0].security;
        assert!(sec.username.is_none());
        assert!(sec.password.is_none());
        assert!(sec.keystore_path.is_none());
        assert!(sec.keystore_password.is_none());
        assert!(sec.truststore_path.is_none());
        assert!(sec.truststore_password.is_none());
    }

    #[test]
    fn endpoint_config_from_uri_full() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://example.com/ws?wsdl=file.wsdl&service=MySvc&port=MyPort&operation=doWork",
        )
        .unwrap();
        assert_eq!(cfg.address, "http://example.com/ws");
        assert_eq!(cfg.wsdl_path, "file.wsdl".to_string());
        assert_eq!(cfg.service_name, "MySvc".to_string());
        assert_eq!(cfg.port_name, "MyPort".to_string());
        assert_eq!(cfg.operation, Some("doWork".to_string()));
    }

    #[test]
    fn endpoint_config_from_uri_minimal() {
        let err = CxfEndpointConfig::from_uri("cxf://http://example.com/ws").unwrap_err();
        assert!(err.to_string().contains("requires 'wsdl'"));
    }
}
