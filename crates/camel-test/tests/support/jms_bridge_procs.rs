#![allow(dead_code)]

/// Direct bridge-process helpers for tests that must control the JMS bridge
/// process lifecycle (spawn from an explicit binary, kill, respawn) instead
/// of going through the shared `JmsBridgePool`.
///
/// Binary resolution mirrors `camel-bridge` download.rs order (env override,
/// then workspace dev build); the download/cache tiers are skipped because
/// these tests exercise a locally built bridge. Panics with actionable
/// instructions when no binary is available, mirroring `xml_bridge.rs`.
use std::path::PathBuf;

use camel_bridge::process::{BridgeProcess, BridgeProcessConfig, BrokerType, JmsTlsEnv, Redacted};
use tonic::transport::Channel;

pub const ENV_JMS_BRIDGE_BIN: &str = "CAMEL_JMS_BRIDGE_BINARY_PATH";

/// Returns the path to the jms-bridge binary, or `None` if unavailable.
///
/// Resolution mirrors `camel-bridge` download.rs order (env override, then
/// workspace dev build); the download/cache tiers are skipped because these
/// tests exercise a locally built bridge. Pool-backed components resolve the
/// same dev build independently, so this helper deliberately does not mutate
/// the process-global environment — `set_var` during parallel tests is unsafe.
pub fn resolve_jms_bridge_binary() -> Option<PathBuf> {
    // 1. Already set via CAMEL_JMS_BRIDGE_BINARY_PATH
    if let Ok(p) = std::env::var(ENV_JMS_BRIDGE_BIN) {
        let path = PathBuf::from(&p);
        if path.is_file() {
            return Some(path);
        }
    }

    // 2. Auto-detect workspace dev build (cargo xtask build-jms-bridge)
    if let Some(root) = find_workspace_root() {
        let candidate = root.join("bridges/jms/build/native/jms-bridge");
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

/// Ensures the jms-bridge binary is available and returns its path.
///
/// Panics with a clear message if unavailable, so the test fails fast with
/// actionable instructions instead of a cryptic connection error.
pub fn require_jms_bridge_binary() -> PathBuf {
    resolve_jms_bridge_binary().unwrap_or_else(|| {
        panic!(
            "jms-bridge binary not found.\n\
             Build it with:\n  cargo xtask build-jms-bridge\n\
             Or set {} to the binary path.\n\
             Example: {}=bridges/jms/build/native/jms-bridge cargo test ...",
            ENV_JMS_BRIDGE_BIN, ENV_JMS_BRIDGE_BIN,
        )
    })
}

/// Spawns one jms-bridge process against `broker_url` (ActiveMQ Classic,
/// admin/admin — matches the shared testcontainer credentials) and returns
/// the process handle plus a connected mTLS channel.
///
/// Rust owns the attempt-journal directory and broker key. Each spawned
/// process gets its own 0700 temp journal root plus a stable key derived
/// from the broker triple, because a queue subscribe requires both env
/// values and a journal domain allows only one live owning process.
///
/// This per-spawn root is deliberate for the ID-uniqueness and process-
/// coexistence regressions, which never need to inherit counts. It is WRONG
/// for the phase-5 crash proof: a fresh root loses the recorded reservation
/// count and the survivor cannot acquire the same journal. The crash test
/// MUST pin one stable explicit domain instead — set `XDG_STATE_HOME` (the
/// only Rust-side channel; there is no operator override) to a fixed test
/// root for both probe and survivor and keep the broker name, type, and URL
/// identical so both derive the same broker key.
///
/// Dropping the returned `BridgeProcess` kills the OS process — this is how
/// tests simulate a bridge restart.
pub async fn spawn_jms_bridge(
    binary: &std::path::Path,
    broker_url: &str,
) -> (BridgeProcess, Channel) {
    spawn_jms_bridge_configured(binary, broker_url, BrokerType::ActiveMq, "admin", "admin").await
}

/// Spawns one jms-bridge process against `broker_url` for an explicit broker
/// type and credentials, returning the process handle plus a connected mTLS
/// channel.
///
/// This mirrors [`spawn_jms_bridge`] (Classic, admin/admin) but lets the Phase-5
/// crash proof send through an Artemis producer with the dedicated fixture's
/// explicit credentials. Each spawned process gets its own 0700 temp journal
/// root plus a stable key derived from the broker triple, as in the Classic
/// helper; producers never need to inherit counts.
pub async fn spawn_jms_bridge_configured(
    binary: &std::path::Path,
    broker_url: &str,
    broker_type: BrokerType,
    username: &str,
    password: &str,
) -> (BridgeProcess, Channel) {
    spawn_configured_with_tls(binary, broker_url, broker_type, username, password, None).await
}

/// Spawns one jms-bridge process against a broker with a `tls` table (task
/// 9.1): Classic, admin/admin, broker TLS material. The table is emitted as
/// `BRIDGE_BROKER_TLS_*` exactly as the component emits it for a configured
/// broker (`ca_cert_path` pins a CA, a `client_cert_path`/`client_key_path`
/// pair adds mTLS, `server_name` binds identity) — this is the process-level
/// seam the CI-owned broker-TLS smoke drives.
pub async fn spawn_jms_bridge_tls(
    binary: &std::path::Path,
    broker_url: &str,
    tls: &camel_component_jms::config::BrokerTlsConfig,
) -> (BridgeProcess, Channel) {
    let env = JmsTlsEnv {
        ca_cert_path: tls.ca_cert_path.clone(),
        client_cert_path: tls.client_cert_path.clone(),
        client_key_path: tls.client_key_path.clone(),
        server_name: tls.server_name.clone(),
    };
    spawn_configured_with_tls(
        binary,
        broker_url,
        BrokerType::ActiveMq,
        "admin",
        "admin",
        Some(env),
    )
    .await
}

async fn spawn_configured_with_tls(
    binary: &std::path::Path,
    broker_url: &str,
    broker_type: BrokerType,
    username: &str,
    password: &str,
    tls: Option<JmsTlsEnv>,
) -> (BridgeProcess, Channel) {
    let journal_root = tempfile::TempDir::new()
        .expect("create temp attempt-journal root")
        .keep();
    let journal_dir = journal_root.join("journal");
    camel_component_jms::config::ensure_attempt_journal_dir(&journal_dir)
        .expect("create 0700 attempt-journal dir");
    let broker_key = camel_component_jms::config::attempt_journal_broker_key(
        "jms-bridge-test",
        &broker_type,
        broker_url,
    );

    let config = BridgeProcessConfig::jms(
        binary.to_path_buf(),
        broker_url.to_string(),
        broker_type,
        Some(username.to_string()),
        Some(Redacted::new(password.to_string())),
        90_000,
        30_000,
        30_000,
        Some(journal_dir.to_string_lossy().into_owned()),
        Some(broker_key),
        tls,
    );
    BridgeProcess::start_and_connect(&config)
        .await
        .unwrap_or_else(|e| panic!("failed to start jms-bridge process: {e}"))
}

fn find_workspace_root() -> Option<PathBuf> {
    let start = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut current = start;
    for _ in 0..10 {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists()
            && std::fs::read_to_string(&cargo_toml)
                .map(|c| c.contains("[workspace]"))
                .unwrap_or(false)
            && current.join("bridges").exists()
        {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}
