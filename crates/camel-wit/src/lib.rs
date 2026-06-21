/// WIT source for the `plugin` world (standalone file).
pub const PLUGIN_WIT: &str = include_str!("../wit/camel-plugin.wit");

/// WIT source for the `bean` world (standalone file, same package as PLUGIN_WIT).
pub const BEAN_WIT: &str = include_str!("../wit/camel-bean.wit");

/// Combined WIT package with both `plugin` and `bean` worlds in a single document.
pub const FULL_WIT: &str = include_str!("../wit/camel-all.wit");

// TODO(WIT-006): WIT interface versioning strategy is not yet defined.
// Consider using `@since(version = X.Y.Z)` WIT annotations when supported by
// the wit-bindgen / wasm-component-ld toolchain to enable compatibility checks.

// ── Common content type constants ────────────────────────────────────────

/// MIME type for JSON data.
pub const APPLICATION_JSON: &str = "application/json";

/// MIME type for plain text.
pub const TEXT_PLAIN: &str = "text/plain";

/// MIME type for arbitrary binary data.
pub const APPLICATION_OCTET_STREAM: &str = "application/octet-stream";

/// MIME type for HTML documents.
pub const TEXT_HTML: &str = "text/html";

/// MIME type for XML documents.
pub const APPLICATION_XML: &str = "application/xml";

/// MIME type for URL-encoded form data.
pub const APPLICATION_FORM_URLENCODED: &str = "application/x-www-form-urlencoded";

/// Absolute path to the `wit/` directory bundled with this crate.
///
/// Returns the `wit/` subdirectory under the crate's manifest directory,
/// resolved at **compile time** via `CARGO_MANIFEST_DIR`. This points to the
/// `camel-wit` source directory (local path dep or registry unpack location).
///
/// # Stability
///
/// This path is stable during builds and in development tooling, but is
/// **not a reliable runtime path in redistributed binaries** — after
/// `cargo install`, the source tree is no longer available and callers may
/// see a missing-directory warning.
///
/// # When to use
///
/// Prefer the `*_WIT` string constants (`PLUGIN_WIT`, `BEAN_WIT`, `FULL_WIT`)
/// for embedding WIT content robustly. Use this function only for CLI tooling
/// that needs filesystem access at build/dev time (e.g. `wasm-tools`,
/// `wit-bindgen` CLI invoked from a build script).
pub fn wit_dir() -> &'static std::path::Path {
    std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/wit"))
}

use std::sync::atomic::{AtomicUsize, Ordering};

use camel_api::CamelError;

/// Default maximum number of resources a WIT host may allocate.
const DEFAULT_MAX_RESOURCES: usize = 1000;

/// Host-side resource tracker for WIT-based WASM plugins.
///
/// Enforces a configurable upper bound on the number of concurrently
/// allocated resources to prevent unbounded memory growth in the host.
///
/// Thread-safe: uses a CAS loop on an atomic counter so concurrent
/// `allocate` calls cannot race past the limit.
#[derive(Debug)]
pub struct WitHost {
    max_resources: usize,
    allocation_count: AtomicUsize,
}

impl WitHost {
    /// Creates a new `WitHost` with the default resource limit (1000).
    pub fn new() -> Self {
        Self::with_max_resources(DEFAULT_MAX_RESOURCES)
    }

    /// Creates a new `WitHost` with an explicit maximum resource count.
    pub fn with_max_resources(max: usize) -> Self {
        Self {
            max_resources: max,
            allocation_count: AtomicUsize::new(0),
        }
    }

    /// Allocates a resource slot.
    ///
    /// Returns `Err(CamelError::ProcessorError)` if the resource limit
    /// would be exceeded. Uses a CAS loop to avoid TOCTOU races when
    /// called concurrently from multiple threads.
    pub fn allocate(&self, _name: &str) -> Result<(), CamelError> {
        let mut current = self.allocation_count.load(Ordering::Relaxed);
        loop {
            if current >= self.max_resources {
                return Err(CamelError::ProcessorError("resource limit exceeded".into()));
            }
            match self.allocation_count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
    }

    /// Deallocates a resource slot, freeing capacity for future allocations.
    pub fn deallocate(&self, _name: &str) {
        self.allocation_count.fetch_sub(1, Ordering::AcqRel);
    }

    /// Returns the current number of allocated resources.
    pub fn resource_count(&self) -> usize {
        self.allocation_count.load(Ordering::Acquire)
    }

    /// Returns the configured maximum resource limit.
    pub fn max_resources(&self) -> usize {
        self.max_resources
    }
}

impl Default for WitHost {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wit_host_rejects_beyond_max_resources() {
        let host = WitHost::with_max_resources(3);
        host.allocate("a").unwrap();
        host.allocate("b").unwrap();
        host.allocate("c").unwrap();
        let result = host.allocate("d"); // must fail
        assert!(result.is_err());
    }

