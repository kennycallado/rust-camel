//! Route discovery module — delegates to [`camel_dsl::discovery`] for YAML and JSON.

use camel_core::route::RouteDefinition;

/// Errors that can occur during route discovery.
///
/// Wraps errors from [`camel_dsl::discovery::DiscoveryError`] with a single
/// variant to preserve the existing `camel-config` public API.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// Error from the underlying DSL discovery (YAML, JSON, glob, env, etc.).
    #[error("{0}")]
    Dsl(#[from] camel_dsl::DiscoveryError),
}

/// Discovers routes from YAML/JSON files matching the given glob patterns.
///
/// Delegates to [`camel_dsl::discover_routes`] which supports:
/// - `.yaml` / `.yml` — parsed as YAML
/// - `.json` — parsed as JSON, but only when the glob explicitly targets `.json`
///
/// # Arguments
/// * `patterns` - Slice of glob patterns to match route definition files
///
/// # Example
/// ```ignore
/// let routes = discover_routes(&["routes/*.yaml".to_string(), "routes/*.json".to_string()])?;
/// ```
pub fn discover_routes(patterns: &[String]) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_with_threshold(
        patterns,
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
    )
}

/// Discovers routes with a custom stream-cache threshold.
///
/// Same as [`discover_routes`] but compiles routes with the given threshold
/// instead of the default.
pub fn discover_routes_with_threshold(
    patterns: &[String],
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    camel_dsl::discover_routes_with_threshold(patterns, stream_cache_threshold)
        .map_err(DiscoveryError::from)
}

/// Discovers routes with a custom stream-cache threshold and security compile context.
///
/// Same as [`discover_routes_with_threshold`] but also passes a
/// [`camel_dsl::SecurityCompileContext`] through to route compilation, allowing
/// permission evaluator registries to be resolved during DSL compilation.
pub fn discover_routes_with_threshold_and_security(
    patterns: &[String],
    stream_cache_threshold: usize,
    security_ctx: camel_dsl::SecurityCompileContext,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    camel_dsl::discover_routes_with_threshold_and_security(
        patterns,
        stream_cache_threshold,
        security_ctx,
    )
    .map_err(DiscoveryError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discover_routes_with_no_patterns_returns_empty() {
        let routes = discover_routes(&[]).unwrap();
        assert!(routes.is_empty());
    }

    #[test]
    fn discover_routes_invalid_glob_returns_pattern_error() {
        let err = discover_routes(&["[".to_string()]).err().unwrap();
        assert!(matches!(err, DiscoveryError::Dsl(_)));
        assert!(
            err.to_string().contains("Glob pattern"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn discover_routes_nonexistent_file_returns_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let pattern = dir
            .path()
            .join("missing-*.yaml")
            .to_string_lossy()
            .to_string();
        let link_path = dir.path().join("missing-route.yaml");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("does-not-exist.yaml"), &link_path).unwrap();

        let err = discover_routes(&[pattern]).err().unwrap();
        assert!(matches!(err, DiscoveryError::Dsl(_)));
    }

    #[test]
    fn discover_routes_invalid_yaml_returns_yaml_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.yaml");
        fs::write(&file, "not: [valid").unwrap();
        let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();

        let err = discover_routes(&[pattern]).err().unwrap();
        assert!(matches!(err, DiscoveryError::Dsl(_)));
        assert!(
            err.to_string().contains("YAML parse error"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn discover_routes_explicit_json_pattern_loads_json() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("route.json");
        fs::write(
            &file_path,
            r#"{"routes":[{"id":"config-json","from":"direct:start","steps":[{"to":"log:out"}]}]}"#,
        )
        .unwrap();

        let pattern = dir.path().join("*.json").to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "config-json");
    }
}
