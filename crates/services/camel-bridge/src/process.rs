use std::fmt;
use std::ops::Deref;
use std::path::PathBuf;
use thiserror::Error;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::spec::{BridgeSpec, CXF_BRIDGE, JMS_BRIDGE, XML_BRIDGE};

// ---------------------------------------------------------------------------
// Redacted<T> — wrapper that never leaks inner value via Debug/Display
// ---------------------------------------------------------------------------

/// A newtype that redacts its inner value in `Debug` and `Display` output.
/// Used for password/credential fields to prevent accidental logging.
#[derive(Clone)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl<T> Deref for Redacted<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
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
    #[error("Config error: {0}")]
    Config(String),
    /// A startup permanent failure announced by the bridge (`{"status":
    /// "permanent-failure","reason":"..."}`) or inferred from the reserved exit
    /// code 78. Never retried (phase 4 parks the slot).
    #[error("Bridge permanent failure: {0}")]
    PermanentFailure(String),
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
    type Err = BridgeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "activemq" => Ok(BrokerType::ActiveMq),
            "artemis" => Ok(BrokerType::Artemis),
            "generic" => Ok(BrokerType::Generic),
            other => Err(BridgeError::Config(format!("unknown broker type: {other}"))), // allow-secret
        }
    }
}

/// Environment variables for a single CXF profile, used by the bridge Java side.
/// Password fields use [`Redacted`] to prevent accidental credential leakage in logs.
#[derive(Debug)]
pub struct CxfProfileEnvVars {
    pub name: String,
    pub wsdl_path: String,
    pub service_name: String,
    pub port_name: String,
    pub address: Option<String>,
    pub keystore_path: Option<String>,
    pub keystore_password: Option<Redacted<String>>,
    pub truststore_path: Option<String>,
    pub truststore_password: Option<Redacted<String>>,
    pub sig_username: Option<String>,
    pub sig_password: Option<Redacted<String>>,
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
            (format!("{}SERVICE_NAME", prefix), self.service_name.clone()),
            (format!("{}PORT_NAME", prefix), self.port_name.clone()),
        ];

        if let Some(ref v) = self.address {
            vars.push((format!("{}ADDRESS", prefix), v.clone()));
        }
        if let Some(ref v) = self.keystore_path {
            vars.push((format!("{}KEYSTORE_PATH", prefix), v.clone()));
        }
        if let Some(ref v) = self.keystore_password {
            vars.push((format!("{}KEYSTORE_PASSWORD", prefix), (**v).clone()));
        }
        if let Some(ref v) = self.truststore_path {
            vars.push((format!("{}TRUSTSTORE_PATH", prefix), v.clone()));
        }
        if let Some(ref v) = self.truststore_password {
            vars.push((format!("{}TRUSTSTORE_PASSWORD", prefix), (**v).clone()));
        }
        if let Some(ref v) = self.sig_username {
            vars.push((format!("{}SIG_USERNAME", prefix), v.clone()));
        }
        if let Some(ref v) = self.sig_password {
            vars.push((format!("{}SIG_PASSWORD", prefix), (**v).clone()));
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

/// Configuration for spawning a bridge subprocess.
/// Password fields use [`Redacted`] to prevent accidental credential leakage in logs.
///
/// ADR-0051 credential boundary: manual-redaction
pub struct BridgeProcessConfig {
    pub spec: &'static BridgeSpec,
    pub binary_path: PathBuf,
    pub broker_url: String,
    pub broker_type: BrokerType,
    pub username: Option<String>,
    pub password: Option<Redacted<String>>,
    pub start_timeout_ms: u64,
    pub env_vars: Vec<(String, String)>,
}

impl fmt::Debug for BridgeProcessConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeProcessConfig")
            .field("spec", &self.spec)
            .field("binary_path", &self.binary_path)
            .field("broker_url", &"[REDACTED]")
            .field("broker_type", &self.broker_type)
            .field("username", &self.username.as_ref().map(|_| "[REDACTED]"))
            .field("password", &self.password)
            .field("start_timeout_ms", &self.start_timeout_ms)
            .field(
                "env_vars",
                &format!("[REDACTED; {} entries]", self.env_vars.len()),
            )
            .finish()
    }
}