    #[test]
    fn test_wit_host_default_limit_is_1000() {
        let host = WitHost::new();
        assert_eq!(host.max_resources(), 1000);
    }

    #[test]
    fn test_wit_host_allows_up_to_limit() {
        let host = WitHost::with_max_resources(2);
        assert!(host.allocate("x").is_ok());
        assert!(host.allocate("y").is_ok());
        assert!(host.allocate("z").is_err());
    }

    #[test]
    fn test_wit_host_deallocate_frees_slot() {
        let host = WitHost::with_max_resources(1);
        host.allocate("a").unwrap();
        assert!(host.allocate("b").is_err());
        host.deallocate("a");
        assert!(host.allocate("b").is_ok());
    }

    #[test]
    fn test_wit_host_resource_count_tracks_allocations() {
        let host = WitHost::new();
        assert_eq!(host.resource_count(), 0);
        host.allocate("a").unwrap();
        host.allocate("b").unwrap();
        assert_eq!(host.resource_count(), 2);
        host.deallocate("a");
        assert_eq!(host.resource_count(), 1);
    }

    #[test]
    fn test_wit_host_error_is_processor_error() {
        let host = WitHost::with_max_resources(1);
        host.allocate("a").unwrap();
        let err = host.allocate("b").unwrap_err();
        assert!(matches!(err, CamelError::ProcessorError(_)));
        assert!(err.to_string().contains("resource limit exceeded"));
    }

    // ── WIT-002: Tests for WIT definitions ──────────────────────────────────

    #[test]
    fn test_wit_dir_exists() {
        let dir = wit_dir();
        assert!(
            dir.exists(),
            "wit_dir() should point to an existing directory"
        );
        assert!(dir.is_dir(), "wit_dir() should be a directory");
    }

    #[test]
    fn test_wit_dir_contains_expected_files() {
        let dir = wit_dir();
        assert!(
            dir.join("camel-plugin.wit").exists(),
            "camel-plugin.wit missing"
        );
        assert!(
            dir.join("camel-bean.wit").exists(),
            "camel-bean.wit missing"
        );
        assert!(dir.join("camel-all.wit").exists(), "camel-all.wit missing");
    }

    #[test]
    fn test_plugin_wit_is_non_empty() {
        assert!(!PLUGIN_WIT.is_empty(), "PLUGIN_WIT should not be empty");
    }

    #[test]
    fn test_bean_wit_is_non_empty() {
        assert!(!BEAN_WIT.is_empty(), "BEAN_WIT should not be empty");
    }

    #[test]
    fn test_full_wit_is_non_empty() {
        assert!(!FULL_WIT.is_empty(), "FULL_WIT should not be empty");
    }

    #[test]
    fn test_wit_constants_contain_package_declaration() {
        assert!(PLUGIN_WIT.contains("package camel:plugin"));
        assert!(BEAN_WIT.contains("package camel:plugin"));
        assert!(FULL_WIT.contains("package camel:plugin"));
    }

    #[test]
    fn test_wit_exchange_has_route_and_message_id_fields() {
        // WIT-005: verify route-id and message-id fields are present
        assert!(
            FULL_WIT.contains("route-id"),
            "wasm-exchange should contain route-id field"
        );
        assert!(
            FULL_WIT.contains("message-id"),
            "wasm-exchange should contain message-id field"
        );
        assert!(
            PLUGIN_WIT.contains("route-id"),
            "plugin WIT should contain route-id field"
        );
        assert!(
            PLUGIN_WIT.contains("message-id"),
            "plugin WIT should contain message-id field"
        );
    }

    #[test]
    fn test_plugin_wit_contains_authorization_policy_world() {
        assert!(
            PLUGIN_WIT.contains("world authorization-policy"),
            "PLUGIN_WIT should contain 'world authorization-policy'"
        );
    }

