//! Route discovery module - finds and loads routes from YAML files using glob patterns.

use camel_core::route::RouteDefinition;
use glob::glob;
use std::fs;
use std::io;

use crate::env_interpolation::interpolate_env;
use crate::yaml::parse_yaml;

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
    let mut routes = Vec::new();

    for pattern in patterns {
        // glob returns an iterator over matching paths
        let entries = glob(pattern)?;

        for entry in entries {
            let path = entry.map_err(|e| DiscoveryError::GlobAccess {
                path: e.path().to_string_lossy().to_string(),
                source: e.into_error(),
            })?;
            let path_str = path.to_string_lossy().to_string();

            // Read file content
            let yaml_content = fs::read_to_string(&path).map_err(|e| DiscoveryError::Io {
                path: path_str.clone(),
                source: e,
            })?;

            let yaml_content =
                interpolate_env(&yaml_content).map_err(|var_name| DiscoveryError::Yaml {
                    path: path_str.clone(),
                    error: format!("Environment variable '{var_name}' is not set"),
                })?;

            // Parse YAML into route definitions
            let file_routes = parse_yaml(&yaml_content).map_err(|e| DiscoveryError::Yaml {
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
    use std::env;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn discovers_route_with_env_var_in_uri() {
        unsafe { env::set_var("TEST_DISC_TIMER_NAME", "my-tick") };

        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "routes:").unwrap();
        writeln!(f, "  - id: \"disc-route-1\"").unwrap();
        writeln!(f, "    from: \"timer:${{env:TEST_DISC_TIMER_NAME}}\"").unwrap();
        writeln!(f, "    steps:").unwrap();
        writeln!(f, "      - to: \"log:out\"").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].from_uri(), "timer:my-tick");

        unsafe { env::remove_var("TEST_DISC_TIMER_NAME") };
    }

    #[test]
    fn discover_fails_when_env_var_missing() {
        unsafe { env::remove_var("TEST_DISC_MISSING_VAR") };

        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "routes:").unwrap();
        writeln!(f, "  - id: \"disc-route-missing\"").unwrap();
        writeln!(f, "    from: \"timer:${{env:TEST_DISC_MISSING_VAR}}\"").unwrap();
        writeln!(f, "    steps: []").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected discover_routes to fail"),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("TEST_DISC_MISSING_VAR"),
            "error should mention var name, got: {msg}"
        );
    }
}
