use std::path::PathBuf;
use thiserror::Error;

use crate::spec::{BridgeSpec, CXF_BRIDGE, JMS_BRIDGE, XML_BRIDGE};

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bridge timed out: {0}")]
    Timeout(String),
    #[error("Bridge stdout closed before ready message")]
    StdoutClosed,
    #[error("Bridge ready message malformed: {0}")]
    BadReadyMessage(String),
    #[error("Download failed: {0}")]
    Download(String),
    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("URL not allowed: {0}")]
    UrlNotAllowed(String),
    #[error("Transport error: {0}")]
    Transport(String),
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrokerType {
    #[serde(alias = "active_mq")]
    ActiveMq,
    Artemis,
    Generic,
}

impl BrokerType {
    pub fn as_env_str(&self) -> &'static str {
        match self {
            BrokerType::ActiveMq => "activemq",
            BrokerType::Artemis => "artemis",
            BrokerType::Generic => "generic",
        }
    }
}

impl std::str::FromStr for BrokerType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "activemq" => BrokerType::ActiveMq,
            "artemis" => BrokerType::Artemis,
            _ => BrokerType::Generic,
        })
    }
}

pub struct BridgeProcessConfig {
    pub spec: &'static BridgeSpec,
    pub binary_path: PathBuf,
    pub broker_url: String,
    pub broker_type: BrokerType,
    pub username: Option<String>,
    pub password: Option<String>,
    pub start_timeout_ms: u64,
    pub env_vars: Vec<(String, String)>,
}

impl BridgeProcessConfig {
    /// Constructor for the JMS bridge.
    pub fn jms(
        binary_path: PathBuf,
        broker_url: String,
        broker_type: BrokerType,
        username: Option<String>,
        password: Option<String>,
        start_timeout_ms: u64,
    ) -> Self {
        let mut env_vars = vec![
            ("BRIDGE_BROKER_URL".to_string(), broker_url.clone()),
            (
                "BRIDGE_BROKER_TYPE".to_string(),
                broker_type.as_env_str().to_string(),
            ),
        ];
        if let Some(u) = &username {
            env_vars.push(("BRIDGE_USERNAME".to_string(), u.clone()));
        }
        if let Some(p) = &password {
            env_vars.push(("BRIDGE_PASSWORD".to_string(), p.clone()));
        }
        Self {
            spec: &JMS_BRIDGE,
            binary_path,
            broker_url,
            broker_type,
            username,
            password,
            start_timeout_ms,
            env_vars,
        }
    }

    /// Constructor for the XML bridge.
    pub fn xml(binary_path: PathBuf, start_timeout_ms: u64) -> Self {
        Self {
            spec: &XML_BRIDGE,
            binary_path,
            broker_url: String::new(),
            broker_type: BrokerType::Generic,
            username: None,
            password: None,
            start_timeout_ms,
            env_vars: vec![],
        }
    }

    /// Constructor for the CXF bridge.
    #[allow(clippy::too_many_arguments)]
    pub fn cxf(
        binary_path: PathBuf,
        wsdl: String,
        service: String,
        port: String,
        security_username: Option<String>,
        security_password: Option<String>,
        keystore_path: Option<String>,
        keystore_password: Option<String>,
        truststore_path: Option<String>,
        truststore_password: Option<String>,
        start_timeout_ms: u64,
    ) -> Self {
        let mut env_vars = vec![
            ("CXF_WSDL_PATH".to_string(), wsdl),
            ("CXF_SERVICE_NAME".to_string(), service),
            ("CXF_PORT_NAME".to_string(), port),
        ];

        if let Some(v) = security_username {
            env_vars.push(("CXF_SECURITY_USERNAME".to_string(), v));
        }
        if let Some(v) = security_password {
            env_vars.push(("CXF_SECURITY_PASSWORD".to_string(), v));
        }
        if let Some(v) = keystore_path {
            env_vars.push(("CXF_KEYSTORE_PATH".to_string(), v));
        }
        if let Some(v) = keystore_password {
            env_vars.push(("CXF_KEYSTORE_PASSWORD".to_string(), v));
        }
        if let Some(v) = truststore_path {
            env_vars.push(("CXF_TRUSTSTORE_PATH".to_string(), v));
        }
        if let Some(v) = truststore_password {
            env_vars.push(("CXF_TRUSTSTORE_PASSWORD".to_string(), v));
        }

        Self {
            spec: &CXF_BRIDGE,
            binary_path,
            broker_url: String::new(),
            broker_type: BrokerType::Generic,
            username: None,
            password: None,
            start_timeout_ms,
            env_vars,
        }
    }
}

