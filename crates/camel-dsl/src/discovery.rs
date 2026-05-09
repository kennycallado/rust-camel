//! Route discovery module - finds and loads routes from YAML/JSON files using glob patterns.

use camel_core::route::RouteDefinition;
use glob::glob;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::Path;

use crate::env_interpolation::interpolate_env;
use crate::json::{parse_json, parse_json_with_threshold};
use crate::yaml::{parse_yaml, parse_yaml_with_threshold};

/// Errors that can occur during route discovery.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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

    /// Environment variable not set during interpolation.
    #[error("Environment variable '{var_name}' not set (required by {path})")]
    Env { path: String, var_name: String },

    /// Error parsing JSON content.
    #[error("JSON parse error in {path}: {error}")]
    Json { path: String, error: String },

    /// File has an unsupported extension (not .yaml, .yml, or .json).
    #[error("Unsupported file extension '{extension}' in {path}")]
    UnsupportedExtension { path: String, extension: String },

    /// JSON file matched by a broad glob pattern requires an explicit .json pattern.
    #[error("JSON file {path} matched by broad pattern '{pattern}' — use an explicit .json glob like 'routes/*.json'")]
    JsonRequiresExplicitPattern { path: String, pattern: String },
}

/// Returns true if the glob pattern explicitly targets `.json` files.
///
/// Only patterns whose **file target extension** is `.json` (case-insensitive) return true.
/// A `.json` segment appearing only in a directory path (e.g. `config/.json/routes/*`)
/// does **not** authorize JSON loading.
fn pattern_targets_json(pattern: &str) -> bool {
    let lower = pattern.to_lowercase();
    // Extract the last path segment (the file/target portion) and check if it ends with .json
    lower
        .rsplit('/')
        .next()
        .is_some_and(|last_segment| last_segment.ends_with(".json"))
}

/// Extracts the lowercase file extension from a path, if any.
fn file_extension(path: &Path) -> Option<String> {
    path.extension().map(|ext| ext.to_string_lossy().to_lowercase())
}

/// Discovers routes from YAML/JSON files matching the given glob patterns.
///
/// # Arguments
/// * `patterns` - Slice of glob patterns to match route definition files
///
/// # Returns
/// A vector of all discovered route definitions, or an error.
///
/// # Supported formats
/// - `.yaml` / `.yml` — parsed as YAML
/// - `.json` — parsed as JSON, but only when the source pattern explicitly targets `.json`
///
/// # Example
/// ```ignore
/// let routes = discover_routes(&["routes/*.yaml".to_string(), "routes/*.json".to_string()])?;
/// ```
pub fn discover_routes(patterns: &[String]) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_inner(patterns, None)
}

/// Discovers routes with a custom stream-cache threshold.
///
/// Same as [`discover_routes`] but uses the given `stream_cache_threshold`
/// instead of the default when compiling routes.
pub fn discover_routes_with_threshold(
    patterns: &[String],
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_inner(patterns, Some(stream_cache_threshold))
}

