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
use crate::json::parse_json_with_threshold_and_security;
use crate::model::SecurityCompileContext;
use crate::template::materializer::materialize_and_compile;
use crate::yaml::parse_yaml_with_threshold_and_security;

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

    /// One or more template materialization failures, aggregated across files.
    /// Rendered as a multi-line report listing every failure with its path.
    #[error(
        "template materialization failed:\n{}",
        failures
            .iter()
            .map(|f| format!("  {f}"))
            .collect::<Vec<_>>()
            .join("\n")
    )]
    MaterializationFailures {
        failures: Vec<MaterializationFailure>,
    },

    /// Duplicate template id across files, or invalid template spec in file.
    #[error("Template error in {path}: {error}")]
    TemplateSpec { path: String, error: String },
}

/// A single template materialization failure, carrying the file path it
/// originated from — Pass 2 iterates specs collected across multiple files.
#[derive(Debug, Clone)]
pub struct MaterializationFailure {
    /// Path of the file declaring the templated route spec.
    pub path: String,
    /// The referenced template id.
    pub template_ref: String,
    /// Optional explicit route id of the failing instance.
    pub route_id: Option<String>,
    /// The classified template error.
    pub error: TemplateError,
}

impl std::fmt::Display for MaterializationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (template '{}'): {}",
            self.path, self.template_ref, self.error
        )
    }
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

