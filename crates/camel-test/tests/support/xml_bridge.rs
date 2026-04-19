#![allow(dead_code)]

/// Support utilities for integration tests that need the xml-bridge binary.
///
/// Resolution order (mirrors `ensure_binary_for_spec` in `camel-bridge`):
///
/// 1. `CAMEL_XML_BRIDGE_BINARY_PATH` — already-set explicit override
/// 2. `XML_BRIDGE_PATH` — convenience alias; forwarded as `CAMEL_XML_BRIDGE_BINARY_PATH`
/// 3. Auto-detect `bridges/xml/build/native/xml-bridge` from workspace root (dev build)
///
/// Tests call [`require_xml_bridge_binary`] which panics with a clear message if no binary
/// is available, making the skip reason obvious in CI logs.
use std::path::PathBuf;

pub const ENV_CAMEL: &str = "CAMEL_XML_BRIDGE_BINARY_PATH";
pub const ENV_ALIAS: &str = "XML_BRIDGE_PATH";

/// Returns the path to the xml-bridge binary, or `None` if unavailable.
///
/// As a side-effect, sets `CAMEL_XML_BRIDGE_BINARY_PATH` so that components
/// started in the same process pick up the binary automatically without any
/// additional wiring.
pub fn resolve_xml_bridge_binary() -> Option<PathBuf> {
    // 1. Already set via CAMEL_XML_BRIDGE_BINARY_PATH
    if let Ok(p) = std::env::var(ENV_CAMEL) {
        let path = PathBuf::from(&p);
        if path.is_file() {
            return Some(path);
        }
    }

    // 2. Convenience alias XML_BRIDGE_PATH
    if let Ok(p) = std::env::var(ENV_ALIAS) {
        let path = PathBuf::from(&p);
        if path.is_file() {
            // SAFETY: test-only single-threaded setup phase.
            unsafe { std::env::set_var(ENV_CAMEL, &path) };
            return Some(path);
        }
    }

    // 3. Auto-detect from workspace root (dev build via build-native.sh)
    if let Some(root) = find_workspace_root() {
        let candidate = root.join("bridges/xml/build/native/xml-bridge");
        if candidate.is_file() {
            unsafe { std::env::set_var(ENV_CAMEL, &candidate) };
            return Some(candidate);
        }
    }

    None
}

/// Ensures the xml-bridge binary is available and returns its path.
///
/// Panics with a clear message if unavailable, so the test fails fast with
/// actionable instructions instead of a cryptic download/connection error.
pub fn require_xml_bridge_binary() -> PathBuf {
    resolve_xml_bridge_binary().unwrap_or_else(|| {
        panic!(
            "xml-bridge binary not found.\n\
             Build it with:\n  cd bridges/xml && ./build-native.sh\n\
             Or set {} or {} to the binary path.\n\
             Example: {} bridges/xml/build/native/xml-bridge cargo test ...",
            ENV_CAMEL, ENV_ALIAS, ENV_ALIAS,
        )
    })
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
