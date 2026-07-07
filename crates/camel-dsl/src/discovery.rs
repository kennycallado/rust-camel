//! Route discovery module - finds and loads routes from YAML/JSON files using glob patterns.

use camel_api::template::{RouteTemplateSpec, TemplateError, TemplatedRouteSpec};
use camel_core::route::RouteDefinition;
use glob::glob;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::Path;

use crate::env_interpolation::interpolate_env;
use crate::json::{parse_json, parse_json_with_threshold_and_security};
use crate::model::SecurityCompileContext;
use crate::template::materializer::materialize_and_compile;
use crate::yaml::{parse_yaml, parse_yaml_with_threshold_and_security};

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
    #[error(
        "JSON file {path} matched by broad pattern '{pattern}' — use an explicit .json glob like 'routes/*.json'"
    )]
    JsonRequiresExplicitPattern { path: String, pattern: String },

    /// Template reference points to a template id that was never defined.
    #[error("Template '{template_id}' not found in {path}")]
    TemplateNotFound { path: String, template_id: String },

    /// A route id was produced more than once across regular + materialized routes.
    #[error("Duplicate route id '{route_id}' in {path}")]
    DuplicateRouteId { path: String, route_id: String },

    /// Template parsing or materialization failed (invalid body, missing params, etc.).
    #[error("Template error in {path}: {source}")]
    MaterializationFailed {
        path: String,
        #[source]
        source: TemplateError,
    },

    /// Duplicate template id across files, or invalid template spec in file.
    #[error("Template error in {path}: {error}")]
    TemplateSpec { path: String, error: String },
}

/// Maximum size for individual route files (YAML/JSON) during discovery.
/// Prevents OOM from abnormally large files.
const MAX_ROUTE_FILE_SIZE: u64 = 16 * 1024 * 1024;

/// Read a file with a size cap. Stats first, rejects if too large.
fn read_file_capped(path: &Path) -> Result<String, DiscoveryError> {
    let metadata = fs::metadata(path).map_err(|e| DiscoveryError::Io {
        path: path.to_string_lossy().to_string(),
        source: e,
    })?;
    if metadata.len() > MAX_ROUTE_FILE_SIZE {
        return Err(DiscoveryError::Io {
            path: path.to_string_lossy().to_string(),
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Route file `{}` is {} bytes, exceeds max {} bytes",
                    path.display(),
                    metadata.len(),
                    MAX_ROUTE_FILE_SIZE
                ),
            ),
        });
    }
    fs::read_to_string(path).map_err(|e| DiscoveryError::Io {
        path: path.to_string_lossy().to_string(),
        source: e,
    })
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
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
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
    discover_routes_inner(patterns, None, None)
}

/// Discovers routes with a custom stream-cache threshold.
///
/// Same as [`discover_routes`] but uses the given `stream_cache_threshold`
/// instead of the default when compiling routes.
pub fn discover_routes_with_threshold(
    patterns: &[String],
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_inner(patterns, Some(stream_cache_threshold), None)
}

/// Discovers routes with a custom stream-cache threshold and security compile context.
///
/// Same as [`discover_routes_with_threshold`] but also passes a
/// [`SecurityCompileContext`] through to route compilation, allowing
/// permission evaluators and security policy registries to be resolved
/// during DSL compilation.
pub fn discover_routes_with_threshold_and_security(
    patterns: &[String],
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_inner(patterns, Some(stream_cache_threshold), Some(security_ctx))
}

