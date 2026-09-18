//! camel-bench — benchmarks for Camel Rust components.
//!
//! Contains integration benchmarks measuring throughput, latency, and overhead
//! of core pipeline operations: filter/choice/split composition, body coercion,
//! and exchange flow through multi-step pipelines. The `xml_bridge_decompose`
//! bench decomposes xml-bridge per-call latency into health / dispatch /
//! transform / serde phases against a real bridge binary resolved via
//! `CAMEL_XML_BRIDGE_BINARY_PATH` (or the workspace default); set
//! `XMLPERF_EVIDENCE_RUN` to make a missing binary fail loudly.
//!
//! Run with: `cargo bench -p camel-bench`

use std::path::{Path, PathBuf};

/// Default xml-bridge binary location, relative to the workspace root.
const XML_BRIDGE_BINARY_RELATIVE: &str = "bridges/xml/build/native/xml-bridge";
/// Env var override pointing directly at an xml-bridge binary.
const XML_BRIDGE_BINARY_ENV: &str = "CAMEL_XML_BRIDGE_BINARY_PATH";

/// Walk up from `start` looking for the workspace root: the first
/// directory (max 10 hops up) containing both a `[workspace]`-bearing
/// `Cargo.toml` and a `bridges/` sentinel dir. Same walk as
/// `camel-bridge/src/download.rs`.
pub fn find_workspace_root_from_path(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    for _ in 0..10 {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists()
            && std::fs::read_to_string(&cargo_toml)
                .map(|contents| contents.contains("[workspace]"))
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

/// Resolve the xml-bridge binary for the decomposition bench.
///
/// Resolution order:
/// 1. `CAMEL_XML_BRIDGE_BINARY_PATH` env var — explicit path override
///    (used only when the path exists)
/// 2. `{workspace_root}/bridges/xml/build/native/xml-bridge` — local
///    build from `cargo xtask build-xml-bridge` (auto-detected)
///
/// The `Err` message names both the env override and the default path.
pub fn resolve_bridge_binary_from(start: &Path) -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var(XML_BRIDGE_BINARY_ENV) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path);
        }
    }
    let default_path =
        find_workspace_root_from_path(start).map(|root| root.join(XML_BRIDGE_BINARY_RELATIVE));
    if let Some(path) = &default_path
        && path.exists()
    {
        return Ok(path.clone());
    }
    let resolved = default_path.map_or_else(
        || "<no workspace root above the start path>".to_string(),
        |path| path.display().to_string(),
    );
    Err(format!(
        "xml-bridge binary not found: set {XML_BRIDGE_BINARY_ENV} to an existing \
         binary, or build the default {XML_BRIDGE_BINARY_RELATIVE} in the workspace \
         root (resolved: {resolved})"
    ))
}

/// Resolve the xml-bridge binary from this crate's manifest dir.
pub fn resolve_bridge_binary() -> Result<PathBuf, String> {
    resolve_bridge_binary_from(Path::new(env!("CARGO_MANIFEST_DIR")))
}

/// True iff `XMLPERF_EVIDENCE_RUN` is set and non-empty: evidence runs
/// must fail loudly (exit non-zero) when the bridge binary is missing
/// instead of silently skipping.
pub fn xmlperf_evidence_run() -> bool {
    std::env::var("XMLPERF_EVIDENCE_RUN").is_ok_and(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes every test that mutates process env vars so they cannot
    /// race inside a single `--lib` process.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_binary_prefers_env_override() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        let bin_dir = tempfile::tempdir().expect("tempdir");
        let bin_path = bin_dir.path().join("xml-bridge");
        std::fs::write(&bin_path, b"stub").expect("write stub binary");
        let start_dir = tempfile::tempdir().expect("tempdir");

        // SAFETY: env access is serialized by ENV_LOCK across the test process.
        unsafe { std::env::set_var(XML_BRIDGE_BINARY_ENV, &bin_path) };
        let resolved = resolve_bridge_binary_from(start_dir.path());
        // SAFETY: env access is serialized by ENV_LOCK across the test process.
        unsafe { std::env::remove_var(XML_BRIDGE_BINARY_ENV) };

        assert_eq!(resolved.expect("env override must win"), bin_path);
    }

    #[test]
    fn resolve_binary_errors_when_no_binary_found() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        // SAFETY: env access is serialized by ENV_LOCK across the test process.
        unsafe { std::env::remove_var(XML_BRIDGE_BINARY_ENV) };
        let start_dir = tempfile::tempdir().expect("tempdir");

        let resolved = resolve_bridge_binary_from(start_dir.path());

        let message = resolved.expect_err("no env override and no workspace root above start");
        assert!(message.contains(XML_BRIDGE_BINARY_ENV));
        assert!(message.contains(XML_BRIDGE_BINARY_RELATIVE));
    }

    #[test]
    fn evidence_run_flag_reads_env() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        // SAFETY: env access is serialized by ENV_LOCK across the test process.
        unsafe { std::env::set_var("XMLPERF_EVIDENCE_RUN", "1") };
        assert!(xmlperf_evidence_run());
        // SAFETY: env access is serialized by ENV_LOCK across the test process.
        unsafe { std::env::remove_var("XMLPERF_EVIDENCE_RUN") };
        assert!(!xmlperf_evidence_run());
    }
}
