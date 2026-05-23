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

/// Validates that a profile name contains only `[a-z0-9_]+`.
pub fn validate_profile_name(name: &str) -> Result<(), camel_component_api::CamelError> {
    if name.is_empty() {
        return Err(camel_component_api::CamelError::ProcessorError(
            "profile name must not be empty".to_string(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(camel_component_api::CamelError::ProcessorError(format!(
            "profile name '{}' must contain only lowercase letters, digits, and underscores",
            name
        )));
    }
    Ok(())
}

#[derive(Clone, Default, serde::Deserialize)]
pub struct CxfSecurityFields {
    pub keystore_path: Option<String>,
    pub keystore_password: Option<String>,
    pub truststore_path: Option<String>,
    pub truststore_password: Option<String>,
    pub sig_username: Option<String>,
    pub sig_password: Option<String>,
    pub enc_username: Option<String>,
    pub security_actions_out: Option<String>,
    pub security_actions_in: Option<String>,
    pub signature_algorithm: Option<String>,
    pub signature_digest_algorithm: Option<String>,
    pub signature_c14n_algorithm: Option<String>,
    pub signature_parts: Option<String>,
}

impl fmt::Debug for CxfSecurityFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CxfSecurityFields")
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
            .field("sig_username", &self.sig_username)
            .field(
                "sig_password",
                &self.sig_password.as_ref().map(|_| "<redacted>"),
            )
            .field("enc_username", &self.enc_username)
            .field("security_actions_out", &self.security_actions_out)
            .field("security_actions_in", &self.security_actions_in)
            .field("signature_algorithm", &self.signature_algorithm)
            .field(
                "signature_digest_algorithm",
                &self.signature_digest_algorithm,
            )
            .field("signature_c14n_algorithm", &self.signature_c14n_algorithm)
            .field("signature_parts", &self.signature_parts)
            .finish()
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct CxfProfileConfig {
    pub name: String,
    pub address: Option<String>,
    pub wsdl_path: String,
    pub service_name: String,
    pub port_name: String,
    #[serde(default)]
    pub security: CxfSecurityFields,
}

impl CxfProfileConfig {
    pub fn env_prefix(&self) -> String {
        format!("CXF_PROFILE_{}_", self.name.to_uppercase())
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(try_from = "CxfPoolConfigRaw")]
pub struct CxfPoolConfig {
    #[serde(default)]
    pub profiles: Vec<CxfProfileConfig>,
    #[serde(default = "default_max_bridges")]
    pub max_bridges: usize,
    #[serde(default = "default_bridge_start_timeout_ms")]
    pub bridge_start_timeout_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
    pub bridge_cache_dir: Option<PathBuf>,
    #[serde(default = "default_bridge_version")]
    pub version: String,
    /// Optional HTTP bind address for the consumer-side SOAP endpoint published by
    /// the bridge process. Forwarded to the bridge as the `CXF_ADDRESS` env var.
    /// When unset the bridge falls back to its built-in default (`http://0.0.0.0:9000/cxf`).
    /// Format: `http://<host>:<port>/<base-path>` — profiles are routed under
    /// `<base-path>/<profile_name>` by the bridge.
    #[serde(default)]
    pub bind_address: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct CxfPoolConfigRaw {
    #[serde(default)]
    profiles: Vec<CxfProfileConfig>,
    #[serde(default = "default_max_bridges")]
    max_bridges: usize,
    #[serde(default = "default_bridge_start_timeout_ms")]
    bridge_start_timeout_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    health_check_interval_ms: u64,
    bridge_cache_dir: Option<PathBuf>,
    #[serde(default = "default_bridge_version")]
    version: String,
    #[serde(default)]
    bind_address: Option<String>,
}

fn default_bridge_version() -> String {
    BRIDGE_VERSION.to_string()
}

impl Default for CxfPoolConfig {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            max_bridges: default_max_bridges(),
            bridge_start_timeout_ms: default_bridge_start_timeout_ms(),
            health_check_interval_ms: default_health_check_interval_ms(),
            bridge_cache_dir: None,
            version: default_bridge_version(),
            bind_address: None,
        }
    }
}

impl CxfPoolConfig {
    pub fn validate(&self) -> Result<(), camel_component_api::CamelError> {
        if self.max_bridges == 0 {
            return Err(camel_component_api::CamelError::Config(
                "cxf.max_bridges must be greater than 0".into(),
            ));
        }
        if self.bridge_start_timeout_ms == 0 {
            return Err(camel_component_api::CamelError::Config(
                "cxf.bridge_start_timeout_ms must be greater than 0".into(),
            ));
        }
        if self.health_check_interval_ms == 0 {
            return Err(camel_component_api::CamelError::Config(
                "cxf.health_check_interval_ms must be greater than 0".into(),
            ));
        }
        Ok(())
    }
}

impl TryFrom<CxfPoolConfigRaw> for CxfPoolConfig {
    type Error = camel_component_api::CamelError;

    fn try_from(raw: CxfPoolConfigRaw) -> Result<Self, Self::Error> {
        let cfg = CxfPoolConfig {
            profiles: raw.profiles,
            max_bridges: raw.max_bridges,
            bridge_start_timeout_ms: raw.bridge_start_timeout_ms,
            health_check_interval_ms: raw.health_check_interval_ms,
            bridge_cache_dir: raw.bridge_cache_dir,
            version: raw.version,
            bind_address: raw.bind_address,
        };
        cfg.validate()?;
        Ok(cfg)
    }
}

#[derive(Debug, Clone)]
pub struct CxfEndpointConfig {
    pub address: String,
    pub wsdl_path: String,
    pub service_name: String,
    pub port_name: String,
    pub operation: Option<String>,
    pub profile: Option<String>,
    pub timeout_ms: Option<u64>,
    /// When `true`, the producer sets `Content-Type: multipart/related` and
    /// `SOAPAction` headers on outgoing requests.
    ///
    /// /// TODO(CXF-014): full MTOM multipart encoding not yet implemented
    pub mtom_enabled: bool,
    /// Optional override for the attachment Content-Type used when `mtom_enabled`
    /// is `true`.
    pub attachment_content_type: Option<String>,
}

impl CxfEndpointConfig {
    pub fn validate(&self) -> Result<(), camel_component_api::CamelError> {
        if self.address.trim().is_empty() {
            return Err(camel_component_api::CamelError::Config(
                "cxf URI field 'address' must not be empty".into(),
            ));
        }
        if self.wsdl_path.trim().is_empty() {
            return Err(camel_component_api::CamelError::Config(
                "cxf URI field 'wsdl' must not be empty".into(),
            ));
        }
        if self.service_name.trim().is_empty() {
            return Err(camel_component_api::CamelError::Config(
                "cxf URI field 'service' must not be empty".into(),
            ));
        }
        if self.port_name.trim().is_empty() {
            return Err(camel_component_api::CamelError::Config(
                "cxf URI field 'port' must not be empty".into(),
            ));
        }
        if matches!(self.timeout_ms, Some(0)) {
            return Err(camel_component_api::CamelError::Config(
                "cxf URI field 'timeout_ms' must be greater than 0".into(),
            ));
        }
        Ok(())
    }

    pub fn from_uri(uri: &str) -> Result<Self, camel_component_api::CamelError> {
        let rest = uri.strip_prefix("cxf://").ok_or_else(|| {
            camel_component_api::CamelError::InvalidUri("expected scheme 'cxf://'".to_string())
        })?;

        let (path, query) = match rest.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (rest, None),
        };

        let address = if path.is_empty() {
            return Err(camel_component_api::CamelError::InvalidUri(
                "cxf URI must include an address after 'cxf://'".into(),
            ));
        } else {
            path.to_string()
        };

        let mut wsdl_path = None;
        let mut service_name = None;
        let mut port_name = None;
        let mut operation = None;
        let mut profile = None;
        let mut timeout_ms = None;
        let mut mtom_enabled = None;
        let mut attachment_content_type = None;
        let mut unknown_params = Vec::new();

        if let Some(q) = query {
            for kv in q.split('&') {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "wsdl" => wsdl_path = Some(v.to_string()),
                        "service" => service_name = Some(v.to_string()),
                        "port" => port_name = Some(v.to_string()),
                        "operation" => operation = Some(v.to_string()),
                        "profile" => profile = Some(v.to_string()),
                        "timeout_ms" => {
                            timeout_ms = Some(v.parse::<u64>().map_err(|_| {
                                camel_component_api::CamelError::InvalidUri(
                                    "cxf URI field 'timeout_ms' must be an unsigned integer"
                                        .to_string(),
                                )
                            })?)
                        }
                        "mtom_enabled" => {
                            mtom_enabled = Some(v.parse::<bool>().map_err(|_| {
                                camel_component_api::CamelError::InvalidUri(
                                    "cxf URI field 'mtom_enabled' must be 'true' or 'false'"
                                        .to_string(),
                                )
                            })?)
                        }
                        "attachment_content_type" => {
                            attachment_content_type = Some(v.to_string());
                        }
                        _ => unknown_params.push(k.to_string()),
                    }
                }
            }
        }

        if !unknown_params.is_empty() {
            return Err(camel_component_api::CamelError::InvalidUri(format!(
                "unknown cxf URI query parameter(s): {}",
                unknown_params.join(", ")
            )));
        }

        let wsdl_path = wsdl_path.ok_or_else(|| {
            camel_component_api::CamelError::InvalidUri(
                "cxf URI requires 'wsdl' query parameter".to_string(),
            )
        })?;
        let service_name = service_name.ok_or_else(|| {
            camel_component_api::CamelError::InvalidUri(
                "cxf URI requires 'service' query parameter".to_string(),
            )
        })?;
        let port_name = port_name.ok_or_else(|| {
            camel_component_api::CamelError::InvalidUri(
                "cxf URI requires 'port' query parameter".to_string(),
            )
        })?;

        let cfg = CxfEndpointConfig {
            address,
            wsdl_path,
            service_name,
            port_name,
            operation,
            profile,
            timeout_ms,
            mtom_enabled: mtom_enabled.unwrap_or(false),
            attachment_content_type,
        };
        cfg.validate()?;
        Ok(cfg)
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
        assert_eq!(cfg.timeout_ms, None);
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
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.max_bridges, 4);
        assert!(cfg.profiles.is_empty());
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

            [[profiles]]
            name = "myservice"
            address = "http://localhost:9090/ws"
            wsdl_path = "service.wsdl"
            service_name = "MyService"
            port_name = "MyPort"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.max_bridges, 8);
        assert_eq!(cfg.bridge_start_timeout_ms, 60_000);
        assert_eq!(cfg.health_check_interval_ms, 10_000);
        assert_eq!(cfg.profiles.len(), 1);
        assert_eq!(cfg.profiles[0].name, "myservice");
        assert_eq!(
            cfg.profiles[0].address,
            Some("http://localhost:9090/ws".to_string())
        );
    }

    #[test]
    fn parse_pool_config_defaults() {
        let cfg: CxfPoolConfig = toml::from_str("").expect("empty toml");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.max_bridges, 4);
        assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
        assert_eq!(cfg.health_check_interval_ms, 5_000);
        assert!(cfg.profiles.is_empty());
        assert!(cfg.bridge_cache_dir.is_none());
    }

    #[test]
    fn parse_profile_config() {
        let toml_str = r#"
            [[profiles]]
            name = "baleares"
            address = "http://localhost:8080/ws"
            wsdl_path = "/etc/112/baleares/service.wsdl"
            service_name = "{urn:112}EmergencyService"
            port_name = "{urn:112}EmergencyPort"

            [profiles.security]
            keystore_path = "/etc/112/baleares/keystore.jks"
            sig_username = "baleares_cert"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        let p = &cfg.profiles[0];
        assert_eq!(p.name, "baleares");
        assert_eq!(p.wsdl_path, "/etc/112/baleares/service.wsdl");
        assert_eq!(
            p.security.keystore_path,
            Some("/etc/112/baleares/keystore.jks".to_string())
        );
        assert_eq!(p.security.sig_username, Some("baleares_cert".to_string()));
    }

    #[test]
    fn parse_multiple_profiles() {
        let toml_str = r#"
            [[profiles]]
            name = "baleares"
            wsdl_path = "/a.wsdl"
            service_name = "Svc"
            port_name = "Port"

            [profiles.security]
            keystore_path = "/a.jks"
            keystore_password = "pass"

            [[profiles]]
            name = "extremadura"
            wsdl_path = "/b.wsdl"
            service_name = "Svc2"
            port_name = "Port2"

            [profiles.security]
            keystore_path = "/b.jks"
            truststore_path = "/b.ts"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        assert_eq!(cfg.profiles.len(), 2);
        assert_eq!(cfg.profiles[0].name, "baleares");
        assert_eq!(cfg.profiles[1].name, "extremadura");
        assert_eq!(
            cfg.profiles[1].security.truststore_path,
            Some("/b.ts".to_string())
        );
    }

    #[test]
    fn parse_pool_config_with_profiles() {
        let toml_str = r#"
            max_bridges = 2

            [[profiles]]
            name = "test"
            wsdl_path = "/wsdl/hello.wsdl"
            service_name = "HelloService"
            port_name = "HelloPort"
        "#;
        let cfg: CxfPoolConfig = toml::from_str(toml_str).expect("valid toml");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.max_bridges, 2);
        assert_eq!(cfg.profiles.len(), 1);
        assert_eq!(cfg.profiles[0].name, "test");
    }

    #[test]
    fn profile_name_validation_rejects_hyphens() {
        assert!(validate_profile_name("my-profile").is_err());
    }

    #[test]
    fn profile_name_validation_rejects_uppercase() {
        assert!(validate_profile_name("MyProfile").is_err());
    }

    #[test]
    fn profile_name_validation_rejects_spaces() {
        assert!(validate_profile_name("my profile").is_err());
    }

    #[test]
    fn profile_name_validation_rejects_empty() {
        assert!(validate_profile_name("").is_err());
    }

    #[test]
    fn profile_name_validation_accepts_valid() {
        assert!(validate_profile_name("baleares").is_ok());
        assert!(validate_profile_name("my_profile").is_ok());
        assert!(validate_profile_name("profile123").is_ok());
    }

    #[test]
    fn endpoint_config_parses_profile_param() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://host:8080/service?wsdl=file.wsdl&service=Svc&port=Port&profile=baleares",
        )
        .unwrap();
        assert_eq!(cfg.profile, Some("baleares".to_string()));
    }

    #[test]
    fn endpoint_config_profile_default_none() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://host:8080/service?wsdl=file.wsdl&service=Svc&port=Port",
        )
        .unwrap();
        assert!(cfg.profile.is_none());
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
        assert_eq!(cfg.timeout_ms, None);
    }

    #[test]
    fn endpoint_config_from_uri_minimal() {
        let err = CxfEndpointConfig::from_uri("cxf://http://example.com/ws").unwrap_err();
        assert!(err.to_string().contains("requires 'wsdl'"));
    }

    #[test]
    fn test_rejects_zero_timeout() {
        let mut cfg = CxfEndpointConfig {
            address: "http://example.com/ws".to_string(),
            wsdl_path: "file.wsdl".to_string(),
            service_name: "svc".to_string(),
            port_name: "port".to_string(),
            operation: None,
            profile: None,
            timeout_ms: None,
            mtom_enabled: false,
            attachment_content_type: None,
        };
        cfg.timeout_ms = Some(0);
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_json_negative_timeout_fails_deserialization() {
        #[derive(serde::Deserialize)]
        struct TimeoutWrapper {
            timeout_ms: u64,
        }

        let result = serde_json::from_str::<TimeoutWrapper>(r#"{"timeout_ms": -1}"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_empty_required_field() {
        let mut cfg = CxfEndpointConfig {
            address: "http://example.com/ws".to_string(),
            wsdl_path: "file.wsdl".to_string(),
            service_name: "svc".to_string(),
            port_name: "port".to_string(),
            operation: None,
            profile: None,
            timeout_ms: None,
            mtom_enabled: false,
            attachment_content_type: None,
        };
        cfg.address = "".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn endpoint_config_rejects_unknown_query_param() {
        let err = CxfEndpointConfig::from_uri(
            "cxf://http://example.com/ws?wsdl=file.wsdl&service=Svc&port=Port&bogus=1",
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown cxf URI query parameter"));
    }

    #[test]
    fn endpoint_config_parses_timeout_ms() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://example.com/ws?wsdl=file.wsdl&service=Svc&port=Port&timeout_ms=5000",
        )
        .unwrap();
        assert_eq!(cfg.timeout_ms, Some(5000));
    }

    #[test]
    fn profile_config_env_prefix() {
        let profile = CxfProfileConfig {
            name: "baleares".to_string(),
            address: None,
            wsdl_path: "/wsdl/test.wsdl".to_string(),
            service_name: "Svc".to_string(),
            port_name: "Port".to_string(),
            security: Default::default(),
        };
        assert_eq!(profile.env_prefix(), "CXF_PROFILE_BALEARES_");
    }

    #[test]
    fn pool_config_validate_rejects_zero_max_bridges() {
        let err = toml::from_str::<CxfPoolConfig>("max_bridges = 0").unwrap_err();
        assert!(err.to_string().contains("max_bridges"));
    }

    #[test]
    fn pool_config_validate_rejects_zero_bridge_start_timeout() {
        let err = toml::from_str::<CxfPoolConfig>("bridge_start_timeout_ms = 0").unwrap_err();
        assert!(err.to_string().contains("bridge_start_timeout_ms"));
    }

    #[test]
    fn pool_config_validate_rejects_zero_health_check_interval() {
        let err = toml::from_str::<CxfPoolConfig>("health_check_interval_ms = 0").unwrap_err();
        assert!(err.to_string().contains("health_check_interval_ms"));
    }

    #[test]
    fn endpoint_config_mtom_disabled_by_default() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://example.com/ws?wsdl=file.wsdl&service=Svc&port=Port",
        )
        .unwrap();
        assert!(!cfg.mtom_enabled);
        assert!(cfg.attachment_content_type.is_none());
    }

    #[test]
    fn endpoint_config_parses_mtom_enabled() {
        let cfg = CxfEndpointConfig::from_uri(
            "cxf://http://example.com/ws?wsdl=file.wsdl&service=Svc&port=Port&mtom_enabled=true",
        )
        .unwrap();
        assert!(cfg.mtom_enabled);
    }
}