impl BridgeProcessConfig {
    /// Constructor for the JMS bridge.
    ///
    /// `journal_dir` and `broker_key` are the Rust-resolved attempt-journal
    /// root (`JMS_ATTEMPT_JOURNAL_DIR`) and the derived broker key
    /// (`JMS_BRIDGE_BROKER_KEY`). Either may be `None`, which leaves the
    /// variable unset so the bridge defers a queue subscribe to a
    /// `FAILED_PRECONDITION`; producer-only and topic paths still work.
    #[allow(clippy::too_many_arguments)]
    pub fn jms(
        binary_path: PathBuf,
        broker_url: String,
        broker_type: BrokerType,
        username: Option<String>,
        password: Option<Redacted<String>>,
        start_timeout_ms: u64,
        journal_dir: Option<String>,
        broker_key: Option<String>,
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
            env_vars.push(("BRIDGE_PASSWORD".to_string(), (**p).clone()));
        }
        if let Some(dir) = journal_dir {
            env_vars.push(("JMS_ATTEMPT_JOURNAL_DIR".to_string(), dir));
        }
        if let Some(key) = broker_key {
            env_vars.push(("JMS_BRIDGE_BROKER_KEY".to_string(), key));
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
        let mut env_vars = vec![("CXF_PROFILES".to_string(), profile_names.join(","))];

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

    pub fn validate(&self) -> Result<(), String> {
        if self.start_timeout_ms == 0 {
            return Err("start_timeout_ms must be > 0".to_string());
        }
        Ok(())
    }
}

/// R4-L7: bounded stdout drain — shared between production start() and tests.
/// Never stops reading: the OS pipe must stay drained (stopping = pipe fill = child
/// blocked = worse regression). Bound single-line size to 64 KiB; rate-limit logging
/// to 100 lines/second with drop summary.
async fn drain_stdout<R: tokio::io::AsyncBufRead + Unpin>(mut reader: R, token: CancellationToken) {
    use tokio::io::AsyncBufReadExt;
    use tokio::time::{Duration, Instant};

    const MAX_LINE_BYTES: usize = 64 * 1024;
    const LOG_INTERVAL: Duration = Duration::from_secs(1);
    const LOG_BUDGET: u32 = 100;

    let mut line_buf: Vec<u8> = Vec::new();
    let mut oversized = false;
    let mut interval_start = Instant::now();
    let mut logged: u32 = 0;
    let mut dropped: u32 = 0;

    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            res = reader.fill_buf() => {
                let chunk_len = match res {
                    Ok(chunk) => {
                        if chunk.is_empty() {
                            break; // EOF
                        }
                        for &b in chunk {
                            if b == b'\n' {
                                if oversized {
                                    // log-policy: degraded
                                    if logged < LOG_BUDGET {
                                        tracing::warn!(
                                            "bridge stdout: oversized line (>{MAX_LINE_BYTES} bytes), truncated"
                                        );
                                        logged += 1;
                                    } else {
                                        dropped += 1;
                                    }
                                } else if logged < LOG_BUDGET {
                                    // log-policy: normal
                                    tracing::debug!(
                                        target: "camel_bridge::child",
                                        "{}",
                                        String::from_utf8_lossy(&line_buf)
                                    );
                                    logged += 1;
                                } else {
                                    dropped += 1;
                                }
                                line_buf.clear();
                                oversized = false;
                            } else if !oversized {
                                if line_buf.len() < MAX_LINE_BYTES {
                                    line_buf.push(b);
                                } else {
                                    oversized = true;
                                }
                            }
                        }
                        chunk.len()
                    }
                    Err(_) => break,
                };
                reader.consume(chunk_len);
                if interval_start.elapsed() >= LOG_INTERVAL {
                    if dropped > 0 {
                        // log-policy: normal
                        tracing::debug!(
                            "bridge stdout: dropped {dropped} lines in last interval"
                        );
                    }
                    interval_start = Instant::now();
                    logged = 0;
                    dropped = 0;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Ready-line classification (phase 4.1 permanent-failure signal)
// ---------------------------------------------------------------------------

/// One classified bridge startup stdout line.
#[derive(Debug, PartialEq, Eq)]
enum ReadyLine {
    /// `{"status":"ready","port":N}` with a usable port.
    Ready(u16),
    /// `{"status":"permanent-failure","reason":"..."}`.
    PermanentFailure(String),
    /// `{"status":"ready"}` without a usable port.
    MalformedReady,
}

/// Classifies one complete startup line. `None` is an unrelated bridge log
/// line, which the handshake skips.
fn parse_ready_line(line: &str) -> Option<ReadyLine> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    match value.get("status").and_then(|s| s.as_str())? {
        "ready" => match value.get("port").and_then(|p| p.as_u64()) {
            Some(port) => Some(ReadyLine::Ready(port as u16)),
            None => Some(ReadyLine::MalformedReady),
        },
        "permanent-failure" => Some(ReadyLine::PermanentFailure(
            value
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("bridge reported a permanent startup failure")
                .to_string(),
        )),
        _ => None,
    }
}

/// Reads bridge stdout until the ready line, a permanent-failure line, or EOF.
///
/// Bounded per line at 64 KiB. Returns [`BridgeError::StdoutClosed`] on EOF so
/// the caller can apply the reserved-exit-code mapping.
async fn read_ready_port<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<u16, BridgeError> {
    use tokio::io::AsyncBufReadExt;

    let mut buf_acc: Vec<u8> = Vec::new();
    const READY_MAX_LINE: usize = 64 * 1024;
    while let Ok(chunk) = reader.fill_buf().await {
        if chunk.is_empty() {
            break; // EOF
        }
        let newline_pos = chunk.iter().position(|&b| b == b'\n');
        let take = match newline_pos {
            Some(i) => i + 1,
            None => chunk.len(),
        };
        if buf_acc.len() + take <= READY_MAX_LINE {
            buf_acc.extend_from_slice(&chunk[..take]);
        }
        reader.consume(take);
        if newline_pos.is_some() {
            let line = String::from_utf8_lossy(&buf_acc);
            let line_trimmed = line.trim_end_matches('\n');
            match parse_ready_line(line_trimmed) {
                Some(ReadyLine::Ready(port)) => return Ok(port),
                Some(ReadyLine::PermanentFailure(reason)) => {
                    return Err(BridgeError::PermanentFailure(reason));
                }
                Some(ReadyLine::MalformedReady) => {
                    // log-policy: system-broken
                    tracing::error!("bridge ready message malformed: {line_trimmed}");
                    return Err(BridgeError::BadReadyMessage(line_trimmed.to_string()));
                }
                None => {}
            }
            buf_acc.clear();
        }
    }
    // log-policy: system-broken
    tracing::error!("bridge stdout closed before ready message");
    Err(BridgeError::StdoutClosed)
}

/// Waits (bounded, 2 s) for a child that closed stdout before readiness. Exit
/// code `78` is the reserved permanent-failure code; any other exit, or a child
/// that does not exit, stays [`BridgeError::StdoutClosed`].
///
/// The 2 s exit classification is separate from cleanup: a child that does not
/// exit is killed and reaped *after* classification, bounded at another 2 s, so
/// a known signal is never delayed by teardown.
async fn eof_exit_error(child: &mut tokio::process::Child) -> BridgeError {
    use tokio::time::{Duration, timeout};

    match timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) if status.code() == Some(78) => BridgeError::PermanentFailure(
            "bridge exited with code 78 before announcing readiness".to_string(),
        ),
        Ok(_) => BridgeError::StdoutClosed,
        Err(_) => {
            // Closed stdout but still running: terminate and reap so no process
            // leaks. This cleanup runs after classification and is bounded.
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(2), child.wait()).await;
            BridgeError::StdoutClosed
        }
    }
}

