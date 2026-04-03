use std::time::Duration;

use camel_api::CamelError;

#[derive(Debug, Clone, Default)]
pub struct WsConfig {
    pub max_connections: Option<u32>,
    pub max_message_size: Option<u32>,
    pub heartbeat_interval_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    pub connect_timeout_ms: Option<u64>,
    pub response_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone)]
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

impl WsEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parsed = camel_endpoint::parse_uri(uri)
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
                let parsed_port = p.parse::<u16>().unwrap();
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
        if let Some(v) = params
            .get("maxConnections")
            .and_then(|v| v.parse::<u32>().ok())
        {
            cfg.max_connections = v;
        }
        if let Some(v) = params
            .get("maxMessageSize")
            .and_then(|v| v.parse::<u32>().ok())
        {
            cfg.max_message_size = v;
        }
        if let Some(v) = params.get("sendToAll").and_then(|v| v.parse::<bool>().ok()) {
            cfg.send_to_all = v;
        }
        if let Some(v) = params
            .get("heartbeatIntervalMs")
            .and_then(|v| v.parse::<u64>().ok())
        {
            cfg.heartbeat_interval = Duration::from_millis(v);
        }
        if let Some(v) = params
            .get("idleTimeoutMs")
            .and_then(|v| v.parse::<u64>().ok())
        {
            cfg.idle_timeout = Duration::from_millis(v);
        }
        if let Some(v) = params
            .get("connectTimeoutMs")
            .and_then(|v| v.parse::<u64>().ok())
        {
            cfg.connect_timeout = Duration::from_millis(v);
        }
        if let Some(v) = params
            .get("responseTimeoutMs")
            .and_then(|v| v.parse::<u64>().ok())
        {
            cfg.response_timeout = Duration::from_millis(v);
        }
        if let Some(v) = params.get("allowOrigin") {
            cfg.allow_origin = v.to_string();
        }
        if let Some(v) = params.get("tlsCert") {
            cfg.tls_cert = Some(v.to_string());
        }
        if let Some(v) = params.get("tlsKey") {
            cfg.tls_key = Some(v.to_string());
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