fn discover_routes_inner(
    patterns: &[String],
    stream_cache_threshold: Option<usize>,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    let mut routes = Vec::new();

    for pattern in patterns {
        let is_json_pattern = pattern_targets_json(pattern);
        let entries = glob(pattern)?;

        for entry in entries {
            let path = entry.map_err(|e| DiscoveryError::GlobAccess {
                path: e.path().to_string_lossy().to_string(),
                source: e.into_error(),
            })?;
            let path_str = path.to_string_lossy().to_string();

            // Validate extension and JSON explicit-pattern gate BEFORE reading or
            // interpolating — rejects must not trigger env lookups.
            let ext = file_extension(&path);
            match ext.as_deref() {
                Some("yaml") | Some("yml") => {}
                Some("json") => {
                    if !is_json_pattern {
                        return Err(DiscoveryError::JsonRequiresExplicitPattern {
                            path: path_str,
                            pattern: pattern.clone(),
                        });
                    }
                }
                Some(other) => {
                    return Err(DiscoveryError::UnsupportedExtension {
                        path: path_str,
                        extension: other.to_string(),
                    });
                }
                None => {
                    return Err(DiscoveryError::UnsupportedExtension {
                        path: path_str,
                        extension: String::new(),
                    });
                }
            }

            // Read file content (only reached for accepted extensions)
            let raw_content = fs::read_to_string(&path).map_err(|e| DiscoveryError::Io {
                path: path_str.clone(),
                source: e,
            })?;

            // Source hash is based on raw content before env interpolation
            let mut hasher = DefaultHasher::new();
            raw_content.hash(&mut hasher);
            let source_hash = hasher.finish();

            // Env interpolation happens before parsing for both YAML and JSON.
            // NOTE: interpolation is textual — env values used inside JSON string
            // positions must already be valid/escaped for JSON context, otherwise
            // the subsequent JSON parse will fail with a DiscoveryError::Json.
            let content = interpolate_env(&raw_content).map_err(|var_name| DiscoveryError::Env {
                path: path_str.clone(),
                var_name,
            })?;

            // Parse based on accepted extension — no fallback arm needed because
            // the validation block above guarantees only yaml/yml/json reach here.
            match ext.as_deref() {
                Some("yaml") | Some("yml") => {
                    let file_routes = match stream_cache_threshold {
                        Some(threshold) => {
                            parse_yaml_with_threshold(&content, threshold)
                                .map_err(|e| DiscoveryError::Yaml {
                                    path: path_str,
                                    error: e.to_string(),
                                })?
                        }
                        None => parse_yaml(&content).map_err(|e| DiscoveryError::Yaml {
                            path: path_str,
                            error: e.to_string(),
                        })?,
                    };
                    for route in file_routes {
                        routes.push(route.with_source_hash(source_hash));
                    }
                }
                Some("json") => {
                    let file_routes = match stream_cache_threshold {
                        Some(threshold) => {
                            parse_json_with_threshold(&content, threshold)
                                .map_err(|e| DiscoveryError::Json {
                                    path: path_str,
                                    error: e.to_string(),
                                })?
                        }
                        None => parse_json(&content).map_err(|e| DiscoveryError::Json {
                            path: path_str,
                            error: e.to_string(),
                        })?,
                    };
                    for route in file_routes {
                        routes.push(route.with_source_hash(source_hash));
                    }
                }
                // SAFETY: Unreachable. The validation block above returns early for
                // any extension that is not yaml, yml, or json.
                _ => unreachable!(
                    "validated extension should be yaml/yml/json but was: {:?}",
                    ext
                ),
            }
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

    // ── pattern_targets_json ──────────────────────────────────────────

    #[test]
    fn pattern_targets_json_explicit() {
        assert!(pattern_targets_json("routes/*.json"));
    }

    #[test]
    fn pattern_targets_json_recursive() {
        assert!(pattern_targets_json("routes/**/*.json"));
    }

    #[test]
    fn pattern_targets_json_uppercase() {
        assert!(pattern_targets_json("routes/*.JSON"));
    }

    #[test]
    fn pattern_targets_json_with_trailing_slash() {
        // .json in directory name but file targets .json — should still match
        assert!(pattern_targets_json("config/.json/routes/*.json"));
    }

    #[test]
    fn pattern_targets_json_dir_name_only_returns_false() {
        // .json only appears in directory path, not as file extension
        assert!(!pattern_targets_json("config/.json/routes/*"));
    }

    #[test]
    fn pattern_targets_json_dir_name_recursive_returns_false() {
        assert!(!pattern_targets_json("config/.json/routes/**/*"));
    }

    #[test]
    fn pattern_targets_json_brace_expansion() {
        assert!(pattern_targets_json("routes/{a,b}.json"));
    }

    #[test]
    fn pattern_targets_json_uppercase_extension() {
        assert!(pattern_targets_json("routes/*.JSON"));
    }

    #[test]
    fn pattern_targets_json_broad_returns_false() {
        assert!(!pattern_targets_json("routes/*"));
    }

    #[test]
    fn pattern_targets_json_broad_recursive_returns_false() {
        assert!(!pattern_targets_json("routes/**/*"));
    }

    // ── YAML discovery ───────────────────────────────────────────────

    #[test]
    fn discovers_route_with_env_var_in_uri_yaml() {
        unsafe { env::set_var("TEST_DISC_TIMER_NAME", "my-tick") };

        let mut f = NamedTempFile::with_suffix(".yaml").unwrap();
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
    fn discover_fails_when_env_var_missing_yaml() {
        unsafe { env::remove_var("TEST_DISC_MISSING_VAR") };

        let mut f = NamedTempFile::with_suffix(".yaml").unwrap();
        writeln!(f, "routes:").unwrap();
        writeln!(f, "  - id: \"disc-route-missing\"").unwrap();
        writeln!(f, "    from: \"timer:${{env:TEST_DISC_MISSING_VAR}}\"").unwrap();
        writeln!(f, "    steps: []").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::Env { path: _, var_name } => {
                assert_eq!(var_name, "TEST_DISC_MISSING_VAR");
            }
            other => panic!("expected Env error, got: {other:?}"),
        }
    }

    #[test]
    fn discovers_yml_extension() {
        let mut f = NamedTempFile::with_suffix(".yml").unwrap();
        writeln!(f, "routes:").unwrap();
        writeln!(f, "  - id: \"yml-route\"").unwrap();
        writeln!(f, "    from: \"timer:tick\"").unwrap();
        writeln!(f, "    steps:").unwrap();
        writeln!(f, "      - to: \"log:info\"").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "yml-route");
    }

    // ── JSON discovery ───────────────────────────────────────────────

    #[test]
    fn discovers_explicit_json_route() {
        let mut f = NamedTempFile::with_suffix(".json").unwrap();
        write!(
            f,
            r#"{{
  "routes": [
    {{
      "id": "json-route-1",
      "from": "timer:tick?period=1000",
      "steps": [
        {{ "to": "log:info" }}
      ]
    }}
  ]
}}"#
        )
        .unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "json-route-1");
        assert_eq!(routes[0].from_uri(), "timer:tick?period=1000");
    }

    #[test]
    fn discovers_json_with_glob_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("route.json");
        fs::write(
            &file_path,
            r#"{"routes":[{"id":"glob-json","from":"direct:start","steps":[{"to":"log:out"}]}]}"#,
        )
        .unwrap();

        let pattern = dir.path().join("*.json").to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "glob-json");
    }

    // ── Extension/gate validation before env interpolation ────────────

    #[test]
    fn unsupported_extension_with_env_var_returns_unsupported_not_env() {
        // .xml file containing a real env var reference must fail with
        // UnsupportedExtension, NOT Env — env interpolation must not run.
        unsafe { env::remove_var("TASK3_SHOULD_NOT_READ_ENV") };

        let f = NamedTempFile::with_suffix(".xml").unwrap();
        let content = "content: ${env:TASK3_SHOULD_NOT_READ_ENV}";
        fs::write(f.path(), content).unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::UnsupportedExtension { path: _, extension } => {
                assert_eq!(extension, "xml");
            }
            other => panic!(
                "expected UnsupportedExtension, got: {:?} — env interpolation ran before extension check",
                other
            ),
        }
    }

    #[test]
    fn broad_glob_json_with_missing_env_returns_gate_not_env() {
        // Broad glob matching .json with missing env var must fail with
        // JsonRequiresExplicitPattern, NOT Env — gate must fire before interpolation.
        unsafe { env::remove_var("TASK3_SHOULD_NOT_READ_ENV") };

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("route.json");
        fs::write(
            &file_path,
            r#"{"routes":[{"id":"x","from":"timer:${env:TASK3_SHOULD_NOT_READ_ENV}","steps":[]}]}"#,
        )
        .unwrap();

        let pattern = dir.path().join("*").to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::JsonRequiresExplicitPattern { path: p, pattern: pat } => {
                assert!(p.ends_with("route.json"), "path was: {p}");
                assert!(!pat.contains(".json"), "pattern was: {pat}");
            }
            other => panic!(
                "expected JsonRequiresExplicitPattern, got: {:?} — gate did not fire before env interpolation",
                other
            ),
        }
    }

    // ── Unsupported extension ────────────────────────────────────────

    #[test]
    fn unsupported_extension_returns_error() {
        let mut f = NamedTempFile::with_suffix(".xml").unwrap();
        writeln!(f, "<routes/>").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::UnsupportedExtension { path: _, extension } => {
                assert_eq!(extension, "xml");
            }
            other => panic!("expected UnsupportedExtension, got: {other:?}"),
        }
    }

    #[test]
    fn no_extension_returns_error() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "routes:").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::UnsupportedExtension { path: _, extension } => {
                assert!(extension.is_empty());
            }
            other => panic!("expected UnsupportedExtension, got: {other:?}"),
        }
    }

    // ── Broad glob rejects JSON ──────────────────────────────────────

    #[test]
    fn broad_glob_rejects_json_with_explicit_pattern_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("route.json");
        fs::write(
            &file_path,
            r#"{"routes":[{"id":"broad-json","from":"direct:start","steps":[]}]}"#,
        )
        .unwrap();

        // Use a broad pattern that matches .json files but doesn't explicitly target .json
        let pattern = dir.path().join("*").to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::JsonRequiresExplicitPattern { path: p, pattern: pat } => {
                assert!(p.ends_with("route.json"), "path was: {p}");
                assert!(pat.ends_with('*'), "pattern was: {pat}");
                assert!(!pat.contains(".json"), "pattern was: {pat}");
            }
            other => panic!("expected JsonRequiresExplicitPattern, got: {other:?}"),
        }
    }

    // ── JSON env interpolation ────────────────────────────────────────────

    #[test]
    fn json_env_interpolation_with_unescaped_quote_returns_json_error() {
        // An env var containing a raw double-quote will break JSON parsing
        // because interpolation is textual — the quote is injected verbatim
        // into the JSON string, producing invalid JSON.
        unsafe { env::set_var("TEST_JSON_BAD_QUOTE", r#"has"quote"#) };

        let mut f = NamedTempFile::with_suffix(".json").unwrap();
        write!(
            f,
            r#"{{
  "routes": [
    {{
      "id": "bad-quote",
      "from": "timer:${{env:TEST_JSON_BAD_QUOTE}}",
      "steps": []
    }}
  ]
}}"#
        )
        .unwrap();

        // The temp file path IS the pattern (already ends in .json)
        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected JSON parse error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::Json { path: _, error } => {
                // Error should mention the parse failure (caused by unescaped quote)
                assert!(
                    !error.is_empty(),
                    "JSON parse error should describe the issue"
                );
            }
            other => panic!(
                "expected DiscoveryError::Json, got: {:?}",
                other
            ),
        }

        unsafe { env::remove_var("TEST_JSON_BAD_QUOTE") };
    }

    #[test]
    fn json_env_interpolation_with_valid_value_succeeds() {
        unsafe { env::set_var("TEST_JSON_GOOD_VAL", "tick") };

        let mut f = NamedTempFile::with_suffix(".json").unwrap();
        write!(
            f,
            r#"{{
  "routes": [
    {{
      "id": "good-env",
      "from": "timer:${{env:TEST_JSON_GOOD_VAL}}",
      "steps": []
    }}
  ]
}}"#
        )
        .unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].from_uri(), "timer:tick");

        unsafe { env::remove_var("TEST_JSON_GOOD_VAL") };
    }
}