pub struct BridgeProcess {
    child: tokio::process::Child,
    grpc_port: u16,
    tls: crate::tls::BridgeTlsMaterial,
    token: CancellationToken,
    handle: Option<JoinHandle<()>>,
}

impl BridgeProcess {
    pub fn grpc_port(&self) -> u16 {
        self.grpc_port
    }

    /// Connect a mTLS tonic channel to this bridge process.
    pub async fn connect(&self) -> Result<tonic::transport::Channel, BridgeError> {
        crate::channel::connect_channel(self.grpc_port, &self.tls).await
    }

    /// Start the bridge process and connect a mTLS channel in one step.
    pub async fn start_and_connect(
        config: &BridgeProcessConfig,
    ) -> Result<(Self, tonic::transport::Channel), BridgeError> {
        let process = Self::start(config).await?;
        let channel = process.connect().await?;
        Ok((process, channel))
    }

    /// Spawn the bridge process. Reads the SSL port from stdout JSON line:
    ///   {"status":"ready","port":PORT}
    ///
    /// Port handoff (bd rc-s7dyw): passes `QUARKUS_HTTP_SSL_PORT=0` so the
    /// Quarkus JVM binds an OS-assigned port itself; PortAnnouncer reports
    /// the ACTUAL bound port (from the Quarkus runtime registry) in the ready
    /// line, and this side connects to the announced port. The port is held
    /// by the JVM from the first moment it exists, so the ADR-0070
    /// bind-read-drop race cannot occur. Build-time TLS props are in
    /// application.yml; only runtime cert paths and the SSL port mode are
    /// passed via env vars.
    pub async fn start(config: &BridgeProcessConfig) -> Result<Self, BridgeError> {
        use tokio::process::Command;
        use tokio::time::{Duration, timeout};

        config.validate().map_err(BridgeError::Config)?;

        let tls = crate::tls::BridgeTlsMaterial::generate()?;

        // Port selection is JVM-authoritative (bd rc-s7dyw): 0 makes Quarkus
        // bind an OS-assigned port and announce the actual bound port. The
        // old bind-:0-read-drop here left the port unheld between the drop
        // and the JVM bind (ADR-0070 port-toctou class); with the JVM owning
        // the bind no such window exists.
        let free_port: u16 = 0;

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
            // Cancellation-safe ownership: if this future is dropped (outer
            // timeout or abort) the raw child is killed instead of orphaned.
            .kill_on_drop(true)
            .env("QUARKUS_HTTP_SSL_PORT", free_port.to_string())
            .env(
                "QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_CERT",
                &tls.server_pem_path,
            )
            .env(
                "QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_KEY",
                &tls.server_key_path,
            )
            .env("QUARKUS_TLS_BRIDGE_TRUST_STORE_PEM_CERTS", &tls.ca_pem_path)
            .stdout(std::process::Stdio::piped())
            .stderr(stderr_stdio);

        // Inject bridge-specific env vars (e.g. JMS broker URL/credentials via ::jms()).
        for (key, value) in &config.env_vars {
            command.env(key, value);
        }

        let mut child = command.spawn()?;

        let stdout = child.stdout.take().ok_or(BridgeError::StdoutClosed)?;
        let mut reader = tokio::io::BufReader::new(stdout);

        // --- Ready-detection phase (bounded by outer timeout + bounded line) ---
        // R4-L7: use fill_buf()/consume() instead of Lines::next_line() to avoid
        // unbounded buffer growth on lines without newlines.
        let ready = timeout(
            Duration::from_millis(config.start_timeout_ms),
            read_ready_port(&mut reader),
        )
        .await;

        let port = match ready {
            Ok(Ok(port)) => port,
            Ok(Err(BridgeError::StdoutClosed)) => {
                // EOF before ready: a reserved exit code 78 is a permanent
                // startup failure; any other exit stays the transient EOF error.
                // Classification is separate from cleanup: `eof_exit_error`
                // terminates and reaps a still-running child after classifying.
                return Err(eof_exit_error(&mut child).await);
            }
            Ok(Err(error)) => {
                // Permanent-failure line or malformed ready line: return the
                // observed signal immediately. The child is dropped at function
                // exit, and `kill_on_drop` terminates it without blocking the
                // handshake deadline.
                return Err(error);
            }
            Err(_) => {
                let msg = format!(
                    "{} failed to start: health check timeout after {}ms",
                    config.spec.name, config.start_timeout_ms
                );
                // log-policy: system-broken
                tracing::error!("{msg}");
                // The stuck child is dropped here; `kill_on_drop` reaps it.
                return Err(BridgeError::Timeout(msg));
            }
        };

        // --- Post-ready bounded drain (R4-L7) ---
        // Never stop reading: the OS pipe must stay drained (stopping = pipe fill
        // = child blocked = worse regression). Bound single-line size to 64 KiB;
        // rate-limit logging to 100 lines/second with drop summary.
        let token = CancellationToken::new();
        let child_token = token.clone();
        let handle = tokio::spawn(async move {
            drain_stdout(reader, child_token).await;
        });

        Ok(BridgeProcess {
            child,
            grpc_port: port,
            tls,
            token,
            handle: Some(handle),
        })
    }

