#![allow(dead_code)]

use std::path::PathBuf;

pub const ENV_CAMEL: &str = "CAMEL_CXF_BRIDGE_BINARY_PATH";
pub const ENV_ALIAS: &str = "CXF_BRIDGE_PATH";

pub fn resolve_cxf_bridge_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var(ENV_CAMEL) {
        let path = PathBuf::from(&p);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Ok(p) = std::env::var(ENV_ALIAS) {
        let path = PathBuf::from(&p);
        if path.is_file() {
            unsafe { std::env::set_var(ENV_CAMEL, &path) };
            return Some(path);
        }
    }

    if let Some(root) = find_workspace_root() {
        let candidate = root.join("bridges/cxf/build/native/cxf-bridge");
        if candidate.is_file() {
            unsafe { std::env::set_var(ENV_CAMEL, &candidate) };
            return Some(candidate);
        }
    }

    None
}

pub fn require_cxf_bridge_binary() -> PathBuf {
    resolve_cxf_bridge_binary().unwrap_or_else(|| {
        panic!(
            "cxf-bridge binary not found.\n\
             Build it with:\n  cd bridges/cxf && ./build-native.sh\n\
             Or set {} or {} to the binary path.\n\
             Example: {} bridges/cxf/build/native/cxf-bridge cargo test ...",
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