pub struct BridgeProcess {
    child: tokio::process::Child,
    grpc_port: u16,
}

impl BridgeProcess {
    pub fn grpc_port(&self) -> u16 {
        self.grpc_port
    }

    /// Spawn the bridge process. Reads the gRPC port from stdout JSON line:
    ///   {"status":"ready","port":PORT}
    ///
    /// Picks a free OS port and passes it to the bridge via `QUARKUS_GRPC_SERVER_PORT`
    /// so Quarkus binds exactly to that port and PortAnnouncer can echo it back.
    pub async fn start(config: &BridgeProcessConfig) -> Result<Self, BridgeError> {
        use tokio::io::AsyncBufReadExt;
        use tokio::process::Command;
        use tokio::time::{Duration, timeout};

        // Bind :0 to let the OS pick a free port, then release so the bridge can use it.
        let free_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            listener.local_addr()?.port()
        };

        // If CAMEL_BRIDGE_LOG_STDERR is set, redirect stderr to a file for debugging.
        let stderr_stdio: std::process::Stdio =
            if let Ok(log_dir) = std::env::var("CAMEL_BRIDGE_LOG_STDERR") {
                let log_filename = config
                    .spec
                    .log_file_template
                    .replace("{pid}", &std::process::id().to_string());
                let log_path = if log_dir.is_empty() {
                    format!("/tmp/{log_filename}")
                } else {
                    format!("{log_dir}/{log_filename}")
                };
                match std::fs::File::create(&log_path) {
                    Ok(f) => {
                        eprintln!("[camel-bridge] stderr → {}", log_path);
                        f.into()
                    }
                    Err(e) => {
                        eprintln!(
                            "[camel-bridge] failed to create log file {}: {}",
                            log_path, e
                        );
                        std::process::Stdio::inherit()
                    }
                }
            } else {
                std::process::Stdio::inherit()
            };

        let mut command = Command::new(&config.binary_path);
        command
            .env("QUARKUS_GRPC_SERVER_PORT", free_port.to_string())
            // Let the OS pick a random HTTP port — we only use gRPC.
            // Without this, Quarkus binds HTTP on 8080 and fails if occupied.
            .env("QUARKUS_HTTP_PORT", "0")
            .stdout(std::process::Stdio::piped())
            .stderr(stderr_stdio);

        // Inject bridge-specific env vars (e.g. JMS broker URL/credentials via ::jms()).
        for (key, value) in &config.env_vars {
            command.env(key, value);
        }

        let mut child = command.spawn()?;

        let stdout = child.stdout.take().ok_or(BridgeError::StdoutClosed)?;
        let mut reader = tokio::io::BufReader::new(stdout).lines();

        let port = timeout(Duration::from_millis(config.start_timeout_ms), async {
            while let Some(line) = reader.next_line().await? {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                    && v.get("status").and_then(|s| s.as_str()) == Some("ready")
                {
                    if let Some(p) = v.get("port").and_then(|p| p.as_u64()) {
                        return Ok(p as u16);
                    }
                    return Err(BridgeError::BadReadyMessage(line));
                }
            }
            Err(BridgeError::StdoutClosed)
        })
        .await
        .map_err(|_| {
            BridgeError::Timeout(format!(
                "{} failed to start: health check timeout after {}ms",
                config.spec.name, config.start_timeout_ms
            ))
        })??;