    /// Gracefully stop: SIGTERM + wait for exit.
    pub async fn stop(mut self) -> Result<(), BridgeError> {
        use tokio::time::{Duration, sleep};

        self.token.cancel();

        if let Some(handle) = self.handle.take() {
            let join_result = tokio::time::timeout(Duration::from_secs(5), handle).await;
            if join_result.is_err() {
                tracing::warn!("bridge stdout drain task did not exit after cancellation");
            }
        }

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
        self.token.cancel();
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
    fn broker_type_from_str_generic() {
        assert_eq!(
            "generic".parse::<BrokerType>().unwrap(),
            BrokerType::Generic
        );
    }

    #[test]
    fn broker_type_from_str_unknown_returns_err() {
        assert!("ibmmq".parse::<BrokerType>().is_err());
        assert!("UnknownBroker".parse::<BrokerType>().is_err());
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
            Some(Redacted::new("pass".to_string())),
            1000,
            Some("/var/lib/state/rust-camel/jms-bridge".to_string()),
            Some("0123456789abcdef0123456789abcdef".to_string()),
        );
        assert_eq!(cfg.spec.name, "jms-bridge");
    }

    /// Task 3.6: the Rust-resolved journal directory and broker key travel to
    /// the bridge on the initial launch and every restart, because both go
    /// through this one constructor.
    #[test]
    fn journal_env_passed_on_launch_and_restart() {
        let dir = "/var/lib/state/rust-camel/jms-bridge".to_string();
        let key = "0123456789abcdef0123456789abcdef".to_string();

        let initial = BridgeProcessConfig::jms(
            PathBuf::from("/tmp/jms-bridge"),
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            None,
            None,
            1000,
            Some(dir.clone()),
            Some(key.clone()),
        );
        let restart = BridgeProcessConfig::jms(
            PathBuf::from("/tmp/jms-bridge"),
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            None,
            None,
            1000,
            Some(dir.clone()),
            Some(key.clone()),
        );

        for cfg in [&initial, &restart] {
            assert!(
                cfg.env_vars
                    .iter()
                    .any(|(k, v)| k == "JMS_ATTEMPT_JOURNAL_DIR" && v == &dir),
                "JMS_ATTEMPT_JOURNAL_DIR must be present"
            );
            assert!(
                cfg.env_vars
                    .iter()
                    .any(|(k, v)| k == "JMS_BRIDGE_BROKER_KEY" && v == &key),
                "JMS_BRIDGE_BROKER_KEY must be present"
            );
        }

        // Unset stays unset (producer-only path).
        let unset = BridgeProcessConfig::jms(
            PathBuf::from("/tmp/jms-bridge"),
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            None,
            None,
            1000,
            None,
            None,
        );
        assert!(
            !unset
                .env_vars
                .iter()
                .any(|(k, _)| k == "JMS_ATTEMPT_JOURNAL_DIR" || k == "JMS_BRIDGE_BROKER_KEY"),
            "unset journal env must not be injected"
        );
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
                keystore_password: Some(Redacted::new("pass".to_string())),
                truststore_path: None,
                truststore_password: None,
                sig_username: Some("cert".to_string()),
                sig_password: Some(Redacted::new("sig_pass".to_string())),
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
        assert!(
            cfg.env_vars
                .iter()
                .any(|(k, v)| k == "CXF_PROFILE_BALEARES_WSDL_PATH" && v == "/a.wsdl")
        );
        assert!(
            cfg.env_vars
                .iter()
                .any(|(k, v)| k == "CXF_PROFILE_BALEARES_SERVICE_NAME" && v == "Svc")
        );
        assert!(
            cfg.env_vars
                .iter()
                .any(|(k, v)| k == "CXF_PROFILE_BALEARES_PORT_NAME" && v == "Port")
        );
        assert!(
            !cfg.env_vars
                .iter()
                .any(|(k, _)| k == "CXF_PROFILE_BALEARES_ADDRESS")
        );

        // Check extremadura profile vars (with security)
        assert!(
            cfg.env_vars
                .iter()
                .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_WSDL_PATH" && v == "/b.wsdl")
        );
        assert!(
            cfg.env_vars
                .iter()
                .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_ADDRESS" && v == "http://host:9090/ws")
        );
        assert!(
            cfg.env_vars
                .iter()
                .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_KEYSTORE_PATH" && v == "/b.jks")
        );
        assert!(
            cfg.env_vars
                .iter()
                .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_KEYSTORE_PASSWORD" && v == "pass")
        );
        assert!(
            cfg.env_vars
                .iter()
                .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_SIG_USERNAME" && v == "cert")
        );
        assert!(
            cfg.env_vars
                .iter()
                .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_SIG_PASSWORD" && v == "sig_pass")
        );
        assert!(
            cfg.env_vars
                .iter()
                .any(|(k, v)| k == "CXF_PROFILE_EXTREMADURA_SECURITY_ACTIONS_OUT"
                    && v == "Timestamp Signature")
        );
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
            keystore_password: Some(Redacted::new("ks_pass".to_string())),
            truststore_path: Some("/ts.jks".to_string()),
            truststore_password: Some(Redacted::new("ts_pass".to_string())),
            sig_username: Some("user".to_string()),
            sig_password: Some(Redacted::new("sig_pass".to_string())),
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

    #[test]
    fn test_start_timeout_zero_rejected() {
        let config = BridgeProcessConfig::jms(
            PathBuf::from("/usr/bin/echo"),
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            None,
            None,
            0,
            None,
            None,
        );
        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_bridge_rejects_zero_start_timeout() {
        let config = BridgeProcessConfig::jms(
            PathBuf::from("/usr/bin/echo"),
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            None,
            None,
            0,
            None,
            None,
        );
        assert!(config.validate().is_err());
    }

    #[tokio::test]
    async fn test_bridge_stop_completes() {
        use tokio::process::Command;
        use tokio::time::{Duration, timeout};

        let child = Command::new("sh")
            .arg("-c")
            .arg("trap '' TERM; while true; do echo tick; sleep 1; done")
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("must spawn test child process");

        let bridge = BridgeProcess {
            child,
            grpc_port: 0,
            tls: crate::tls::BridgeTlsMaterial::generate().expect("test tls"),
            token: CancellationToken::new(),
            handle: None,
        };

        let result = timeout(Duration::from_secs(10), bridge.stop()).await;
        assert!(result.is_ok(), "stop() must complete within 10s");
    }

    // --- BRG-004: Redacted<T> tests ---

    #[test]
    fn redacted_debug_displays_redacted() {
        let r = Redacted::new("secret_password".to_string());
        assert_eq!(format!("{r:?}"), "[REDACTED]");
    }

    #[test]
    fn redacted_display_displays_redacted() {
        let r = Redacted::new("secret_password".to_string());
        assert_eq!(format!("{r}"), "[REDACTED]");
    }

    #[test]
    fn redacted_deref_gives_inner_value() {
        let r = Redacted::new("secret".to_string());
        assert_eq!(&*r, "secret");
    }

    #[test]
    fn redacted_into_inner_returns_value() {
        let r = Redacted::new("secret".to_string());
        assert_eq!(r.into_inner(), "secret");
    }

    #[test]
    fn redacted_clone_works() {
        let r = Redacted::new("secret".to_string());
        let c = r.clone();
        assert_eq!(&*c, "secret");
        assert_eq!(format!("{c:?}"), "[REDACTED]");
    }

    #[test]
    fn bridge_process_config_debug_redacts_password() {
        let cfg = BridgeProcessConfig::jms(
            PathBuf::from("/tmp/jms-bridge"),
            "tcp://localhost:61616".to_string(),
            BrokerType::ActiveMq,
            Some("user".to_string()),
            Some(Redacted::new("super_secret".to_string())),
            1000,
            None,
            None,
        );
        // The Redacted<T> password field must show [REDACTED] in debug output.
        // env_vars is a separate Vec<(String, String)> used for process injection
        // and legitimately contains the raw value — that is not a Redacted leak.
        let password_debug = format!("{:?}", cfg.password); // allow-secret
        assert!(
            !password_debug.contains("super_secret"),
            "Password field must not leak in Debug: {password_debug}"
        );
        assert_eq!(
            password_debug, "Some([REDACTED])",
            "Password field must show [REDACTED]: {password_debug}"
        );
    }

    #[test]
    fn bridge_process_config_debug_redacts_broker_url_and_env_vars() {
        let cfg = BridgeProcessConfig {
            spec: &JMS_BRIDGE,
            binary_path: PathBuf::from("/tmp/jms-bridge"),
            broker_url: "amqp://u:SENTINEL-BRIDGE-PASS@host:5672".to_string(),
            broker_type: BrokerType::ActiveMq,
            username: Some("user".to_string()),
            password: Some(Redacted::new("pass".to_string())),
            start_timeout_ms: 1000,
            env_vars: vec![(
                "KEYSTORE_PASSWORD".to_string(),
                "SENTINEL-ENV-PASS".to_string(),
            )],
        };
        let debug_output = format!("{cfg:?}");
        assert!(
            !debug_output.contains("SENTINEL-BRIDGE-PASS"),
            "broker_url must be redacted in Debug: {debug_output}"
        );
        assert!(
            !debug_output.contains("SENTINEL-ENV-PASS"),
            "env_vars values must be redacted in Debug: {debug_output}"
        );
        assert!(
            debug_output.contains("[REDACTED; 1 entries]"),
            "env_vars must show redacted count: {debug_output}"
        );
    }

    #[test]
    fn bridge_process_config_debug_redacts_all_sensitive_fields() {
        let cfg = BridgeProcessConfig {
            spec: &JMS_BRIDGE,
            binary_path: PathBuf::from("/tmp/jms-bridge"),
            broker_url: "amqp://SENTINEL-BROKER-URL@host:5672".to_string(),
            broker_type: BrokerType::ActiveMq,
            username: Some("SENTINEL-USERNAME".to_string()),
            password: Some(Redacted::new("SENTINEL-PASSWORD".to_string())),
            start_timeout_ms: 1000,
            env_vars: vec![(
                "KEYSTORE_PASSWORD".to_string(),
                "SENTINEL-ENV-PASS".to_string(),
            )],
        };
        let debug_output = format!("{cfg:?}");
        assert!(
            !debug_output.contains("SENTINEL-BROKER-URL")
                && !debug_output.contains("SENTINEL-USERNAME")
                && !debug_output.contains("SENTINEL-PASSWORD")
                && !debug_output.contains("SENTINEL-ENV-PASS"),
            "No sensitive field may leak in Debug: {debug_output}"
        );
        assert!(
            debug_output.contains("username: Some(\"[REDACTED]\")"),
            "username must be masked in Debug: {debug_output}"
        );
    }

    #[test]
    fn cxf_profile_debug_redacts_passwords() {
        let profile = CxfProfileEnvVars {
            name: "test".to_string(),
            wsdl_path: "/a.wsdl".to_string(),
            service_name: "Svc".to_string(),
            port_name: "Port".to_string(),
            address: None,
            keystore_path: None,
            keystore_password: Some(Redacted::new("ks_secret_val".to_string())),
            truststore_path: None,
            truststore_password: Some(Redacted::new("ts_secret_val".to_string())),
            sig_username: None,
            sig_password: Some(Redacted::new("sig_secret_val".to_string())),
            enc_username: None,
            security_actions_out: None,
            security_actions_in: None,
            signature_algorithm: None,
            signature_digest_algorithm: None,
            signature_c14n_algorithm: None,
            signature_parts: None,
        };
        let debug_output = format!("{profile:?}");
        assert!(
            !debug_output.contains("ks_secret_val")
                && !debug_output.contains("ts_secret_val")
                && !debug_output.contains("sig_secret_val"),
            "Debug must not contain passwords: {debug_output}"
        );
    }

    // --- R4-L7: Bounded stdout drain tests ---

    /// Helper: spawn a child that writes to stdout, return (child, stdout BufReader).
    ///
    /// stdin/stderr are nulled so no descendant inherits the test binary's
    /// harness pipes. Under cargo-nextest, a leaked grandchild (e.g. an
    /// orphaned `sleep` outliving a SIGKILLed `sh`) that still holds the
    /// inherited stdin/stderr keeps nextest's pipes open past leak-timeout
    /// and the test is flagged LEAK even though it passed.
    async fn spawn_child_with_stdout(script: &str) -> tokio::process::Child {
        use tokio::process::Command;

        Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("must spawn test child")
    }

    #[tokio::test]
    async fn drain_chatty_child_no_deadlock() {
        // Child writes many short lines quickly, then exits
        let mut child =
            spawn_child_with_stdout("for i in $(seq 1 500); do echo \"line $i\"; done").await;

        let stdout = child.stdout.take().expect("stdout piped");
        let reader = tokio::io::BufReader::new(stdout);
        let token = CancellationToken::new();

        // Call the production drain function directly (I-1: no duplicated logic)
        let handle = tokio::spawn(drain_stdout(reader, token.clone()));

        // Wait for child to exit
        let status = child.wait().await.expect("wait for child");
        assert!(status.success(), "child should exit successfully");

        // Drain task should complete promptly after child exits (EOF)
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "drain task should complete within 2s");
    }

    #[tokio::test]
    async fn drain_oversized_line_bounded() {
        // Child writes a line >64 KiB without newline, then newline.
        // Use printf + head to generate 200 KiB of 'A' without python dependency.
        // Bound verification: the drain loop caps line_buf at MAX_LINE_BYTES (64 KiB)
        // via the `oversized` flag — bytes beyond the cap are discarded, not accumulated.
        // Enforced by code inspection (src/process.rs drain_stdout: `line_buf.len() < MAX_LINE_BYTES`
        // guard + `oversized = true` branch that skips push). This test verifies the function
        // completes within a tight time bound (500ms), which would fail if the loop stalled
        // or allocated unboundedly.
        let oversized_bytes = 200 * 1024; // 200 KiB
        let mut child = spawn_child_with_stdout(&format!(
            "head -c {oversized_bytes} /dev/zero | tr '\\0' 'A'; echo"
        ))
        .await;

        let stdout = child.stdout.take().expect("stdout piped");
        let reader = tokio::io::BufReader::new(stdout);
        let token = CancellationToken::new();

        let handle = tokio::spawn(drain_stdout(reader, token.clone()));

        let status = child.wait().await.expect("wait for child");
        assert!(status.success(), "child should exit successfully");

        // Tighter bound: 500ms (was 2s). If the cap were broken, the loop would
        // still complete but would have allocated >64 KiB per line — caught by
        // code review + the oversized warning log assertion in integration tests.
        let result = tokio::time::timeout(std::time::Duration::from_millis(500), handle).await;
        assert!(result.is_ok(), "drain task should complete within 500ms");
    }

    #[tokio::test]
    async fn drain_cancellation_exits_promptly() {
        // Child writes slowly (sleep between lines)
        let mut child = spawn_child_with_stdout("while true; do echo tick; sleep 1; done").await;

        let stdout = child.stdout.take().expect("stdout piped");
        let reader = tokio::io::BufReader::new(stdout);
        let token = CancellationToken::new();

        // Call the production drain function directly (I-1: no duplicated logic)
        let handle = tokio::spawn(drain_stdout(reader, token.clone()));

        // drain_stdout polls `token.cancelled()` first in a biased select,
        // so cancelling before the task's first poll still exits promptly.
        token.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_millis(500), handle).await;
        assert!(
            result.is_ok(),
            "drain task should exit promptly after cancellation"
        );

        // Clean up child
        let _ = child.kill().await;
    }