    #[test]
    fn test_plugin_wit_authorization_policy_has_evaluate() {
        assert!(
            PLUGIN_WIT.contains("export evaluate: func(exchange: wasm-exchange) -> result<option<string>, wasm-error>"),
            "PLUGIN_WIT should contain evaluate export"
        );
    }

    #[test]
    fn test_plugin_wit_authorization_policy_has_init_with_config() {
        assert!(
            PLUGIN_WIT.contains(
                "export init: func(config: list<tuple<string, string>>) -> result<_, string>"
            ),
            "PLUGIN_WIT should contain init with config parameter"
        );
    }

    #[test]
    fn test_full_wit_contains_authorization_policy_world() {
        assert!(
            FULL_WIT.contains("world authorization-policy"),
            "FULL_WIT should contain 'world authorization-policy'"
        );
    }

    fn strip_comments(wit: &str) -> String {
        wit.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }

    #[test]
    fn test_example_bean_wit_matches_canonical() {
        let example_dir = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/wasm-bean-example/wit"
        ));
        if !example_dir.exists() {
            return;
        }
        let example_bean = std::fs::read_to_string(example_dir.join("camel-bean.wit"))
            .expect("read example bean wit");
        let canonical_stripped = strip_comments(BEAN_WIT);
        let example_stripped = strip_comments(&example_bean);
        assert_eq!(
            canonical_stripped, example_stripped,
            "examples/wasm-bean-example/wit/camel-bean.wit must match canonical without comments"
        );
    }

    #[test]
    fn test_example_plugin_wit_has_route_id_and_message_id() {
        let example_dir = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/wasm-bean-example/wit"
        ));
        if !example_dir.exists() {
            return;
        }
        let example_plugin = std::fs::read_to_string(example_dir.join("camel-plugin.wit"))
            .expect("read example plugin wit");
        assert!(
            example_plugin.contains("route-id"),
            "example camel-plugin.wit must contain route-id"
        );
        assert!(
            example_plugin.contains("message-id"),
            "example camel-plugin.wit must contain message-id"
        );
        assert!(
            example_plugin.contains("world authorization-policy"),
            "example camel-plugin.wit must contain authorization-policy world"
        );
        assert!(
            example_plugin.contains(
                "export init: func(config: list<tuple<string, string>>) -> result<_, string>"
            ),
            "example camel-plugin.wit must contain init(config) in bean world"
        );
    }

    #[test]
    fn test_full_wit_has_all_worlds() {
        // FULL_WIT (canonical camel-all.wit) is the merged reference document
        // containing every world. The example wit dir intentionally ships only
        // camel-plugin.wit + camel-bean.wit (compile-ready subset, no world
        // overlap) because wit-bindgen 0.58 rejects duplicate world
        // declarations across files in the same package.
        assert!(
            FULL_WIT.contains("world plugin"),
            "FULL_WIT must contain plugin world"
        );
        assert!(
            FULL_WIT.contains("world bean"),
            "FULL_WIT must contain bean world"
        );
        assert!(
            FULL_WIT.contains("world authorization-policy"),
            "FULL_WIT must contain authorization-policy world"
        );
    }

    #[test]
    fn test_host_wit_matches_canonical() {
        let host_wit_dir = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../components/camel-component-wasm/wit"
        ));
        if !host_wit_dir.exists() {
            return;
        }
        let host_plugin = std::fs::read_to_string(host_wit_dir.join("camel-plugin.wit"))
            .expect("read host plugin wit");
        let canonical_stripped = strip_comments(PLUGIN_WIT);
        let host_stripped = strip_comments(&host_plugin);
        assert_eq!(
            canonical_stripped, host_stripped,
            "camel-component-wasm/wit/camel-plugin.wit must match canonical without comments"
        );
    }

    #[test]
    fn test_content_type_constants_compile() {
        // Verifies the exported constants are accessible and have expected values.
        assert_eq!(APPLICATION_JSON, "application/json");
        assert_eq!(TEXT_PLAIN, "text/plain");
        assert_eq!(APPLICATION_OCTET_STREAM, "application/octet-stream");
        assert_eq!(TEXT_HTML, "text/html");
        assert_eq!(APPLICATION_XML, "application/xml");
        assert_eq!(
            APPLICATION_FORM_URLENCODED,
            "application/x-www-form-urlencoded"
        );
    }
}