        // Keep draining the bridge's stdout in the background so the pipe
        // buffer never fills up.  If the pipe blocks, the bridge process
        // blocks on its next stdout write and silently stops responding.
        tokio::spawn(async move {
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!(target: "camel_bridge::child", "{}", line);
            }
        });

        Ok(BridgeProcess {
            child,
            grpc_port: port,
        })
    }

    /// Gracefully stop: SIGTERM + wait for exit.
    pub async fn stop(mut self) -> Result<(), BridgeError> {
        use tokio::time::{Duration, sleep};

        // Send SIGTERM first (graceful shutdown)
        #[cfg(unix)]
        {
            let pid = self.child.id().unwrap_or(0);
            if pid > 0 {
                // SAFETY: libc::kill is called with the child process PID obtained from tokio.
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
            }
        }

        // On non-Unix (Windows), fall through to kill immediately
        #[cfg(not(unix))]
        let _ = self.child.start_kill();

        // Wait up to 5 seconds for graceful exit, then SIGKILL
        tokio::select! {
            result = self.child.wait() => {
                result?;
            }
            _ = sleep(Duration::from_secs(5)) => {
                let _ = self.child.start_kill();
                self.child.wait().await?;
            }
        }
        Ok(())
    }
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        // Best-effort only. Does NOT wait — cannot block in Drop.
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_type_from_str_activemq() {
        assert_eq!(
            "activemq".parse::<BrokerType>().unwrap(),
            BrokerType::ActiveMq
        );
        assert_eq!(
            "ACTIVEMQ".parse::<BrokerType>().unwrap(),
            BrokerType::ActiveMq
        );
    }

    #[test]
    fn broker_type_from_str_artemis() {
        assert_eq!(
            "artemis".parse::<BrokerType>().unwrap(),
            BrokerType::Artemis
        );
    }

    #[test]
    fn broker_type_from_str_unknown_is_generic() {
        assert_eq!("ibmmq".parse::<BrokerType>().unwrap(), BrokerType::Generic);
    }

    #[test]
    fn broker_type_env_str() {
        assert_eq!(BrokerType::ActiveMq.as_env_str(), "activemq");
        assert_eq!(BrokerType::Artemis.as_env_str(), "artemis");
        assert_eq!(BrokerType::Generic.as_env_str(), "generic");
    }

    #[test]
    fn jms_constructor_uses_jms_spec() {
        let cfg = BridgeProcessConfig::jms(
            PathBuf::from("/tmp/jms-bridge"),
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            Some("user".to_string()),
            Some("pass".to_string()),
            1000,
        );
        assert_eq!(cfg.spec.name, "jms-bridge");
    }

    #[test]
    fn xml_constructor_uses_xml_spec() {
        let cfg = BridgeProcessConfig::xml(PathBuf::from("/tmp/xml-bridge"), 1000);
        assert_eq!(cfg.spec.name, "xml-bridge");
    }

    #[test]
    fn cxf_constructor_uses_cxf_spec() {
        let cfg = BridgeProcessConfig::cxf(
            PathBuf::from("/tmp/cxf-bridge"),
            "service.wsdl".into(),
            "{http://example.com}Service".into(),
            "{http://example.com}Port".into(),
            None,
            None,
            None,
            None,
            None,
            None,
            15_000,
        );
        assert_eq!(cfg.spec.name, "cxf-bridge");
        assert!(cfg.broker_url.is_empty());
        assert_eq!(cfg.broker_type, BrokerType::Generic);
        assert!(cfg.username.is_none());
        assert!(cfg.password.is_none());
        assert_eq!(cfg.env_vars.len(), 3);
        assert_eq!(cfg.env_vars[0].0, "CXF_WSDL_PATH");
        assert_eq!(cfg.env_vars[0].1, "service.wsdl");
        assert_eq!(cfg.env_vars[1].0, "CXF_SERVICE_NAME");
        assert_eq!(cfg.env_vars[1].1, "{http://example.com}Service");
        assert_eq!(cfg.env_vars[2].0, "CXF_PORT_NAME");
        assert_eq!(cfg.env_vars[2].1, "{http://example.com}Port");
    }

    #[test]
    fn cxf_constructor_passes_security_env_vars() {
        let cfg = BridgeProcessConfig::cxf(
            PathBuf::from("/tmp/cxf-bridge"),
            "svc.wsdl".into(),
            "Svc".into(),
            "Port".into(),
            Some("alice".into()),
            Some("secret".into()),
            Some("/keys/keystore.jks".into()),
            Some("ksPass".into()),
            Some("/keys/truststore.jks".into()),
            Some("tsPass".into()),
            15_000,
        );
        assert_eq!(cfg.env_vars.len(), 9);
        assert_eq!(cfg.env_vars[3].0, "CXF_SECURITY_USERNAME");
        assert_eq!(cfg.env_vars[3].1, "alice");
        assert_eq!(cfg.env_vars[4].0, "CXF_SECURITY_PASSWORD");
        assert_eq!(cfg.env_vars[4].1, "secret");
        assert_eq!(cfg.env_vars[5].0, "CXF_KEYSTORE_PATH");
        assert_eq!(cfg.env_vars[5].1, "/keys/keystore.jks");
        assert_eq!(cfg.env_vars[6].0, "CXF_KEYSTORE_PASSWORD");
        assert_eq!(cfg.env_vars[6].1, "ksPass");
        assert_eq!(cfg.env_vars[7].0, "CXF_TRUSTSTORE_PATH");
        assert_eq!(cfg.env_vars[7].1, "/keys/truststore.jks");
        assert_eq!(cfg.env_vars[8].0, "CXF_TRUSTSTORE_PASSWORD");
        assert_eq!(cfg.env_vars[8].1, "tsPass");
    }
}
