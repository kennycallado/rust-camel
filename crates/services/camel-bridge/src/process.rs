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

/// Environment variables for a single CXF profile, used by the bridge Java side.
pub struct CxfProfileEnvVars {
    pub name: String,
    pub wsdl_path: String,
    pub service_name: String,
    pub port_name: String,
    pub address: Option<String>,
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

impl CxfProfileEnvVars {
    pub fn to_env_vars(&self) -> Vec<(String, String)> {
        let prefix = format!("CXF_PROFILE_{}_", self.name.to_uppercase());
        let mut vars = vec![
            (format!("{}WSDL_PATH", prefix), self.wsdl_path.clone()),
            (
                format!("{}SERVICE_NAME", prefix),
                self.service_name.clone(),
            ),
            (format!("{}PORT_NAME", prefix), self.port_name.clone()),
        ];

        if let Some(ref v) = self.address {
            vars.push((format!("{}ADDRESS", prefix), v.clone()));
        }
        if let Some(ref v) = self.keystore_path {
            vars.push((format!("{}KEYSTORE_PATH", prefix), v.clone()));
        }
        if let Some(ref v) = self.keystore_password {
            vars.push((format!("{}KEYSTORE_PASSWORD", prefix), v.clone()));
        }
        if let Some(ref v) = self.truststore_path {
            vars.push((format!("{}TRUSTSTORE_PATH", prefix), v.clone()));
        }
        if let Some(ref v) = self.truststore_password {
            vars.push((format!("{}TRUSTSTORE_PASSWORD", prefix), v.clone()));
        }
        if let Some(ref v) = self.sig_username {
            vars.push((format!("{}SIG_USERNAME", prefix), v.clone()));
        }
        if let Some(ref v) = self.sig_password {
            vars.push((format!("{}SIG_PASSWORD", prefix), v.clone()));
        }
        if let Some(ref v) = self.enc_username {
            vars.push((format!("{}ENC_USERNAME", prefix), v.clone()));
        }
        if let Some(ref v) = self.security_actions_out {
            vars.push((format!("{}SECURITY_ACTIONS_OUT", prefix), v.clone()));
        }
        if let Some(ref v) = self.security_actions_in {
            vars.push((format!("{}SECURITY_ACTIONS_IN", prefix), v.clone()));
        }
        if let Some(ref v) = self.signature_algorithm {
            vars.push((format!("{}SIGNATURE_ALGORITHM", prefix), v.clone()));
        }
        if let Some(ref v) = self.signature_digest_algorithm {
            vars.push((format!("{}SIGNATURE_DIGEST_ALGORITHM", prefix), v.clone()));
        }
        if let Some(ref v) = self.signature_c14n_algorithm {
            vars.push((format!("{}SIGNATURE_C14N_ALGORITHM", prefix), v.clone()));
        }
        if let Some(ref v) = self.signature_parts {
            vars.push((format!("{}SIGNATURE_PARTS", prefix), v.clone()));
        }

        vars
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

    /// Constructor for the CXF bridge with multi-profile support.
    /// Generates `CXF_PROFILES=list` env var plus per-profile env vars.
    pub fn cxf_profiles(
        binary_path: PathBuf,
        profiles: &[CxfProfileEnvVars],
        start_timeout_ms: u64,
    ) -> Self {
        let profile_names: Vec<String> = profiles.iter().map(|p| p.name.clone()).collect();
        let mut env_vars = vec![(
            "CXF_PROFILES".to_string(),
            profile_names.join(","),
        )];

        for profile in profiles {
            env_vars.extend(profile.to_env_vars());
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
    fn cxf_profiles_generates_cxf_profiles_env_var() {
        let profiles = vec![
            CxfProfileEnvVars {
                name: "baleares".to_string(),
                wsdl_path: "/a.wsdl".to_string(),
                service_name: "Svc".to_string(),
                port_name: "Port".to_string(),
                address: None,
                keystore_path: None,
                keystore_password: None,
                truststore_path: None,
                truststore_password: None,
                sig_username: None,
                sig_password: None,
                enc_username: None,
                security_actions_out: None,
                security_actions_in: None,
                signature_algorithm: None,
                signature_digest_algorithm: None,
                signature_c14n_algorithm: None,
                signature_parts: None,
            },
            CxfProfileEnvVars {
                name: "extremadura".to_string(),
                wsdl_path: "/b.wsdl".to_string(),
                service_name: "Svc2".to_string(),
                port_name: "Port2".to_string(),
                address: Some("http://host:9090/ws".to_string()),
                keystore_path: Some("/b.jks".to_string()),
                keystore_password: Some("pass".to_string()),
                truststore_path: None,
                truststore_password: None,
                sig_username: Some("cert".to_string()),
                sig_password: Some("sig_pass".to_string()),
                enc_username: None,
                security_actions_out: Some("Timestamp Signature".to_string()),
                security_actions_in: Some("Timestamp Signature".to_string()),
                signature_algorithm: None,
                signature_digest_algorithm: None,
                signature_c14n_algorithm: None,
                signature_parts: None,
            },
        ];

        let cfg =
            BridgeProcessConfig::cxf_profiles(PathBuf::from("/tmp/cxf-bridge"), &profiles, 15_000);

        assert_eq!(cfg.spec.name, "cxf-bridge");
        assert!(cfg.broker_url.is_empty());
        assert_eq!(cfg.broker_type, BrokerType::Generic);
        assert!(cfg.username.is_none());
        assert!(cfg.password.is_none());

        // Find CXF_PROFILES env var
        let profiles_var = cfg
            .env_vars
            .iter()
            .find(|(k, _)| k == "CXF_PROFILES")
            .expect("CXF_PROFILES env var must exist");
        assert_eq!(profiles_var.1, "baleares,extremadura");

        // Check baleares profile vars (no security)
        assert!(cfg
            .env_vars
            .iter()
            .any(|(k, v)| k == "CXF_PROFILE_BALEARES_WSDL_PATH" && v == "/a.wsdl"));
        assert!(cfg
            .env_vars
            .iter()
            .any(|(k, v)| k == "CXF_PROFILE_BALEARES_SERVICE_NAME" && v == "Svc"));
        assert!(cfg
            .env_vars
            .iter()
            .any(|(k, v)| k == "CXF_PROFILE_BALEARES_PORT_NAME" && v == "Port"));
        assert!(!cfg
            .env_vars
            .iter()
            .any(|(k, _)| k == "CXF_PROFILE_BALEARES_ADDRESS"));

        // Check extremadura profile vars (with security)
        assert!(cfg
            .env_vars
            .iter()
            .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_WSDL_PATH" && v == "/b.wsdl"));
        assert!(cfg
            .env_vars
            .iter()
            .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_ADDRESS" && v == "http://host:9090/ws"));
        assert!(cfg
            .env_vars
            .iter()
            .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_KEYSTORE_PATH" && v == "/b.jks"));
        assert!(cfg
            .env_vars
            .iter()
            .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_KEYSTORE_PASSWORD" && v == "pass"));
        assert!(cfg
            .env_vars
            .iter()
            .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_SIG_USERNAME" && v == "cert"));
        assert!(cfg
            .env_vars
            .iter()
            .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_SIG_PASSWORD" && v == "sig_pass"));
        assert!(cfg.env_vars.iter().any(
           |(k, v)| k == "CXF_PROFILE_EXTREMADURA_SECURITY_ACTIONS_OUT"
                && v == "Timestamp Signature"
        ));
    }

    #[test]
    fn cxf_profiles_single_profile_no_security() {
        let profiles = vec![CxfProfileEnvVars {
            name: "test".to_string(),
            wsdl_path: "service.wsdl".to_string(),
            service_name: "{http://example.com}Service".to_string(),
            port_name: "{http://example.com}Port".to_string(),
            address: None,
            keystore_path: None,
            keystore_password: None,
            truststore_path: None,
            truststore_password: None,
            sig_username: None,
            sig_password: None,
            enc_username: None,
            security_actions_out: None,
            security_actions_in: None,
            signature_algorithm: None,
            signature_digest_algorithm: None,
            signature_c14n_algorithm: None,
            signature_parts: None,
        }];

        let cfg =
            BridgeProcessConfig::cxf_profiles(PathBuf::from("/tmp/cxf-bridge"), &profiles, 15_000);

        assert_eq!(cfg.spec.name, "cxf-bridge");
        // CXF_PROFILES + 3 required vars (WSDL_PATH, SERVICE_NAME, PORT_NAME)
        assert_eq!(cfg.env_vars.len(), 4);
        assert_eq!(cfg.env_vars[0].0, "CXF_PROFILES");
        assert_eq!(cfg.env_vars[0].1, "test");
        assert_eq!(cfg.env_vars[1].0, "CXF_PROFILE_TEST_WSDL_PATH");
        assert_eq!(cfg.env_vars[1].1, "service.wsdl");
        assert_eq!(cfg.env_vars[2].0, "CXF_PROFILE_TEST_SERVICE_NAME");
        assert_eq!(cfg.env_vars[2].1, "{http://example.com}Service");
        assert_eq!(cfg.env_vars[3].0, "CXF_PROFILE_TEST_PORT_NAME");
        assert_eq!(cfg.env_vars[3].1, "{http://example.com}Port");
    }

    #[test]
    fn profile_env_vars_to_env_vars_includes_all_fields() {
        let vars = CxfProfileEnvVars {
            name: "full".to_string(),
            wsdl_path: "/wsdl".to_string(),
            service_name: "Svc".to_string(),
            port_name: "Port".to_string(),
            address: Some("http://host:8080".to_string()),
            keystore_path: Some("/ks.jks".to_string()),
            keystore_password: Some("ks_pass".to_string()),
            truststore_path: Some("/ts.jks".to_string()),
            truststore_password: Some("ts_pass".to_string()),
            sig_username: Some("user".to_string()),
            sig_password: Some("sig_pass".to_string()),
            enc_username: None,
            security_actions_out: Some("Timestamp Signature".to_string()),
            security_actions_in: Some("Timestamp".to_string()),
            signature_algorithm: None,
            signature_digest_algorithm: None,
            signature_c14n_algorithm: None,
            signature_parts: None,
        };

        let env = vars.to_env_vars();
        // 3 required + 1 address + 8 security = 12
        assert_eq!(env.len(), 12);

        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"CXF_PROFILE_FULL_WSDL_PATH"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_SERVICE_NAME"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_PORT_NAME"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_ADDRESS"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_KEYSTORE_PATH"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_KEYSTORE_PASSWORD"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_TRUSTSTORE_PATH"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_TRUSTSTORE_PASSWORD"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_SIG_USERNAME"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_SIG_PASSWORD"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_SECURITY_ACTIONS_OUT"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_SECURITY_ACTIONS_IN"));
    }
}
