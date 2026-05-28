use std::time::Duration;

use camel_component_api::CamelError;

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct WsConfig {
    pub max_connections: Option<u32>,
    pub max_message_size: Option<u32>,
    pub heartbeat_interval_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    pub connect_timeout_ms: Option<u64>,
    pub response_timeout_ms: Option<u64>,
    pub send_timeout_ms: Option<u64>,
    pub binary_payload: Option<bool>,
    pub subprotocols: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct WsEndpointConfig {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub max_connections: u32,
    pub max_message_size: u32,
    pub send_to_all: bool,
    pub heartbeat_interval: Duration,
    pub idle_timeout: Duration,
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    pub allow_origin: String,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub reconnect: bool,
    pub reconnect_max_attempts: u32,
    pub reconnect_delay_ms: u64,
    pub send_timeout: Duration,
    pub binary_payload: bool,
    pub subprotocols: Vec<String>,
}

fn redacted_opt(opt: &Option<String>) -> Option<&'static str> {
    if opt.is_some() { Some("***") } else { None }
}

impl std::fmt::Debug for WsEndpointConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsEndpointConfig")
            .field("scheme", &self.scheme)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("path", &self.path)
            .field("max_connections", &self.max_connections)
            .field("max_message_size", &self.max_message_size)
            .field("send_to_all", &self.send_to_all)
            .field("heartbeat_interval", &self.heartbeat_interval)
            .field("idle_timeout", &self.idle_timeout)
            .field("connect_timeout", &self.connect_timeout)
            .field("response_timeout", &self.response_timeout)
            .field("allow_origin", &self.allow_origin)
            .field("tls_cert", &redacted_opt(&self.tls_cert))
            .field("tls_key", &redacted_opt(&self.tls_key))
            .field("reconnect", &self.reconnect)
            .field("reconnect_max_attempts", &self.reconnect_max_attempts)
            .field("reconnect_delay_ms", &self.reconnect_delay_ms)
            .field("send_timeout", &self.send_timeout)
            .field("binary_payload", &self.binary_payload)
            .field("subprotocols", &self.subprotocols)
            .finish()
    }
}

