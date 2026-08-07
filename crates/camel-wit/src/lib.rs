/// WIT source for the `plugin` world (standalone file).
pub const PLUGIN_WIT: &str = include_str!("../wit/camel-plugin.wit");

/// WIT source for the `bean` world (standalone file, same package as PLUGIN_WIT).
pub const BEAN_WIT: &str = include_str!("../wit/camel-bean.wit");

/// WIT source for the `source` world (standalone file, same package as PLUGIN_WIT).
pub const SOURCE_WIT: &str = include_str!("../wit/camel-source.wit");

/// Combined WIT package with both `plugin` and `bean` worlds in a single document.
pub const FULL_WIT: &str = include_str!("../wit/camel-all.wit");

#[cfg(test)]
mod tests {
    use super::*;

    // ── WIT-002: Tests for WIT definitions ──────────────────────────────────

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
        assert!(PLUGIN_WIT.contains("package camel:plugin@1.0.0"));
        assert!(BEAN_WIT.contains("package camel:plugin@1.0.0"));
        assert!(FULL_WIT.contains("package camel:plugin@1.0.0"));
        assert!(SOURCE_WIT.contains("package camel:plugin@1.0.0"));
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
        assert!(
            host_wit_dir.exists(),
            "camel-component-wasm/wit/ must exist — drift cannot be detected if the directory is missing"
        );

        for (name, canonical) in [
            ("camel-plugin.wit", PLUGIN_WIT),
            ("camel-bean.wit", BEAN_WIT),
            ("camel-source.wit", SOURCE_WIT),
        ] {
            let host = std::fs::read_to_string(host_wit_dir.join(name))
                .unwrap_or_else(|_| panic!("read host {}", name));
            let canonical_stripped = strip_comments(canonical);
            let host_stripped = strip_comments(&host);
            assert_eq!(
                canonical_stripped, host_stripped,
                "camel-component-wasm/wit/{} must match canonical without comments",
                name
            );
        }
    }
}
