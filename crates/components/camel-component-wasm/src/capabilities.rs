//! Per-world capability allowlist for WASM host functions.
//!
//! Controls which host operations a WASM guest can invoke based on the
//! plugin kind. Policy worlds (AuthorizationPolicy, SecurityPolicy) get a
//! fully denied set — they cannot call `camel_call`, `camel_poll`,
//! `host_store`, or `host_load`. Processor and Bean worlds get an
//! explicitly-configured scheme allowlist (fail-closed: empty by default).

use std::collections::HashSet;

/// Per-world capability allowlist.
#[derive(Clone, Debug, Default)]
pub struct WasmCapabilities {
    /// URI schemes the guest may call via `camel_call` / `camel_poll`.
    /// Empty set = deny all (fail-closed).
    pub call_schemes: HashSet<String>,
    /// Whether `host_store` / `host_load` are available.
    pub host_kv: bool,
}

impl WasmCapabilities {
    /// Check whether a URI scheme may be called.
    pub fn can_call(&self, scheme: &str) -> bool {
        self.call_schemes.contains(scheme)
    }

    /// Policy world capabilities — everything denied.
    pub fn denied() -> Self {
        Self::default()
    }

    /// Build capabilities from a comma-separated scheme list (e.g. "log,direct").
    /// host_kv defaults to true for Processor/Bean (trusted plugin kinds).
    pub fn from_scheme_list(schemes: &str) -> Self {
        let call_schemes = schemes
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Self {
            call_schemes,
            host_kv: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_capabilities_block_all_calls() {
        let caps = WasmCapabilities::denied();
        assert!(!caps.can_call("log"));
        assert!(!caps.can_call("direct"));
        assert!(!caps.host_kv);
    }

    #[test]
    fn from_scheme_list_grants_listed_schemes() {
        let caps = WasmCapabilities::from_scheme_list("log,direct,file");
        assert!(caps.can_call("log"));
        assert!(caps.can_call("direct"));
        assert!(caps.can_call("file"));
        assert!(!caps.can_call("kafka"));
        assert!(caps.host_kv);
    }

    #[test]
    fn from_scheme_list_empty_is_denied() {
        let caps = WasmCapabilities::from_scheme_list("");
        assert!(caps.call_schemes.is_empty());
        assert!(!caps.can_call("anything"));
    }

    #[test]
    fn from_scheme_list_trims_whitespace() {
        let caps = WasmCapabilities::from_scheme_list(" log , direct ");
        assert!(caps.can_call("log"));
        assert!(caps.can_call("direct"));
    }

    #[test]
    fn default_is_fail_closed() {
        let caps = WasmCapabilities::default();
        assert!(caps.call_schemes.is_empty());
        assert!(!caps.host_kv);
    }
}