impl Default for WsEndpointConfig {
    fn default() -> Self {
        Self {
            scheme: "ws".into(),
            host: "0.0.0.0".into(),
            port: 8080,
            path: "/".into(),
            max_connections: 100,
            max_message_size: 65536,
            send_to_all: false,
            heartbeat_interval: Duration::ZERO,
            idle_timeout: Duration::ZERO,
            connect_timeout: Duration::from_secs(10),
            response_timeout: Duration::from_secs(30),
            allow_origin: "*".into(),
            tls_cert: None,
            tls_key: None,
            reconnect: true,
            reconnect_max_attempts: 5,
            reconnect_delay_ms: 1000,
            send_timeout: Duration::from_secs(30),
            binary_payload: false,
            subprotocols: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WsServerConfig {
    pub inner: WsEndpointConfig,
}

#[derive(Debug, Clone)]
pub struct WsClientConfig {
    pub inner: WsEndpointConfig,
}

impl WsConfig {
    /// Validate configuration values.
    ///
    /// Returns an error if any explicitly-set value is invalid (e.g. zero).
    /// `None` values are valid — they mean "use the default / unlimited".
    pub fn validate(&self) -> Result<(), CamelError> {
        if let Some(0) = self.max_connections {
            return Err(CamelError::Config(
                "maxConnections must be >= 1 when specified".into(),
            ));
        }
        if let Some(0) = self.max_message_size {
            return Err(CamelError::Config(
                "maxMessageSize must be >= 1 when specified".into(),
            ));
        }
        Ok(())
    }
}

impl WsEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parsed = camel_component_api::parse_uri(uri)
            .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;

        let scheme = parsed.scheme;
        if scheme != "ws" && scheme != "wss" {
            return Err(CamelError::EndpointCreationFailed(format!(
                "Invalid WebSocket scheme: {scheme}"
            )));
        }

        let host_port_path = parsed.path;
        let host_port_path = host_port_path.strip_prefix("//").unwrap_or(&host_port_path);
        let (host_port, path) = match host_port_path.split_once('/') {
            Some((hp, p)) => (hp, format!("/{p}")),
            None => (host_port_path, "/".to_string()),
        };

        let (host, port) = match host_port.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => {
                let parsed_port = p.parse::<u16>().unwrap(); // allow-unwrap
                (h.to_string(), parsed_port)
            }
            _ => (
                host_port.to_string(),
                if scheme == "wss" { 443 } else { 80 },
            ),
        };

        let mut cfg = Self {
            scheme,
            host: if host.is_empty() {
                "0.0.0.0".to_string()
            } else {
                host
            },
            port,
            path,
            ..Self::default()
        };

        let params = parsed.params;
        // Validate maxConnections >= 1 (WS-015)
        if let Some(raw) = params.get("maxConnections") {
            let v = raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "maxConnections must be an unsigned integer, got '{raw}'"
                ))
            })?;
            if v == 0 {
                return Err(CamelError::InvalidUri("maxConnections must be >= 1".into()));
            }
            cfg.max_connections = v;
        }
        // Validate maxMessageSize > 0 (WS-019)
        if let Some(raw) = params.get("maxMessageSize") {
            let v = raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "maxMessageSize must be an unsigned integer, got '{raw}'"
                ))
            })?;
            if v == 0 {
                return Err(CamelError::InvalidUri("maxMessageSize must be > 0".into()));
            }
            cfg.max_message_size = v;
        }
        if let Some(raw) = params.get("sendToAll") {
            let v = raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "sendToAll must be a boolean ('true' or 'false'), got '{raw}'"
                ))
            })?;
            cfg.send_to_all = v;
        }
        if let Some(raw) = params.get("heartbeatIntervalMs") {
            let v = raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "heartbeatIntervalMs must be an unsigned integer, got '{raw}'"
                ))
            })?;
            cfg.heartbeat_interval = Duration::from_millis(v);
        }
        if let Some(raw) = params.get("idleTimeoutMs") {
            let v = raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "idleTimeoutMs must be an unsigned integer, got '{raw}'"
                ))
            })?;
            cfg.idle_timeout = Duration::from_millis(v);
        }
        if let Some(raw) = params.get("connectTimeoutMs") {
            let v = raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "connectTimeoutMs must be an unsigned integer, got '{raw}'"
                ))
            })?;
            cfg.connect_timeout = Duration::from_millis(v);
        }
        if let Some(raw) = params.get("responseTimeoutMs") {
            let v = raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "responseTimeoutMs must be an unsigned integer, got '{raw}'"
                ))
            })?;
            cfg.response_timeout = Duration::from_millis(v);
        }
        if let Some(v) = params.get("allowOrigin") {
            if v.is_empty() {
                return Err(CamelError::InvalidUri(
                    "allowOrigin must not be empty when specified".into(),
                ));
            }
            cfg.allow_origin = v.to_string();
        }
        if let Some(v) = params.get("tlsCert") {
            cfg.tls_cert = Some(v.to_string());
        }
        if let Some(v) = params.get("tlsKey") {
            cfg.tls_key = Some(v.to_string());
        }
        if let Some(raw) = params.get("reconnect") {
            cfg.reconnect = raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "reconnect must be a boolean ('true' or 'false'), got '{raw}'"
                ))
            })?;
        }
        if let Some(raw) = params.get("reconnectMaxAttempts") {
            cfg.reconnect_max_attempts = raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "reconnectMaxAttempts must be an unsigned integer, got '{raw}'"
                ))
            })?;
        }
        if let Some(raw) = params.get("reconnectDelayMs") {
            cfg.reconnect_delay_ms = raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "reconnectDelayMs must be an unsigned integer, got '{raw}'"
                ))
            })?;
        }
        if let Some(raw) = params.get("sendTimeoutMs") {
            let v = raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "sendTimeoutMs must be an unsigned integer, got '{raw}'"
                ))
            })?;
            cfg.send_timeout = Duration::from_millis(v);
        }
        if let Some(raw) = params.get("binaryPayload") {
            cfg.binary_payload = raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "binaryPayload must be a boolean ('true' or 'false'), got '{raw}'"
                ))
            })?;
        }
        if let Some(raw) = params.get("subprotocols") {
            cfg.subprotocols = raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

        Ok(cfg)
    }

    pub fn server_config(&self) -> WsServerConfig {
        WsServerConfig {
            inner: self.clone(),
        }
    }

    pub fn client_config(&self) -> WsClientConfig {
        WsClientConfig {
            inner: self.clone(),
        }
    }

    pub fn canonical_host(&self) -> String {
        match self.host.as_str() {
            "0.0.0.0" | "localhost" => "127.0.0.1".to_string(),
            h => h.to_string(),
        }
    }
}

#[cfg(test)]
mod config_validation_tests {
    use super::*;

