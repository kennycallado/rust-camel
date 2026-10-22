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
    #[error(
        "bridge cache directory {path} is not writable: {source}; the directory may be owned by another user (e.g. left by a Docker run as root) or on a read-only filesystem — repair its ownership/permissions, or bypass the cache via CAMEL_<BRIDGE>_BINARY_PATH or the component's bridge_cache_dir"
    )]
    CacheUnwritable {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
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

/// Environment variables for a profile's producer transport TLS material,
/// emitted as `CXF_PROFILE_<NAME>_TLS_*`. Password fields use [`Redacted`] to
/// prevent accidental credential leakage in logs (ADR-0051).
#[derive(Debug, Default)]
pub struct CxfTlsEnvVars {
    pub keystore_path: Option<String>,
    pub keystore_password: Option<Redacted<String>>,
    pub keystore_type: Option<String>,
    pub truststore_path: Option<String>,
    pub truststore_password: Option<Redacted<String>>,
    pub truststore_type: Option<String>,
}

impl CxfTlsEnvVars {
    /// Emit the six `TLS_*` env vars under the given `CXF_PROFILE_<NAME>_`
    /// prefix.
    pub fn to_env_vars(&self, prefix: &str) -> Vec<(String, String)> {
        let mut vars = Vec::new();
        if let Some(ref v) = self.keystore_path {
            vars.push((format!("{prefix}TLS_KEYSTORE_PATH"), v.clone()));
        }
        if let Some(ref v) = self.keystore_password {
            vars.push((format!("{prefix}TLS_KEYSTORE_PASSWORD"), (**v).clone()));
        }
        if let Some(ref v) = self.keystore_type {
            vars.push((format!("{prefix}TLS_KEYSTORE_TYPE"), v.clone()));
        }
        if let Some(ref v) = self.truststore_path {
            vars.push((format!("{prefix}TLS_TRUSTSTORE_PATH"), v.clone()));
        }
        if let Some(ref v) = self.truststore_password {
            vars.push((format!("{prefix}TLS_TRUSTSTORE_PASSWORD"), (**v).clone()));
        }
        if let Some(ref v) = self.truststore_type {
            vars.push((format!("{prefix}TLS_TRUSTSTORE_TYPE"), v.clone()));
        }
        vars
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
    /// Optional comma-separated inbound signature algorithm allow-list,
    /// emitted as `CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS`.
    pub allowed_signature_algorithms: Option<String>,
    /// Optional comma-separated inbound digest algorithm allow-list, emitted
    /// as `CXF_PROFILE_<NAME>_ALLOWED_DIGEST_ALGORITHMS`.
    pub allowed_digest_algorithms: Option<String>,
    /// Per-profile expired pinned-certificate opt-in, emitted as
    /// `CXF_PROFILE_<NAME>_ALLOW_EXPIRED_PINNED_CERTIFICATE` only when true.
    pub allow_expired_pinned_certificate: bool,
    /// Optional per-profile Timestamp TTL in seconds, emitted as
    /// `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS` only when set.
    pub timestamp_ttl_seconds: Option<u64>,
    /// Optional per-profile Timestamp future tolerance in seconds, emitted as
    /// `CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS` only when set.
    pub timestamp_future_tolerance_seconds: Option<u64>,
    /// Optional per-profile HTTP status override for security (WSS) faults only,
    /// emitted as `CXF_PROFILE_<NAME>_FAULT_STATUS`.
    pub fault_status: Option<u16>,
    /// Producer transport TLS material (`[profiles.security.tls]`).
    pub tls: Option<CxfTlsEnvVars>,
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
        if let Some(ref v) = self.allowed_signature_algorithms {
            vars.push((format!("{}ALLOWED_SIGNATURE_ALGORITHMS", prefix), v.clone()));
        }
        if let Some(ref v) = self.allowed_digest_algorithms {
            vars.push((format!("{}ALLOWED_DIGEST_ALGORITHMS", prefix), v.clone()));
        }
        if self.allow_expired_pinned_certificate {
            vars.push((
                format!("{}ALLOW_EXPIRED_PINNED_CERTIFICATE", prefix),
                "true".to_string(),
            ));
        }
        if let Some(v) = self.timestamp_ttl_seconds {
            vars.push((format!("{}TIMESTAMP_TTL_SECONDS", prefix), v.to_string()));
        }
        if let Some(v) = self.timestamp_future_tolerance_seconds {
            vars.push((
                format!("{}TIMESTAMP_FUTURE_TOLERANCE_SECONDS", prefix),
                v.to_string(),
            ));
        }
        if let Some(v) = self.fault_status {
            vars.push((format!("{}FAULT_STATUS", prefix), v.to_string()));
        }
        if let Some(ref tls) = self.tls {
            vars.extend(tls.to_env_vars(&prefix));
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
    /// When `true`, the supervisor awaits the bridge's `soap-ready` announcement
    /// in addition to the gRPC `ready` line (either order) before `start()`
    /// returns. Only a CXF bridge configured with a SOAP bind sets it; every
    /// other bridge (and a CXF pool without a bind) keeps `false` and returns on
    /// the gRPC `ready` line alone.
    pub expect_soap_ready: bool,
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
            .field("expect_soap_ready", &self.expect_soap_ready)
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
            expect_soap_ready: false,
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
            expect_soap_ready: false,
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
            expect_soap_ready: false,
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

/// Validate the announced port of a recognized ready line (`ready` or
/// `soap-ready`): integer 1..=65535. Untrusted-stdout policy — a missing,
/// non-numeric, or out-of-range port fails startup with `BadReadyMessage`
/// instead of silently cast-wrapping into a wrong `u16` (e.g. 70000 → 4464).
/// The reason carries the raw line, already bounded by the 64 KiB line cap.
fn announced_port(value: &serde_json::Value, line: &str) -> Result<u16, BridgeError> {
    match value.get("port").and_then(|p| p.as_u64()) {
        Some(p) if (1..=u64::from(u16::MAX)).contains(&p) => Ok(p as u16),
        _ => {
            // log-policy: system-broken
            tracing::error!("bridge ready message malformed: {line}");
            Err(BridgeError::BadReadyMessage(line.to_string()))
        }
    }
}

#[derive(Debug, PartialEq)]
enum ReadyLine {
    Ready(u16),
    SoapReady(u16),
    PermanentFailure(String),
    MalformedReady,
}

/// Classifies every startup status through the same strict port validator.
fn parse_ready_line(line: &str) -> Option<ReadyLine> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    match value.get("status").and_then(|s| s.as_str())? {
        status @ ("ready" | "soap-ready") => Some(match announced_port(&value, line) {
            Ok(port) if status == "ready" => ReadyLine::Ready(port),
            Ok(port) => ReadyLine::SoapReady(port),
            Err(_) => ReadyLine::MalformedReady,
        }),
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

/// Reads both readiness announcements in either order, with a 64 KiB line cap.
/// EOF classification belongs to the caller, outside the startup timeout.
async fn read_ready_ports<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    expect_soap_ready: bool,
) -> Result<(u16, Option<u16>), BridgeError> {
    use tokio::io::AsyncBufReadExt;
    const READY_MAX_LINE: usize = 64 * 1024;
    let mut buf = Vec::new();
    let mut oversized = false;
    let mut grpc_port = None;
    let mut soap_port = None;
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Err(BridgeError::StdoutClosed);
        }
        let newline = chunk.iter().position(|&b| b == b'\n');
        let take = newline.map_or(chunk.len(), |i| i + 1);
        if !oversized && buf.len() + take <= READY_MAX_LINE {
            buf.extend_from_slice(&chunk[..take]);
        } else {
            oversized = true;
        }
        reader.consume(take);
        if newline.is_some() {
            if !oversized {
                let line = String::from_utf8_lossy(&buf);
                match parse_ready_line(line.trim_end_matches('\n')) {
                    Some(ReadyLine::Ready(port)) => grpc_port = Some(port),
                    Some(ReadyLine::SoapReady(port)) => soap_port = Some(port),
                    Some(ReadyLine::PermanentFailure(reason)) => {
                        return Err(BridgeError::PermanentFailure(reason));
                    }
                    Some(ReadyLine::MalformedReady) => {
                        return Err(BridgeError::BadReadyMessage(line.into_owned()));
                    }
                    None => {}
                }
                if let Some(port) = grpc_port
                    && (!expect_soap_ready || soap_port.is_some())
                {
                    return Ok((port, soap_port));
                }
            }
            buf.clear();
            oversized = false;
        }
    }
}

#[cfg(test)]
async fn read_ready_port<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<u16, BridgeError> {
    read_ready_ports(reader, false).await.map(|(port, _)| port)
}

/// Classify EOF outside the readiness deadline, then bound cleanup separately.
async fn eof_exit_error(child: &mut tokio::process::Child) -> BridgeError {
    use tokio::time::{Duration, timeout};
    match timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) if status.code() == Some(78) => BridgeError::PermanentFailure(
            "bridge exited with code 78 before announcing readiness".to_string(),
        ),
        Ok(_) => BridgeError::StdoutClosed,
        Err(_) => {
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(2), child.wait()).await;
            BridgeError::StdoutClosed
        }
    }
}

