//! Process-global registry of embedded proto assets (mission 350).
//!
//! Sealed artifacts carry their `protoFile` descriptors as verbatim store
//! assets. At boot, `camel-cli` registers every proto-class asset here
//! under its store path (`assets/protos/hello.proto`) and rewrites the
//! endpoint URIs to a reference `camel-embedded:<asset path>`. The
//! in-process proto compiler resolves such references from this registry
//! — no file is written for the proto class, so a sealed artifact boots
//! on a host with no writable temp directory.
//!
//! # Reserved reference prefix
//!
//! `EMBEDDED_REF_PREFIX` (`camel-embedded:`) is reserved: a `protoFile`
//! value that starts with it is resolved against this registry and never
//! against the filesystem. A real file whose name starts with the prefix
//! is therefore shadowed — an accepted reservation, documented here and
//! in the gRPC component's acceptance rule.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

/// Prefix marking an in-memory proto reference: `camel-embedded:<asset
/// path>`. Reserved across the workspace; see the module documentation.
pub const EMBEDDED_REF_PREFIX: &str = "camel-embedded:";

type Registry = HashMap<String, Arc<[u8]>>;

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Registers `bytes` under `name` (the store asset path, e.g.
/// `assets/protos/hello.proto`). A later registration for the same name
/// replaces the earlier bytes. Idempotent and infallible: the registry
/// is populated once per boot from verified store entries.
pub fn register(name: &str, bytes: Vec<u8>) {
    let mut guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    guard.insert(name.to_owned(), Arc::from(bytes.into_boxed_slice()));
}

/// Returns the bytes registered under `name`, or `None` when the name is
/// not registered (the reference then fails closed at resolution).
pub fn get(name: &str) -> Option<Arc<[u8]>> {
    let guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    guard.get(name).cloned()
}

/// True when `name` is registered. Cheaper than [`get`] when the bytes
/// are not needed.
pub fn contains(name: &str) -> bool {
    let guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    guard.contains_key(name)
}

/// Strips [`EMBEDDED_REF_PREFIX`] from a `protoFile` path, returning the
/// registry name (`camel-embedded:assets/p.proto` → `assets/p.proto`).
/// `None` when the path does not carry the prefix.
pub fn strip_ref(proto: &Path) -> Option<&str> {
    proto.to_str()?.strip_prefix(EMBEDDED_REF_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_get_roundtrip_and_replace() {
        let name = "assets/protos/rt.proto";
        register(name, b"syntax = \"proto3\";".to_vec());
        assert!(contains(name));
        assert_eq!(&get(name).expect("registered")[..], b"syntax = \"proto3\";");
        register(name, b"second".to_vec());
        assert_eq!(&get(name).expect("replaced")[..], b"second");
    }

    #[test]
    fn get_unregistered_is_none() {
        assert!(!contains("assets/protos/never-registered.proto"));
        assert!(get("assets/protos/never-registered.proto").is_none());
    }

    #[test]
    fn strip_ref_requires_prefix() {
        assert_eq!(
            strip_ref(Path::new("camel-embedded:assets/p.proto")),
            Some("assets/p.proto")
        );
        assert_eq!(strip_ref(Path::new("assets/p.proto")), None);
        assert_eq!(strip_ref(Path::new("plain.proto")), None);
    }
}