/// Parse a `TemplateError::InvalidParameter` Display string
/// (`parameter '<name>' declared type <ty> but value '<value>' is not
/// coercible`) back into its fields, preserving the error class through
/// Config-string propagation. Returns `None` for any other shape.
fn parse_invalid_parameter_message(msg: &str) -> Option<(String, String, String)> {
    let rest = msg.strip_prefix("parameter '")?;
    let (name, rest) = rest.split_once("' ")?;
    let rest = rest.strip_prefix("declared type ")?;
    let (ty, rest) = rest.split_once(" but value '")?;
    let value = rest.strip_suffix("' is not coercible")?;
    Some((name.to_string(), ty.to_string(), value.to_string()))
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
                source: e.into(),
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
                    let file_routes = parse_yaml_with_threshold_and_security(
                        &content,
                        stream_cache_threshold
                            .unwrap_or(camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD),
                        security_ctx.clone().unwrap_or_default(),
                    )
                    .map_err(|e| DiscoveryError::Yaml {
                        path: path_str.clone(),
                        error: e.to_string(),
                    })?;
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
                    let file_routes = parse_json_with_threshold_and_security(
                        &content,
                        stream_cache_threshold
                            .unwrap_or(camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD),
                        security_ctx.clone().unwrap_or_default(),
                    )
                    .map_err(|e| DiscoveryError::Json {
                        path: path_str.clone(),
                        error: e.to_string(),
                    })?;
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

    // Pass 2: materialize all templated specs using the collected templates.
    // Failures are aggregated — every spec is attempted so the caller sees
    // the full set of broken templates, not just the first one.
    let mut seen_route_ids: HashSet<String> =
        routes.iter().map(|r| r.route_id().to_string()).collect();
    let mut failures: Vec<MaterializationFailure> = Vec::new();

    for (path_str, spec) in &templated_specs {
        let Some(template) = templates.get(&spec.route_template_ref) else {
            failures.push(MaterializationFailure {
                path: path_str.clone(),
                template_ref: spec.route_template_ref.clone(),
                route_id: spec.route_id.clone(),
                error: TemplateError::NotFound(spec.route_template_ref.clone()),
            });
            continue;
        };

        let compiled = match materialize_and_compile(
            template,
            spec,
            stream_cache_threshold
                .unwrap_or(camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD),
            security_ctx.clone().unwrap_or_default(),
        ) {
            Ok(compiled) => compiled,
            Err(e) => {
                let source = match &e {
                    camel_api::CamelError::RouteError(msg)
                        if msg.starts_with("route requires an authenticator") =>
                    {
                        TemplateError::SecurityRequired {
                            template_id: spec.route_template_ref.clone(),
                            detail: msg.clone(),
                        }
                    }
                    // Typed-parameter coercion failures surface as Config
                    // strings in the InvalidParameter display format —
                    // parse the fields back so the class survives to the
                    // aggregated surface instead of flattening to
                    // InvalidBody. Unparseable text falls through below.
                    camel_api::CamelError::Config(msg)
                        if msg.starts_with("parameter '") && msg.contains("declared type") =>
                    {
                        match parse_invalid_parameter_message(msg) {
                            Some((name, ty, value)) => {
                                TemplateError::InvalidParameter(name, ty, value)
                            }
                            None => TemplateError::InvalidBody(msg.clone()),
                        }
                    }
                    camel_api::CamelError::Config(msg) => TemplateError::InvalidBody(msg.clone()),
                    other => TemplateError::InvalidBody(other.to_string()),
                };
                failures.push(MaterializationFailure {
                    path: path_str.clone(),
                    template_ref: spec.route_template_ref.clone(),
                    route_id: spec.route_id.clone(),
                    error: source,
                });
                continue;
            }
        };

        for result in compiled {
            let rid = result.route_def.route_id().to_string();
            if !seen_route_ids.insert(rid.clone()) {
                // Precedence decision: a duplicate id aborts immediately and
                // preempts any materialization failures collected so far —
                // identity conflicts poison the seen-id set, so continuing
                // would attribute later failures to the wrong instance.
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

    if !failures.is_empty() {
        return Err(DiscoveryError::MaterializationFailures { failures });
    }

    Ok(routes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Pin the display-format round-trip: `parse_invalid_parameter_message`
    /// must recover the three fields from a REAL `TemplateError::InvalidParameter`
    /// Display string. If the thiserror format string in camel-api changes,
    /// this fails loudly instead of silently downgrading the error class to
    /// InvalidBody at the aggregation surface.
    #[test]
    fn invalid_parameter_display_round_trip_pins_parser() {
        let err = TemplateError::InvalidParameter(
            "delay".to_string(),
            "number".to_string(),
            "abc".to_string(),
        );
        let parsed = parse_invalid_parameter_message(&err.to_string());
        assert_eq!(
            parsed,
            Some(("delay".to_string(), "number".to_string(), "abc".to_string()))
        );
        // Values containing delimiter substrings must still round-trip
        // (split_once binds first, strip_suffix binds last).
        let tricky = TemplateError::InvalidParameter(
            "p".to_string(),
            "string".to_string(),
            "' is not coercible' is not coercible".to_string(),
        );
        assert_eq!(
            parse_invalid_parameter_message(&tricky.to_string()),
            Some((
                "p".to_string(),
                "string".to_string(),
                "' is not coercible' is not coercible".to_string()
            ))
        );
        // Non-matching input yields None (falls back to InvalidBody).
        assert_eq!(parse_invalid_parameter_message("parameter 'x' oops"), None);
    }

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
            DiscoveryError::MaterializationFailures { failures } => {
                assert_eq!(failures.len(), 1, "expected exactly one failure: {err:?}");
                let failure = &failures[0];
                assert_eq!(failure.template_ref, "nonexistent-template");
                match &failure.error {
                    TemplateError::NotFound(ref_) => {
                        assert_eq!(ref_, "nonexistent-template");
                    }
                    other => panic!("expected NotFound, got: {other:?}"),
                }
            }
            other => panic!("expected MaterializationFailures, got: {other:?}"),
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
    fn materialized_source_hash_is_instance_sensitive() {
        let dir = tempfile::tempdir().unwrap();

        let template_body = vec![serde_json::json!({
            "id": "same-route",
            "from": "direct:x",
            "steps": []
        })];
        let empty_params = std::collections::BTreeMap::new();
        let hash_a = crate::template::materializer::compute_instance_source_hash(
            &template_body,
            &empty_params,
            "inst-a",
        );
        let hash_b = crate::template::materializer::compute_instance_source_hash(
            &template_body,
            &empty_params,
            "inst-b",
        );

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

        let mut hashes_by_id = std::collections::HashMap::new();
        for route in &routes {
            let hash = route.source_hash().expect("should have source_hash");
            assert_ne!(hash, 0, "source_hash should be non-zero");
            hashes_by_id.insert(route.route_id().to_string(), hash);
        }
        assert_eq!(
            hashes_by_id.get("inst-a"),
            Some(&hash_a),
            "inst-a hash must reflect body + params + its effective id"
        );
        assert_eq!(
            hashes_by_id.get("inst-b"),
            Some(&hash_b),
            "inst-b hash must reflect body + params + its effective id"
        );
        assert_ne!(
            hash_a, hash_b,
            "instances differing only in override id must hash distinctly"
        );
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

    // ── threshold-less discovery threads security context ────────────

    struct TestAuthenticator;

    #[async_trait::async_trait]
    impl camel_auth::TokenAuthenticator for TestAuthenticator {
        async fn authenticate_bearer(
            &self,
            _token: &str,
        ) -> Result<camel_api::security_policy::Principal, camel_api::CamelError> {
            Ok(camel_api::security_policy::Principal {
                subject: "test-user".into(),
                issuer: "test-issuer".into(),
                audience: vec![],
                scopes: vec!["read:api".into()],
                roles: vec!["admin".into()],
                claims: serde_json::Value::Null,
            })
        }
    }

    #[test]
    fn threshold_less_discovery_threads_security_context() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("route.yaml");
        fs::write(
            &file_path,
            r#"
routes:
  - id: sec-route
    from: direct:start
    security_policy:
      roles: ["admin"]
    steps:
      - to: log:info
"#,
        )
        .unwrap();
        let pattern = file_path.to_string_lossy().to_string();

        let auth = std::sync::Arc::new(TestAuthenticator)
            as std::sync::Arc<dyn camel_auth::TokenAuthenticator>;
        let ctx = SecurityCompileContext::new(Some(auth), None);

        // (None-threshold, Some-ctx) — only reachable through the private fn.
        let routes =
            discover_routes_inner(std::slice::from_ref(&pattern), None, Some(ctx)).unwrap();
        assert_eq!(routes.len(), 1);
        assert!(routes[0].security_authenticator().is_some());

        // Fail-closed pin: public path (None ctx) must reject the secured route.
        let err = match discover_routes_inner(&[pattern], None, None) {
            Ok(_) => panic!("expected error for secured route without authenticator"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("route requires an authenticator"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn security_required_error_classified() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("secured-tpl.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templates:
  - id: secured-tpl
    parameters: []
    routes:
      - id: "secured-route"
        from: "direct:start"
        security_policy:
          roles: ["admin"]
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: secured-tpl
    parameters: {}
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();

        // Fail-closed: default security ctx (no authenticator) must classify
        // the failure as SecurityRequired, not InvalidBody.
        let err = match discover_routes_inner(&[pattern], None, None) {
            Ok(_) => panic!("expected secured templated route to fail closed"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::MaterializationFailures { failures } => {
                assert_eq!(failures.len(), 1, "expected exactly one failure: {err:?}");
                match &failures[0].error {
                    TemplateError::SecurityRequired { template_id, .. } => {
                        assert_eq!(template_id, "secured-tpl");
                    }
                    other => panic!("expected SecurityRequired, got: {other:?}"),
                }
            }
            other => panic!("expected MaterializationFailures, got: {other:?}"),
        }
    }
}