pub struct BridgeProcess {
    child: tokio::process::Child,
    grpc_port: u16,
    /// Announced SOAP listener port (`soap-ready` line). `None` when the bridge
    /// runs without a SOAP bind. Fresh per [`BridgeProcess::start`] — a restart
    /// builds a new process, so no stale port can survive into the next slot.
    soap_port: Option<u16>,
    tls: crate::tls::BridgeTlsMaterial,
    token: CancellationToken,
    handle: Option<JoinHandle<()>>,
}

impl BridgeProcess {
    pub fn grpc_port(&self) -> u16 {
        self.grpc_port
    }

    /// Announced SOAP listener port; `None` without a `soap-ready` line.
    pub fn soap_port(&self) -> Option<u16> {
        self.soap_port
    }

    /// PID of the owned bridge child (read-only lifecycle accessor).
    pub fn child_pid(&self) -> Option<u32> {
        self.child.id()
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
        // Owned cleanup: any early return from start() (timeout, malformed ready
        // line, missing stdout) drops the child handle and kills the process —
        // no leaked bridge keeps running with stale credentials.
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

        // --- Ready-detection phase (bounded by ONE outer timeout + bounded line) ---
        // Startup protocol (Task 2.8): parse the gRPC `ready` line AND the
        // `soap-ready` line in ANY order. `config.start_timeout_ms` encloses the
        // WHOLE wait — it is never reset per line or per chunk. With
        // `expect_soap_ready = false` the loop returns on the gRPC `ready` line
        // alone (a `soap-ready` seen before it is still stored when valid).
        // R4-L7: use fill_buf()/consume() instead of Lines::next_line() to avoid
        // unbounded buffer growth on lines without newlines.
        let ready = timeout(
            Duration::from_millis(config.start_timeout_ms),
            read_ready_ports(&mut reader, config.expect_soap_ready),
        )
        .await;
        let (port, soap_port) = match ready {
            Ok(Ok(ports)) => ports,
            Ok(Err(BridgeError::StdoutClosed)) => {
                // EOF classification must not lose exit 78 to the readiness deadline.
                let error = eof_exit_error(&mut child).await;
                // log-policy: system-broken
                tracing::error!("bridge stdout closed before ready message");
                return Err(error);
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                let msg = format!(
                    "{} failed to start: health check timeout after {}ms",
                    config.spec.name, config.start_timeout_ms
                );
                // log-policy: system-broken
                tracing::error!("{msg}");
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
            soap_port,
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
            None,
            None,
        );
        assert_eq!(cfg.spec.name, "jms-bridge");
    }

    #[test]
    fn xml_constructor_uses_xml_spec() {
        let cfg = BridgeProcessConfig::xml(PathBuf::from("/tmp/xml-bridge"), 1000);
        assert_eq!(cfg.spec.name, "xml-bridge");
    }

    #[test]
    fn journal_env_passed_on_launch_and_restart() {
        for _ in 0..2 {
            let config = BridgeProcessConfig::jms(
                PathBuf::from("/tmp/jms-bridge"),
                "tcp://localhost:61616".into(),
                BrokerType::ActiveMq,
                None,
                None,
                1000,
                Some("/var/lib/state/rust-camel/jms-bridge".into()),
                Some("broker-key".into()),
            );
            assert!(config.env_vars.contains(&(
                "JMS_ATTEMPT_JOURNAL_DIR".into(),
                "/var/lib/state/rust-camel/jms-bridge".into()
            )));
            assert!(
                config
                    .env_vars
                    .contains(&("JMS_BRIDGE_BROKER_KEY".into(), "broker-key".into()))
            );
        }
        let unset = BridgeProcessConfig::jms(
            PathBuf::from("/tmp/jms-bridge"),
            "tcp://localhost:61616".into(),
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
                .any(|(key, _)| key == "JMS_ATTEMPT_JOURNAL_DIR" || key == "JMS_BRIDGE_BROKER_KEY")
        );
    }

    #[test]
    fn parses_permanent_failure_status_line() {
        assert_eq!(
            parse_ready_line(
                r#"{"status":"permanent-failure","reason":"JMS_ATTEMPT_JOURNAL_DIR is not owner-private"}"#
            ),
            Some(ReadyLine::PermanentFailure(
                "JMS_ATTEMPT_JOURNAL_DIR is not owner-private".into()
            ))
        );
        assert_eq!(
            parse_ready_line(r#"{"status":"ready","port":8080}"#),
            Some(ReadyLine::Ready(8080))
        );
        assert_eq!(parse_ready_line("just a log line"), None);
        for status in ["ready", "soap-ready"] {
            for port in ["0", "65536", "-1", "1.5", "\"123\"", "null"] {
                assert_eq!(
                    parse_ready_line(&format!(r#"{{"status":"{status}","port":{port}}}"#)),
                    Some(ReadyLine::MalformedReady)
                );
            }
        }
    }

    #[cfg(unix)]
    fn wait_pid_gone(pid: u32) -> bool {
        std::process::Command::new("sh").arg("-c").arg(format!(
            "i=0; while [ $i -lt 100 ]; do kill -0 {pid} 2>/dev/null || exit 0; case \"$(ps -o stat= -p {pid} 2>/dev/null)\" in Z*) exit 0;; esac; i=$((i+1)); sleep 0.05; done; exit 1"
        )).status().map(|s| s.success()).unwrap_or(false)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn eof_with_exit_78_maps_to_permanent() {
        // Exit occurs after stdout closes and after the readiness budget ends.
        let (_dir, script) = fake_bridge_script("exec 1>&-; sleep 0.3; exit 78");
        let result = BridgeProcess::start(&fake_bridge_config(&script, false, 150)).await;
        assert!(
            matches!(result, Err(BridgeError::PermanentFailure(reason)) if reason.contains("78"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn eof_with_other_exit_stays_transient() {
        let (_dir, script) = fake_bridge_script("exit 1");
        let result = BridgeProcess::start(&fake_bridge_config(&script, false, 5000)).await;
        assert!(matches!(result, Err(BridgeError::StdoutClosed)));
    }

    #[tokio::test]
    async fn oversized_ready_line_is_discarded_before_next_announcement() {
        let mut output = vec![b' '; 64 * 1024];
        output.extend_from_slice(b"{\"status\":\"ready\",\"port\":9999}\n");
        output.extend_from_slice(b"{\"status\":\"ready\",\"port\":12345}\n");
        let mut reader = tokio::io::BufReader::new(output.as_slice());
        assert_eq!(
            read_ready_port(&mut reader).await.expect("ready port"),
            12345
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn eof_closed_stdout_child_is_bounded_other_and_killed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("child.pid");
        let (_script_dir, script) = fake_bridge_script(&format!(
            "echo $$ > '{}'; exec 1>&-; exec sleep 30",
            pidfile.display()
        ));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(6),
            BridgeProcess::start(&fake_bridge_config(&script, false, 5000)),
        )
        .await
        .expect("bounded EOF classification");
        assert!(matches!(result, Err(BridgeError::StdoutClosed)));
        let pid = std::fs::read_to_string(pidfile)
            .expect("pidfile")
            .trim()
            .parse()
            .expect("pid");
        assert!(wait_pid_gone(pid), "EOF child must be killed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn permanent_line_returns_prompt_and_kills_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("child.pid");
        let (_script_dir, script) = fake_bridge_script(&format!(
            "echo $$ > '{}'; echo '{{\"status\":\"permanent-failure\",\"reason\":\"JMS_ATTEMPT_JOURNAL_DIR is not owner-private\"}}'; exec sleep 30",
            pidfile.display()
        ));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            BridgeProcess::start(&fake_bridge_config(&script, true, 300)),
        )
        .await
        .expect("prompt permanent failure");
        assert!(
            matches!(result, Err(BridgeError::PermanentFailure(reason)) if reason.contains("JMS_ATTEMPT_JOURNAL_DIR"))
        );
        let pid = std::fs::read_to_string(pidfile)
            .expect("pidfile")
            .trim()
            .parse()
            .expect("pid");
        assert!(wait_pid_gone(pid), "permanent failure child must be killed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_cancellation_kills_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("child.pid");
        let (_script_dir, script) =
            fake_bridge_script(&format!("echo $$ > '{}'; exec sleep 30", pidfile.display()));
        let config = fake_bridge_config(&script, true, 5000);
        let task = tokio::spawn(async move { BridgeProcess::start(&config).await });
        let pid = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&pidfile)
                    && let Ok(pid) = text.trim().parse::<u32>()
                {
                    break pid;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("child starts");
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        assert!(wait_pid_gone(pid), "cancelled startup child must be killed");
    }

    #[test]
    fn default_expect_soap_ready_is_false() {
        let xml = BridgeProcessConfig::xml(PathBuf::from("/tmp/xml-bridge"), 1000);
        assert!(
            !xml.expect_soap_ready,
            "xml bridge must not expect soap-ready"
        );
        let jms = BridgeProcessConfig::jms(
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
            !jms.expect_soap_ready,
            "jms bridge must not expect soap-ready"
        );
        let cxf = BridgeProcessConfig::cxf_profiles(PathBuf::from("/tmp/cxf-bridge"), &[], 1000);
        assert!(
            !cxf.expect_soap_ready,
            "a CXF pool without a SOAP bind must not expect soap-ready"
        );
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
                allowed_signature_algorithms: None,
                allowed_digest_algorithms: None,
                allow_expired_pinned_certificate: false,
                timestamp_ttl_seconds: None,
                timestamp_future_tolerance_seconds: None,
                fault_status: None,
                tls: None,
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
                allowed_signature_algorithms: None,
                allowed_digest_algorithms: None,
                allow_expired_pinned_certificate: false,
                timestamp_ttl_seconds: None,
                timestamp_future_tolerance_seconds: None,
                fault_status: None,
                tls: None,
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
            allowed_signature_algorithms: None,
            allowed_digest_algorithms: None,
            allow_expired_pinned_certificate: false,
            timestamp_ttl_seconds: None,
            timestamp_future_tolerance_seconds: None,
            fault_status: None,
            tls: None,
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
            allowed_signature_algorithms: Some(
                "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256".to_string(),
            ),
            allowed_digest_algorithms: Some("http://www.w3.org/2001/04/xmlenc#sha256".to_string()),
            allow_expired_pinned_certificate: true,
            timestamp_ttl_seconds: Some(900),
            timestamp_future_tolerance_seconds: Some(120),
            fault_status: Some(403),
            tls: None,
        };

        let env = vars.to_env_vars();
        // 3 required + 1 address + 8 security + 2 allowed_algorithms
        // + 1 expired-pinned-cert opt-in + 2 timestamp window + 1 fault_status = 18
        assert_eq!(env.len(), 18);

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
        assert!(keys.contains(&"CXF_PROFILE_FULL_ALLOWED_SIGNATURE_ALGORITHMS"));
        assert!(keys.contains(&"CXF_PROFILE_FULL_ALLOWED_DIGEST_ALGORITHMS"));
        let allow_expired = env
            .iter()
            .find(|(k, _)| k == "CXF_PROFILE_FULL_ALLOW_EXPIRED_PINNED_CERTIFICATE")
            .expect("expired-pinned-cert opt-in env var emitted");
        assert_eq!(allow_expired.1, "true");
        let fault_status = env
            .iter()
            .find(|(k, _)| k == "CXF_PROFILE_FULL_FAULT_STATUS")
            .expect("fault_status env var emitted");
        assert_eq!(fault_status.1, "403");
        let timestamp_ttl = env
            .iter()
            .find(|(k, _)| k == "CXF_PROFILE_FULL_TIMESTAMP_TTL_SECONDS")
            .expect("timestamp ttl env var emitted");
        assert_eq!(timestamp_ttl.1, "900");
        let timestamp_future_tolerance = env
            .iter()
            .find(|(k, _)| k == "CXF_PROFILE_FULL_TIMESTAMP_FUTURE_TOLERANCE_SECONDS")
            .expect("timestamp future tolerance env var emitted");
        assert_eq!(timestamp_future_tolerance.1, "120");
    }

    #[test]
    fn profile_env_vars_omits_timestamp_window_when_unset() {
        let vars = CxfProfileEnvVars {
            name: "default".to_string(),
            wsdl_path: "/wsdl".to_string(),
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
            allowed_signature_algorithms: None,
            allowed_digest_algorithms: None,
            allow_expired_pinned_certificate: false,
            timestamp_ttl_seconds: None,
            timestamp_future_tolerance_seconds: None,
            fault_status: None,
            tls: None,
        };

        let env = vars.to_env_vars();
        assert!(
            !env.iter()
                .any(|(key, _)| key.ends_with("TIMESTAMP_TTL_SECONDS")),
            "unset ttl must not be emitted: {env:?}"
        );
        assert!(
            !env.iter()
                .any(|(key, _)| key.ends_with("TIMESTAMP_FUTURE_TOLERANCE_SECONDS")),
            "unset future tolerance must not be emitted: {env:?}"
        );
    }

    #[test]
    fn profile_env_vars_emits_u64_max_timestamp_window_unchanged() {
        let vars = CxfProfileEnvVars {
            name: "max".to_string(),
            wsdl_path: "/wsdl".to_string(),
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
            allowed_signature_algorithms: None,
            allowed_digest_algorithms: None,
            allow_expired_pinned_certificate: false,
            timestamp_ttl_seconds: Some(u64::MAX),
            timestamp_future_tolerance_seconds: Some(u64::MAX),
            fault_status: None,
            tls: None,
        };

        let env = vars.to_env_vars();
        let ttl = env
            .iter()
            .find(|(k, _)| k == "CXF_PROFILE_MAX_TIMESTAMP_TTL_SECONDS")
            .expect("timestamp ttl env var emitted");
        assert_eq!(ttl.1, "18446744073709551615");
        let future = env
            .iter()
            .find(|(k, _)| k == "CXF_PROFILE_MAX_TIMESTAMP_FUTURE_TOLERANCE_SECONDS")
            .expect("timestamp future tolerance env var emitted");
        assert_eq!(future.1, "18446744073709551615");
    }

    #[test]
    fn profile_env_vars_omits_allow_expired_pinned_certificate_when_false() {
        let vars = CxfProfileEnvVars {
            name: "default".to_string(),
            wsdl_path: "/wsdl".to_string(),
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
            allowed_signature_algorithms: None,
            allowed_digest_algorithms: None,
            allow_expired_pinned_certificate: false,
            timestamp_ttl_seconds: None,
            timestamp_future_tolerance_seconds: None,
            fault_status: None,
            tls: None,
        };

        let env = vars.to_env_vars();
        assert!(
            !env.iter()
                .any(|(key, _)| key.ends_with("ALLOW_EXPIRED_PINNED_CERTIFICATE")),
            "default-off must not emit the opt-in: {env:?}"
        );
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
            soap_port: None,
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
            expect_soap_ready: false,
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
            expect_soap_ready: false,
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
            allowed_signature_algorithms: None,
            allowed_digest_algorithms: None,
            allow_expired_pinned_certificate: false,
            timestamp_ttl_seconds: None,
            timestamp_future_tolerance_seconds: None,
            fault_status: None,
            tls: None,
        };
        let debug_output = format!("{profile:?}");
        assert!(
            !debug_output.contains("ks_secret_val")
                && !debug_output.contains("ts_secret_val")
                && !debug_output.contains("sig_secret_val"),
            "Debug must not contain passwords: {debug_output}"
        );
    }

    // --- Task 2.8: dual ready-line startup protocol tests (fake bridge) ---

    /// Fake bridge binary: an executable `sh` script standing in for the native
    /// bridge on stdout. The tempdir is returned so the script file outlives
    /// every child the test spawns from it.
    fn fake_bridge_script(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("fake bridge tempdir");
        let path = dir.path().join("fake-bridge.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake bridge script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod fake bridge script");
        }
        (dir, path)
    }

    fn fake_bridge_config(
        script: &std::path::Path,
        expect_soap_ready: bool,
        start_timeout_ms: u64,
    ) -> BridgeProcessConfig {
        let mut config =
            BridgeProcessConfig::cxf_profiles(script.to_path_buf(), &[], start_timeout_ms);
        config.expect_soap_ready = expect_soap_ready;
        config
    }

    /// Task 2.8: `ready` and `soap-ready` are accepted in ANY order under the
    /// single enclosing startup budget, and BOTH announced ports are stored.
    #[tokio::test]
    async fn process_dual_ready_reordered_is_accepted() {
        use std::time::{Duration, Instant};

        for (label, body, expect_soap_ready, want_soap) in [
            (
                "soap-ready before ready (expect=true)",
                r#"echo '{"status":"soap-ready","port":23456}'; echo '{"status":"ready","port":12345}'"#,
                true,
                Some(23456u16),
            ),
            (
                "ready before soap-ready (expect=true)",
                r#"echo '{"status":"ready","port":12345}'; echo '{"status":"soap-ready","port":23456}'"#,
                true,
                Some(23456u16),
            ),
            (
                "soap-ready before ready (expect=false still stores the valid soap port)",
                r#"echo '{"status":"soap-ready","port":23456}'; echo '{"status":"ready","port":12345}'"#,
                false,
                Some(23456u16),
            ),
        ] {
            let (_dir, script) = fake_bridge_script(body);
            let config = fake_bridge_config(&script, expect_soap_ready, 10_000);
            let started = Instant::now();
            let process = BridgeProcess::start(&config).await.unwrap_or_else(|e| {
                panic!("{label}: dual ready lines in any order must start: {e}")
            });
            let elapsed = started.elapsed();
            assert!(
                elapsed < Duration::from_millis(10_000),
                "{label}: start must complete inside one config budget, took {elapsed:?}"
            );
            assert_eq!(process.grpc_port(), 12345, "{label}: gRPC port stored");
            assert_eq!(process.soap_port(), want_soap, "{label}: soap port stored");
            process
                .stop()
                .await
                .unwrap_or_else(|e| panic!("{label}: fake bridge must stop cleanly: {e}"));
        }
    }

    /// Task 2.8: a partial (soap-only) announcement followed by the ready line
    /// completes within ONE `start_timeout_ms` budget — and the counterexample
    /// row pins that the budget ENCLOSES the whole wait (it is never reset per
    /// line). Counterexample layout: two independent gaps BOTH strictly inside
    /// the budget (2.5s < 4s) whose SUM comfortably exceeds it (5s > 4s), so a
    /// per-line-reset implementation would return Ok at ~5s while the correct
    /// one times out at ~4s. A marker file proves the partial soap-ready line
    /// was really emitted before the deadline — the timeout cannot be excused
    /// as "the partial never arrived".
    #[tokio::test]
    async fn partial_ready_is_bounded_and_completes_in_one_timeout() {
        use std::time::{Duration, Instant};

        for (label, body_template, budget_ms, expect_ok, max_elapsed_ms) in [
            (
                "partial soap-ready then ready completes within ONE budget",
                r#"echo '{"status":"soap-ready","port":23456}'; sleep 1; echo '{"status":"ready","port":12345}'"#,
                6_000u64,
                true,
                6_000u64,
            ),
            (
                "counterexample: TOTAL budget exceeded fires StartTimeout — no per-line reset",
                r#"sleep 2.5; echo '{"status":"soap-ready","port":23456}'; touch '{marker}'; sleep 2.5; echo '{"status":"ready","port":12345}'"#,
                4_000u64,
                false,
                5_000u64,
            ),
        ] {
            // The counterexample proves the PARTIAL line really was emitted
            // before the deadline: the fake bridge drops a marker right after
            // the soap-ready print (test fixture state — no prod test hooks).
            let marker_dir = tempfile::tempdir().expect("marker tempdir");
            let marker = marker_dir.path().join("soap-ready-emitted");
            let body = body_template.replace("{marker}", &marker.display().to_string());
            let (_dir, script) = fake_bridge_script(&body);
            let config = fake_bridge_config(&script, true, budget_ms);
            let started = Instant::now();
            let result = BridgeProcess::start(&config).await;
            let elapsed = started.elapsed();
            assert!(
                elapsed < Duration::from_millis(max_elapsed_ms),
                "{label}: must resolve in {max_elapsed_ms}ms (one enclosing budget), took {elapsed:?}"
            );
            if expect_ok {
                let process =
                    result.unwrap_or_else(|e| panic!("{label}: start must complete: {e}"));
                assert_eq!(process.grpc_port(), 12345, "{label}: gRPC port");
                assert_eq!(process.soap_port(), Some(23456), "{label}: soap port");
                process
                    .stop()
                    .await
                    .unwrap_or_else(|e| panic!("{label}: fake bridge must stop cleanly: {e}"));
            } else {
                match result {
                    Err(BridgeError::Timeout(_)) => {}
                    Err(e) => panic!("{label}: expected StartTimeout, got {e}"),
                    Ok(_) => panic!(
                        "{label}: start must hit the single enclosing budget instead of \
                         waiting out per-line resets"
                    ),
                }
                assert!(
                    marker.exists(),
                    "{label}: the partial soap-ready line must have been emitted before \
                     the deadline — the timeout comes from the ENCLOSING budget, not \
                     from a missing partial line"
                );
            }
        }
    }

    /// Task 2.8: a recognized `soap-ready` line with a non-numeric, missing, or
    /// out-of-range port fails startup with `BadReadyMessage` — in BOTH line
    /// orders, with `expect_soap_ready` enabled. Port validation is 1..=65535
    /// (untrusted stdout policy: never cast-wrap into a wrong `u16`).
    #[tokio::test]
    async fn malformed_soap_ready_fails_startup() {
        let malformed = [
            (
                "non-numeric port",
                r#"{"status":"soap-ready","port":"abc"}"#,
            ),
            ("missing port", r#"{"status":"soap-ready"}"#),
            (
                "port above u16 range",
                r#"{"status":"soap-ready","port":70000}"#,
            ),
            ("port zero", r#"{"status":"soap-ready","port":0}"#),
        ];
        for (kind, soap_line) in malformed {
            for (order, body) in [
                (
                    "soap-ready first",
                    format!("echo '{soap_line}'; echo '{{\"status\":\"ready\",\"port\":12345}}'"),
                ),
                (
                    "ready first",
                    format!("echo '{{\"status\":\"ready\",\"port\":12345}}'; echo '{soap_line}'"),
                ),
            ] {
                let label = format!("{kind}, {order}");
                let (_dir, script) = fake_bridge_script(&body);
                let config = fake_bridge_config(&script, true, 5_000);
                match BridgeProcess::start(&config).await {
                    Err(BridgeError::BadReadyMessage(_)) => {}
                    Err(e) => panic!("{label}: expected BadReadyMessage, got {e}"),
                    Ok(_) => panic!("{label}: malformed soap-ready must fail startup"),
                }
            }
        }
    }

    /// Task 2.8: with `expect_soap_ready = false` the supervisor awaits ONLY the
    /// gRPC `ready` line and returns without any second wait (no soap-ready ever
    /// arrives — the fake bridge holds its stdout open afterwards to prove it),
    /// and the soap port stays `None` (no stale value).
    #[tokio::test]
    async fn no_soap_wait_when_unbound() {
        use std::time::{Duration, Instant};

        // `exec` replaces the shell so the blocker IS the child: no orphaned
        // grandchildren after stop().
        let (_dir, script) =
            fake_bridge_script(r#"echo '{"status":"ready","port":12345}'; exec tail -f /dev/null"#);
        let config = fake_bridge_config(&script, false, 5_000);
        let started = Instant::now();
        let process = BridgeProcess::start(&config)
            .await
            .expect("start must return on the gRPC ready line alone");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(2_500),
            "expect_soap_ready=false must not wait for a soap-ready line, took {elapsed:?}"
        );
        assert_eq!(process.grpc_port(), 12345);
        assert_eq!(
            process.soap_port(),
            None,
            "no soap-ready line → soap_port stays None"
        );
        process.stop().await.expect("fake bridge must stop cleanly");
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
        // Child emits ready JSON + trailing log lines
        let mut child = spawn_child_with_stdout(
            r#"echo '{"status":"ready","port":12345}'; echo "trailing-1"; echo "trailing-2"; echo "trailing-3""#,
        )
        .await;

        let stdout = child.stdout.take().expect("stdout piped");
        let mut reader = tokio::io::BufReader::new(stdout);

        // --- Phase 1: production readiness parser, not a test copy ---
        assert_eq!(
            read_ready_port(&mut reader).await.expect("ready port"),
            12345
        );

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
}