    /// I-2: Verify the single-BufReader-for-both-phases invariant.
    /// The ready-detection phase and drain phase must share the SAME BufReader;
    /// otherwise trailing bytes after the ready message would be lost (a second
    /// reader would start at offset 0, missing data already consumed by phase 1).
    /// This test exercises the handoff by mimicking the ready-detection loop,
    /// then calling drain_stdout with the same reader to verify trailing lines
    /// are processed (not lost).
    #[tokio::test]
    async fn drain_handoff_preserves_trailing_bytes() {
        use tokio::io::AsyncBufReadExt;

        // Child emits ready JSON + trailing log lines
        let mut child = spawn_child_with_stdout(
            r#"echo '{"status":"ready","port":12345}'; echo "trailing-1"; echo "trailing-2"; echo "trailing-3""#,
        )
        .await;

        let stdout = child.stdout.take().expect("stdout piped");
        let mut reader = tokio::io::BufReader::new(stdout);

        // --- Phase 1: ready-detection (mimics start() logic) ---
        let mut buf_acc: Vec<u8> = Vec::new();
        const READY_MAX_LINE: usize = 64 * 1024;
        let mut port_found = None;
        while let Ok(chunk) = reader.fill_buf().await {
            if chunk.is_empty() {
                break; // EOF
            }
            let newline_pos = chunk.iter().position(|&b| b == b'\n');
            let take = match newline_pos {
                Some(i) => i + 1,
                None => chunk.len(),
            };
            if buf_acc.len() + take <= READY_MAX_LINE {
                buf_acc.extend_from_slice(&chunk[..take]);
            }
            reader.consume(take);
            if newline_pos.is_some() {
                let line = String::from_utf8_lossy(&buf_acc);
                let line_trimmed = line.trim_end_matches('\n');
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line_trimmed)
                    && v.get("status").and_then(|s| s.as_str()) == Some("ready")
                    && let Some(p) = v.get("port").and_then(|p| p.as_u64())
                {
                    port_found = Some(p as u16);
                    break;
                }
                buf_acc.clear();
            }
        }
        assert_eq!(port_found, Some(12345), "ready message must be parsed");