    #[test]
    fn test_rejects_zero_max_connections() {
        let cfg = WsConfig {
            max_connections: Some(0),
            ..WsConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_rejects_zero_max_message_size() {
        let cfg = WsConfig {
            max_message_size: Some(0),
            ..WsConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_accepts_valid_config() {
        let cfg = WsConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_accepts_nonzero_max_connections() {
        let cfg = WsConfig {
            max_connections: Some(50),
            ..WsConfig::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_accepts_nonzero_max_message_size() {
        let cfg = WsConfig {
            max_message_size: Some(1024),
            ..WsConfig::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_from_uri_rejects_invalid_send_to_all() {
        let err = WsEndpointConfig::from_uri("ws://localhost:8080?sendToAll=yes").unwrap_err();
        assert!(err.to_string().contains("sendToAll"));
    }

    #[test]
    fn test_from_uri_rejects_invalid_max_connections_numeric() {
        let err = WsEndpointConfig::from_uri("ws://localhost:8080?maxConnections=abc").unwrap_err();
        assert!(err.to_string().contains("maxConnections"));
    }

    #[test]
    fn test_from_uri_rejects_invalid_max_message_size_numeric() {
        let err = WsEndpointConfig::from_uri("ws://localhost:8080?maxMessageSize=abc").unwrap_err();
        assert!(err.to_string().contains("maxMessageSize"));
    }

    #[test]
    fn test_from_uri_rejects_invalid_heartbeat_interval_numeric() {
        let err =
            WsEndpointConfig::from_uri("ws://localhost:8080?heartbeatIntervalMs=abc").unwrap_err();
        assert!(err.to_string().contains("heartbeatIntervalMs"));
    }

    #[test]
    fn test_from_uri_rejects_invalid_idle_timeout_numeric() {
        let err = WsEndpointConfig::from_uri("ws://localhost:8080?idleTimeoutMs=abc").unwrap_err();
        assert!(err.to_string().contains("idleTimeoutMs"));
    }

    #[test]
    fn test_from_uri_rejects_invalid_connect_timeout_numeric() {
        let err =
            WsEndpointConfig::from_uri("ws://localhost:8080?connectTimeoutMs=abc").unwrap_err();
        assert!(err.to_string().contains("connectTimeoutMs"));
    }

    #[test]
    fn test_from_uri_rejects_invalid_response_timeout_numeric() {
        let err =
            WsEndpointConfig::from_uri("ws://localhost:8080?responseTimeoutMs=abc").unwrap_err();
        assert!(err.to_string().contains("responseTimeoutMs"));
    }

    // WS-017: sendTimeoutMs parsing
    #[test]
    fn test_from_uri_parses_send_timeout_ms() {
        let cfg = WsEndpointConfig::from_uri("ws://localhost:8080?sendTimeoutMs=7500").unwrap();
        assert_eq!(cfg.send_timeout, Duration::from_millis(7500));
    }

    #[test]
    fn test_from_uri_rejects_invalid_send_timeout_ms() {
        let err = WsEndpointConfig::from_uri("ws://localhost:8080?sendTimeoutMs=xyz").unwrap_err();
        assert!(err.to_string().contains("sendTimeoutMs"));
    }

    // WS-018: binaryPayload parsing
    #[test]
    fn test_from_uri_parses_binary_payload_true() {
        let cfg = WsEndpointConfig::from_uri("ws://localhost:8080?binaryPayload=true").unwrap();
        assert!(cfg.binary_payload);
    }

    #[test]
    fn test_from_uri_parses_binary_payload_false() {
        let cfg = WsEndpointConfig::from_uri("ws://localhost:8080?binaryPayload=false").unwrap();
        assert!(!cfg.binary_payload);
    }

    #[test]
    fn test_from_uri_rejects_invalid_binary_payload() {
        let err = WsEndpointConfig::from_uri("ws://localhost:8080?binaryPayload=sure").unwrap_err();
        assert!(err.to_string().contains("binaryPayload"));
    }

    // WS-007: subprotocols parsing
    #[test]
    fn test_from_uri_parses_subprotocols() {
        let cfg =
            WsEndpointConfig::from_uri("ws://localhost:8080?subprotocols=json,protobuf").unwrap();
        assert_eq!(cfg.subprotocols, vec!["json", "protobuf"]);
    }

    #[test]
    fn test_from_uri_subprotocols_trims_whitespace() {
        let cfg = WsEndpointConfig::from_uri("ws://localhost:8080?subprotocols=a, b").unwrap();
        assert_eq!(cfg.subprotocols, vec!["a", "b"]);
    }

    #[test]
    fn test_from_uri_subprotocols_empty_when_not_specified() {
        let cfg = WsEndpointConfig::from_uri("ws://localhost:8080").unwrap();
        assert!(cfg.subprotocols.is_empty());
    }

    #[test]
    fn ws_endpoint_config_debug_redacts_tls() {
        let config = WsEndpointConfig {
            tls_cert: Some("/secret/cert.pem".to_string()),
            tls_key: Some("/secret/key.pem".to_string()),
            ..WsEndpointConfig::default()
        };
        let debug = format!("{:?}", config);
        assert!(
            !debug.contains("/secret/"),
            "TLS paths must be redacted: {debug}"
        );
        assert!(
            debug.contains("tls_cert"),
            "field name should appear: {debug}"
        );
        assert!(
            debug.contains("tls_key"),
            "field name should appear: {debug}"
        );
    }
}