fn discover_routes_inner(
    patterns: &[String],
    stream_cache_threshold: Option<usize>,
    security_ctx: Option<SecurityCompileContext>,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    let mut routes = Vec::new();
    let mut templates: HashMap<String, RouteTemplateSpec> = HashMap::new();
    // (path_str, templated_spec) — materialized after all files scanned
    let mut templated_specs: Vec<(String, TemplatedRouteSpec)> = Vec::new();

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
            let raw_content = read_file_capped(&path)?;

            // Source hash is based on raw content before env interpolation
            let mut hasher = DefaultHasher::new();
            raw_content.hash(&mut hasher);
            let source_hash = hasher.finish();

            // Env interpolation happens before parsing for both YAML and JSON.
            let content =
                interpolate_env(&raw_content).map_err(|var_name| DiscoveryError::Env {
                    path: path_str.clone(),
                    var_name,
                })?;

            // Parse based on extension — collect templates, templated specs, and regular routes
            match ext.as_deref() {
                Some("yaml") | Some("yml") => {
                    // Parse regular routes
                    let file_routes = match stream_cache_threshold {
                        Some(threshold) => parse_yaml_with_threshold_and_security(
                            &content,
                            threshold,
                            security_ctx.clone().unwrap_or_default(),
                        )
                        .map_err(|e| DiscoveryError::Yaml {
                            path: path_str.clone(),
                            error: e.to_string(),
                        })?,
                        None => parse_yaml(&content).map_err(|e| DiscoveryError::Yaml {
                            path: path_str.clone(),
                            error: e.to_string(),
                        })?,
                    };
                    for route in file_routes {
                        routes.push(route.with_source_hash(source_hash));
                    }

                    // Parse templates
                    let tpls =
                        crate::template::yaml::parse_yaml_templates(&content).map_err(|e| {
                            DiscoveryError::MaterializationFailed {
                                path: path_str.clone(),
                                source: e,
                            }
                        })?;
                    for tpl in tpls {
                        if templates.contains_key(&tpl.id) {
                            return Err(DiscoveryError::TemplateSpec {
                                path: path_str.clone(),
                                error: format!("duplicate template id '{}'", tpl.id),
                            });
                        }
                        templates.insert(tpl.id.clone(), tpl);
                    }

                    // Parse templated route specs for later materialization
                    let specs = crate::template::yaml::parse_yaml_templated_routes(&content)
                        .map_err(|e| DiscoveryError::MaterializationFailed {
                            path: path_str.clone(),
                            source: e,
                        })?;
                    for spec in specs {
                        templated_specs.push((path_str.clone(), spec));
                    }
                }
                Some("json") => {
                    // Parse regular routes
                    let file_routes = match stream_cache_threshold {
                        Some(threshold) => parse_json_with_threshold_and_security(
                            &content,
                            threshold,
                            security_ctx.clone().unwrap_or_default(),
                        )
                        .map_err(|e| DiscoveryError::Json {
                            path: path_str.clone(),
                            error: e.to_string(),
                        })?,
                        None => parse_json(&content).map_err(|e| DiscoveryError::Json {
                            path: path_str.clone(),
                            error: e.to_string(),
                        })?,
                    };
                    for route in file_routes {
                        routes.push(route.with_source_hash(source_hash));
                    }

                    // Parse templates
                    let tpls =
                        crate::template::json::parse_json_templates(&content).map_err(|e| {
                            DiscoveryError::MaterializationFailed {
                                path: path_str.clone(),
                                source: e,
                            }
                        })?;
                    for tpl in tpls {
                        if templates.contains_key(&tpl.id) {
                            return Err(DiscoveryError::TemplateSpec {
                                path: path_str.clone(),
                                error: format!("duplicate template id '{}'", tpl.id),
                            });
                        }
                        templates.insert(tpl.id.clone(), tpl);
                    }

                    // Parse templated route specs for later materialization
                    let specs = crate::template::json::parse_json_templated_routes(&content)
                        .map_err(|e| DiscoveryError::MaterializationFailed {
                            path: path_str.clone(),
                            source: e,
                        })?;
                    for spec in specs {
                        templated_specs.push((path_str.clone(), spec));
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

    // Pass 2: materialize all templated specs using the collected templates
    let mut seen_route_ids: HashSet<String> =
        routes.iter().map(|r| r.route_id().to_string()).collect();

    for (path_str, spec) in &templated_specs {
        let template = templates.get(&spec.route_template_ref).ok_or_else(|| {
            DiscoveryError::TemplateNotFound {
                path: path_str.clone(),
                template_id: spec.route_template_ref.clone(),
            }
        })?;

        let compiled = materialize_and_compile(template, spec).map_err(|e| {
            let source = match &e {
                camel_api::CamelError::Config(msg) => TemplateError::InvalidBody(msg.clone()),
                other => TemplateError::InvalidBody(other.to_string()),
            };
            DiscoveryError::MaterializationFailed {
                path: path_str.clone(),
                source,
            }
        })?;

        for result in compiled {
            let rid = result.route_def.route_id().to_string();
            if !seen_route_ids.insert(rid.clone()) {
                return Err(DiscoveryError::DuplicateRouteId {
                    path: path_str.clone(),
                    route_id: rid,
                });
            }
            let route_def = match result.source_hash {
                Some(h) => result.route_def.with_source_hash(h),
                None => result.route_def,
            };
            routes.push(route_def);
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
            DiscoveryError::JsonRequiresExplicitPattern {
                path: p,
                pattern: pat,
            } => {
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
            DiscoveryError::JsonRequiresExplicitPattern {
                path: p,
                pattern: pat,
            } => {
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
            other => panic!("expected DiscoveryError::Json, got: {:?}", other),
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

    // ── Template-aware discovery ─────────────────────────────────────

    #[test]
    fn discovers_yaml_template_and_materializes() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("routes.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templates:
  - id: http-route
    parameters:
      - name: path
    routes:
      - id: "materialized-http"
        from: "rest:{{path}}"
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: http-route
    route_id: "my-http"
    parameters:
      path: /api/users
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "my-http");
        assert_eq!(routes[0].from_uri(), "rest:/api/users");
    }

    #[test]
    fn discovers_json_template_and_materializes() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("routes.json");
        fs::write(
            &file_path,
            r#"{
  "routes": [],
  "templates": [
    {
      "id": "timer-route",
      "parameters": [{"name": "period"}],
      "routes": [
        {
          "id": "materialized-timer",
          "from": "timer:tick?period={{period}}",
          "steps": []
        }
      ]
    }
  ],
  "templated_routes": [
    {
      "route_template_ref": "timer-route",
      "parameters": {"period": "5000"}
    }
  ]
}"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "materialized-timer");
        assert_eq!(routes[0].from_uri(), "timer:tick?period=5000");
    }

    #[test]
    fn discovers_mixed_regular_routes_and_templates() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("mixed.yaml");
        fs::write(
            &file_path,
            r#"
routes:
  - id: regular-route
    from: direct:start
    steps:
      - to: log:info
templates:
  - id: log-route
    parameters:
      - name: level
    routes:
      - id: "materialized-log"
        from: "direct:log"
        steps:
          - to: "log:{{level}}"
templated_routes:
  - route_template_ref: log-route
    parameters:
      level: warn
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 2);
        let ids: Vec<&str> = routes.iter().map(|r| r.route_id()).collect();
        assert!(ids.contains(&"regular-route"));
        assert!(ids.contains(&"materialized-log"));
    }

    #[test]
    fn discovers_cross_file_template_reference() {
        let dir = tempfile::tempdir().unwrap();
        // File A: defines the template
        let file_a = dir.path().join("templates.yaml");
        fs::write(
            &file_a,
            r#"
routes: []
templates:
  - id: shared-http
    parameters:
      - name: path
    routes:
      - id: "shared-route"
        from: "rest:{{path}}"
        steps:
          - to: "log:shared"
"#,
        )
        .unwrap();

        // File B: instantiates the template
        let file_b = dir.path().join("instances.yaml");
        fs::write(
            &file_b,
            r#"
routes: []
templated_routes:
  - route_template_ref: shared-http
    parameters:
      path: /cross-file
"#,
        )
        .unwrap();

        let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "shared-route");
        assert_eq!(routes[0].from_uri(), "rest:/cross-file");
    }

    #[test]
    fn missing_template_ref_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("missing.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templated_routes:
  - route_template_ref: nonexistent-template
    parameters:
      path: /test
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::TemplateNotFound {
                path: _,
                template_id,
            } => {
                assert_eq!(template_id, "nonexistent-template");
            }
            other => panic!("expected TemplateNotFound error, got: {other:?}"),
        }
    }

    #[test]
    fn duplicate_template_ids_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        // File A: defines template "dup-tpl"
        let file_a = dir.path().join("a.yaml");
        fs::write(
            &file_a,
            r#"
routes: []
templates:
  - id: dup-tpl
    routes:
      - id: "route-a"
        from: "direct:a"
"#,
        )
        .unwrap();

        // File B: also defines template "dup-tpl"
        let file_b = dir.path().join("b.yaml");
        fs::write(
            &file_b,
            r#"
routes: []
templates:
  - id: dup-tpl
    routes:
      - id: "route-b"
        from: "direct:b"
"#,
        )
        .unwrap();

        let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::TemplateSpec { path: _, error } => {
                assert!(error.contains("dup-tpl"));
                assert!(error.contains("duplicate"));
            }
            other => panic!("expected TemplateSpec error, got: {other:?}"),
        }
    }

    #[test]
    fn materialized_routes_preserve_source_hash() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("hash-test.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templates:
  - id: hash-tpl
    routes:
      - id: "hash-route"
        from: "direct:hash"
        steps: []
templated_routes:
  - route_template_ref: hash-tpl
    parameters: {}
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        let hash = routes[0].source_hash();
        assert!(hash.is_some(), "materialized route should have source_hash");
        assert_ne!(hash.unwrap(), 0, "source_hash should be non-zero");
    }

    #[test]
    fn materialized_source_hash_reflects_template_body_not_instance_file() {
        let dir = tempfile::tempdir().unwrap();

        let template_body = serde_json::json!([{
            "id": "same-route",
            "from": "direct:x",
            "steps": []
        }]);
        let template_hash = {
            let s = serde_json::to_string(&template_body).unwrap();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        };

        let file_path = dir.path().join("two-instances.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templates:
  - id: shared-tpl
    routes:
      - id: "same-route"
        from: "direct:x"
        steps: []
templated_routes:
  - route_template_ref: shared-tpl
    route_id: "inst-a"
    parameters: {}
  - route_template_ref: shared-tpl
    route_id: "inst-b"
    parameters: {}
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 2);

        for route in &routes {
            let hash = route.source_hash().expect("should have source_hash");
            assert_eq!(
                hash, template_hash,
                "materialized route source_hash must match template body hash, not instance file hash"
            );
        }
    }

    #[test]
    fn template_only_file_without_routes_key() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("tpl-only.yaml");
        fs::write(
            &file_path,
            r#"
templates:
  - id: solo-tpl
    parameters:
      - name: target
    routes:
      - id: "solo-{{target}}"
        from: "direct:start"
        steps:
          - to: "{{target}}"
templated_routes:
  - route_template_ref: solo-tpl
    parameters:
      target: "log:info"
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].from_uri(), "direct:start");
    }

    #[test]
    fn duplicate_route_ids_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("dup-rid.yaml");
        fs::write(
            &file_path,
            r#"
routes:
  - id: "shared-id"
    from: "direct:a"
    steps: []
templates:
  - id: tpl
    routes:
      - id: "tpl-route"
        from: "direct:b"
        steps: []
templated_routes:
  - route_template_ref: tpl
    route_id: "shared-id"
    parameters: {}
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected duplicate route id error"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("shared-id"),
            "expected duplicate route id error, got: {msg}"
        );
        match &err {
            DiscoveryError::DuplicateRouteId { route_id, .. } => {
                assert_eq!(route_id, "shared-id");
            }
            other => panic!("expected DuplicateRouteId error, got: {other:?}"),
        }
    }

    #[test]
    fn discovers_multi_route_template() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("multi.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templates:
  - id: chain
    parameters:
      - name: PROV
    routes:
      - id: "step1-{{PROV}}"
        from: "direct:start"
        steps:
          - to: "controlbus:route?routeId=step2-{{PROV}}&action=start"
      - id: "step2-{{PROV}}"
        from: "direct:step2"
        steps:
          - to: "log:done"
templated_routes:
  - route_template_ref: chain
    parameters:
      PROV: granada
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].route_id(), "step1-granada");
        assert_eq!(routes[1].route_id(), "step2-granada");
    }
}