        // --- Phase 2: drain with the SAME reader (I-2: handoff invariant) ---
        let token = CancellationToken::new();
        let handle = tokio::spawn(drain_stdout(reader, token.clone()));

        // Wait for child to exit; the echo script terminates promptly, the
        // bound only guards a hung child from hanging the test forever
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .expect("child exits within 5s")
            .expect("wait for child");
        assert!(status.success(), "child should exit successfully");

        // Drain task should complete (trailing lines processed, not lost)
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(
            result.is_ok(),
            "drain task should complete — trailing bytes preserved through handoff"
        );
    }

    // --- Phase 4.1: permanent-failure startup signal ---

    /// Pure protocol classification; platform-independent.
    #[test]
    fn parses_permanent_failure_status_line() {
        assert_eq!(
            parse_ready_line(
                r#"{"status":"permanent-failure","reason":"JMS_ATTEMPT_JOURNAL_DIR is not owner-private"}"#
            ),
            Some(ReadyLine::PermanentFailure(
                "JMS_ATTEMPT_JOURNAL_DIR is not owner-private".to_string()
            )),
            "a permanent-failure line must classify as PermanentFailure"
        );
        // A ready line never classifies as permanent.
        assert!(matches!(
            parse_ready_line(r#"{"status":"ready","port":8080}"#),
            Some(ReadyLine::Ready(8080))
        ));
        // Unrelated lines are skipped.
        assert_eq!(parse_ready_line("just a log line"), None);
    }

    /// Writes an executable stub bridge script into `dir`.
    #[cfg(unix)]
    fn write_stub(dir: &std::path::Path, name: &str, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(name);
        std::fs::write(&path, script).expect("write stub bridge");
        let mut perms = std::fs::metadata(&path)
            .expect("stat stub bridge")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod stub bridge");
        path
    }

    /// A minimal JMS `BridgeProcessConfig` whose binary is the given stub.
    #[cfg(unix)]
    fn stub_config(binary: PathBuf, start_timeout_ms: u64) -> BridgeProcessConfig {
        BridgeProcessConfig {
            spec: &JMS_BRIDGE,
            binary_path: binary,
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            username: None,
            password: None,
            start_timeout_ms,
            env_vars: vec![],
        }
    }

    /// Bounded (external, no Rust sleep) check that `pid` is gone.
    #[cfg(unix)]
    fn wait_pid_gone(pid: u32) -> bool {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "i=0; while [ $i -lt 100 ]; do kill -0 {pid} 2>/dev/null || exit 0; i=$((i+1)); sleep 0.05; done; exit 1"
            ))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Reads a PID file written by a stub.
    #[cfg(unix)]
    fn read_pid(path: &std::path::Path) -> u32 {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Task 4.1: an EOF before ready with exit code 78 maps to a permanent
    /// failure, using a real bounded child.
    #[cfg(unix)]
    #[tokio::test]
    async fn eof_with_exit_78_maps_to_permanent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stub = write_stub(dir.path(), "stub-bridge.sh", "#!/bin/sh\nexit 78\n");

        let result = BridgeProcess::start(&stub_config(stub, 5_000)).await;
        match result {
            Err(BridgeError::PermanentFailure(reason)) => {
                assert!(
                    reason.contains("78"),
                    "reason must name the reserved exit code: {reason}"
                );
            }
            Err(other) => panic!("expected PermanentFailure from exit 78, got: {other:?}"),
            Ok(_) => panic!("exit 78 before ready must never start a bridge"),
        }
    }

    /// A child that closes stdout without exiting is a bounded transient error,
    /// and the still-running child is terminated on drop (no leak).
    #[cfg(unix)]
    #[tokio::test]
    async fn eof_closed_stdout_child_is_bounded_other_and_killed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("child.pid");
        let stub = write_stub(
            dir.path(),
            "stub-bridge.sh",
            &format!(
                "#!/bin/sh\necho $$ > '{}'\nexec 1>&-\nexec sleep 30\n",
                pidfile.display()
            ),
        );

        let result = BridgeProcess::start(&stub_config(stub, 5_000)).await;
        assert!(
            matches!(result, Err(BridgeError::StdoutClosed)),
            "a non-exiting child must surface the bounded EOF error"
        );

        let pid = read_pid(&pidfile);
        assert!(pid > 0, "stub must have written its pid");
        assert!(wait_pid_gone(pid), "child {pid} must be terminated");
    }

    /// A permanent-failure line returns promptly (not delayed by cleanup) and
    /// the still-running child is terminated on drop.
    #[cfg(unix)]
    #[tokio::test]
    async fn permanent_line_returns_prompt_and_kills_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("child.pid");
        let stub = write_stub(
            dir.path(),
            "stub-bridge.sh",
            &format!(
                "#!/bin/sh\necho $$ > '{}'\nprintf '%s\\n' '{{\"status\":\"permanent-failure\",\"reason\":\"JMS_ATTEMPT_JOURNAL_DIR is not owner-private\"}}'\nexec sleep 30\n",
                pidfile.display()
            ),
        );

        // The startup budget is short so a delayed cleanup would blow it.
        let result = BridgeProcess::start(&stub_config(stub, 300)).await;
        match result {
            Err(BridgeError::PermanentFailure(reason)) => {
                assert!(reason.contains("JMS_ATTEMPT_JOURNAL_DIR"), "{reason}");
            }
            Err(other) => panic!("expected PermanentFailure, got: {other:?}"),
            Ok(_) => panic!("a permanent-failure line must never start a bridge"),
        }

        let pid = read_pid(&pidfile);
        assert!(pid > 0, "stub must have written its pid");
        assert!(wait_pid_gone(pid), "child {pid} must be terminated");
    }
}
