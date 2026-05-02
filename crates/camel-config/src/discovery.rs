//! Route discovery module - finds and loads routes from YAML files using glob patterns.

use camel_core::route::RouteDefinition;
use glob::glob;
use std::fs;
use std::io;

use crate::yaml::parse_yaml_with_threshold;

/// Errors that can occur during route discovery.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// Invalid glob pattern.
    #[error("Glob pattern error: {0}")]
    GlobPattern(#[from] glob::PatternError),

    /// Error accessing file while iterating glob.
    #[error("Glob error accessing {path}: {source}")]
    GlobAccess { path: String, source: io::Error },

    /// Error reading a file.
    #[error("IO error reading {path}: {source}")]
    Io { path: String, source: io::Error },

    /// Error parsing YAML content.
    #[error("YAML parse error in {path}: {error}")]
    Yaml { path: String, error: String },
}

/// Discovers routes from YAML files matching the given glob patterns.
///
/// # Arguments
/// * `patterns` - Slice of glob patterns to match YAML files
///
/// # Returns
/// A vector of all discovered route definitions, or an error.
///
/// # Example
/// ```ignore
/// let routes = discover_routes(&["routes/*.yaml".to_string(), "extra/**/*.yaml".to_string()])?;
/// ```
pub fn discover_routes(patterns: &[String]) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_with_threshold(
        patterns,
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
    )
}

pub fn discover_routes_with_threshold(
    patterns: &[String],
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    let mut routes = Vec::new();

    for pattern in patterns {
        let entries = glob(pattern)?;

        for entry in entries {
            let path = entry.map_err(|e| DiscoveryError::GlobAccess {
                path: e.path().to_string_lossy().to_string(),
                source: e.into_error(),
            })?;
            let path_str = path.to_string_lossy().to_string();

            let yaml_content = fs::read_to_string(&path).map_err(|e| DiscoveryError::Io {
                path: path_str.clone(),
                source: e,
            })?;

            let file_routes = parse_yaml_with_threshold(&yaml_content, stream_cache_threshold)
                .map_err(|e| DiscoveryError::Yaml {
                    path: path_str,
                    error: e.to_string(),
                })?;

            routes.extend(file_routes);
        }
    }

    Ok(routes)
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
        assert!(matches!(err, DiscoveryError::GlobPattern(_)));
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
        assert!(matches!(err, DiscoveryError::Io { .. }));
    }

    #[test]
    fn discover_routes_invalid_yaml_returns_yaml_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.yaml");
        fs::write(&file, "not: [valid").unwrap();
        let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();

        let err = discover_routes(&[pattern]).err().unwrap();
        assert!(matches!(err, DiscoveryError::Yaml { .. }));
    }
}
