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

use camel_bridge::process::{BridgeProcess, BridgeProcessConfig, BrokerType, Redacted};
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
/// Dropping the returned `BridgeProcess` kills the OS process — this is how
/// tests simulate a bridge restart.
pub async fn spawn_jms_bridge(
    binary: &std::path::Path,
    broker_url: &str,
) -> (BridgeProcess, Channel) {
    let config = BridgeProcessConfig::jms(
        binary.to_path_buf(),
        broker_url.to_string(),
        BrokerType::ActiveMq,
        Some("admin".to_string()),
        Some(Redacted::new("admin".to_string())),
        90_000,
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
