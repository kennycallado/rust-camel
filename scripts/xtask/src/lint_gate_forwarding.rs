//! Lint: verify that every workspace crate depending on `camel-bundles`
//! forwards the bundle gates it names, and that boot consumers forward
//! all of them, by static manifest analysis only.
//!
//! Established by rc-n8ss: a gate added to `camel-bundles` without
//! matching forwarding in a consumer compiles clean, then that consumer
//! silently boots without the component. The route then fails at boot
//! with an unknown scheme. This lint prevents regression.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Collect the gate set from a `camel-bundles` `[features]` table.
///
/// Every key is a gate except the literal key `default`, which Cargo
/// treats as the default-feature selector rather than a component gate.
fn gates_of(features: &toml::Table) -> BTreeSet<String> {
    features
        .keys()
        .filter(|k| k.as_str() != "default")
        .cloned()
        .collect()
}

/// Resolve the transitive closure of a feature through the consumer's
/// own `[features]` table.
///
/// String entries activate sibling features. Entries starting with
/// `dep:` are skipped (they activate optional dependencies, not
/// features). Unknown sibling names are added to the closure but not
/// expanded further — Cargo rejects them at build time, and the lint
/// does not duplicate Cargo's own validation.
fn closure_of(feature: &str, features: &BTreeMap<String, Vec<String>>) -> BTreeSet<String> {
    let mut closure = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut stack = vec![feature.to_string()];

    while let Some(current) = stack.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        closure.insert(current.clone());
        if let Some(entries) = features.get(&current) {
            for entry in entries {
                if entry.starts_with("dep:") {
                    continue;
                }
                // String entries join the closure; known sibling names
                // are also queued for expansion.
                closure.insert(entry.clone());
                if features.contains_key(entry) {
                    stack.push(entry.clone());
                }
            }
        }
    }

    closure
}

/// Whether a manifest depends on `camel-bundles`, directly or as a
/// dev-dependency. Absent tables count as empty.
fn is_consumer(manifest: &toml::Table) -> bool {
    let deps = manifest.get("dependencies").and_then(toml::Value::as_table);
    if deps.is_some_and(|t| t.contains_key("camel-bundles")) {
        return true;
    }
    let dev_deps = manifest
        .get("dev-dependencies")
        .and_then(toml::Value::as_table);
    dev_deps.is_some_and(|t| t.contains_key("camel-bundles"))
}

/// Check one consumer manifest against both gate-forwarding rules.
///
/// Rule 1 (shadow-feature forwarding): a consumer feature whose name
/// equals a bundles gate must transitively activate
/// `camel-bundles/<gate>` through the consumer's own feature graph.
///
/// Rule 2 (boot-consumer completeness): a consumer marked
/// `[package.metadata.camel-bundles] boot-consumer = true` must forward
/// every bundles gate through some feature. Unmarked consumers are
/// exempt.
fn check_consumer(
    crate_path: &str,
    gates: &BTreeSet<String>,
    features: &BTreeMap<String, Vec<String>>,
    boot_consumer: bool,
) -> Vec<String> {
    let mut violations = Vec::new();

    for name in features.keys() {
        if gates.contains(name) {
            let target = format!("camel-bundles/{name}");
            if !closure_of(name, features).contains(&target) {
                violations.push(format!(
                    "{crate_path}: feature '{name}' shadows bundles gate '{name}' \
                     but does not forward camel-bundles/{name}"
                ));
            }
        }
    }

    if boot_consumer {
        for gate in gates {
            let target = format!("camel-bundles/{gate}");
            let forwarded = features
                .keys()
                .any(|f| closure_of(f, features).contains(&target));
            if !forwarded {
                violations.push(format!(
                    "{crate_path}: boot consumer does not forward gate '{gate}'"
                ));
            }
        }
    }

    violations
}

/// Read the `[features]` table of a manifest into the map shape the
/// rule functions expect. Absent tables count as empty, and table-form
/// feature entries (`foo = { dep = "bar" }`) carry no sibling-feature
/// strings, so they map to an empty entry list.
fn features_of(manifest: &toml::Table) -> BTreeMap<String, Vec<String>> {
    manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .map(|features| {
            features
                .iter()
                .map(|(name, value)| {
                    let entries = value
                        .as_array()
                        .map(|entries| {
                            entries
                                .iter()
                                .filter_map(toml::Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    (name.clone(), entries)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `metadata.camel-bundles.boot-consumer` is set to `true` on a
/// manifest. Absent tables, absent keys, and non-boolean values count
/// as false.
fn is_boot_consumer(manifest: &toml::Table) -> bool {
    manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|pkg| pkg.get("metadata"))
        .and_then(toml::Value::as_table)
        .and_then(|metadata| metadata.get("camel-bundles"))
        .and_then(toml::Value::as_table)
        .and_then(|bundles| bundles.get("boot-consumer"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// Path relative to the workspace root with forward slashes. Used as
/// the `crate_path` in violation lines (e.g. `crates/camel-cli`).
fn rel_path(path: &Path, workspace_root: &Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Expand the root `[workspace] members` list into concrete member
/// manifest paths.
///
/// Entries containing `*` are glob patterns (resolved with the `glob`
/// crate) matched against `<entry>/Cargo.toml`; explicit path entries
/// map to `<entry>/Cargo.toml` directly. Manifests that do not exist
/// are skipped, and glob matches listed in `[workspace] exclude` are
/// dropped — explicit members beat the exclude list, mirroring
/// Cargo's own membership semantics. The result is sorted so the
/// violation output is deterministic.
fn member_manifests(
    workspace_root: &Path,
    members: &[String],
    excluded: &BTreeSet<String>,
) -> Vec<PathBuf> {
    let mut manifests = Vec::new();
    for member in members {
        if member.contains(['*', '?', '[']) {
            let pattern = workspace_root.join(member).join("Cargo.toml");
            let Ok(paths) = glob::glob(&pattern.to_string_lossy()) else {
                continue;
            };
            for manifest in paths.filter_map(Result::ok) {
                if !manifest.is_file() {
                    continue;
                }
                // `exclude` lists directories, so compare against the
                // manifest's parent, not the manifest itself.
                if manifest
                    .parent()
                    .is_some_and(|dir| excluded.contains(&rel_path(dir, workspace_root)))
                {
                    continue;
                }
                manifests.push(manifest);
            }
        } else {
            // Explicit member entries are unaffected by `exclude`.
            let manifest = workspace_root.join(member).join("Cargo.toml");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }
    }
    manifests.sort();
    manifests.dedup();
    manifests
}

/// Run the gate-forwarding lint over the workspace.
///
/// Parses the gate set from `crates/camel-bundles`, resolves workspace
/// member manifests from the root `[workspace] members` list, and
/// checks every consumer (a manifest depending on `camel-bundles`)
/// against both forwarding rules. Returns all violation lines in
/// sorted member order.
pub fn lint_gate_forwarding(workspace_root: &Path) -> Result<Vec<String>, String> {
    let bundles_manifest = workspace_root.join("crates/camel-bundles/Cargo.toml");
    let bundles_raw = std::fs::read_to_string(&bundles_manifest)
        .map_err(|e| format!("Cannot read {}: {e}", bundles_manifest.display()))?;
    let bundles: toml::Table = toml::from_str(&bundles_raw)
        .map_err(|e| format!("Cannot parse {}: {e}", bundles_manifest.display()))?;
    let bundles_features = bundles
        .get("features")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{} has no [features] table", bundles_manifest.display()))?;
    let gates = gates_of(bundles_features);

    let root_manifest = workspace_root.join("Cargo.toml");
    let root_raw = std::fs::read_to_string(&root_manifest)
        .map_err(|e| format!("Cannot read {}: {e}", root_manifest.display()))?;
    let root: toml::Table = toml::from_str(&root_raw)
        .map_err(|e| format!("Cannot parse {}: {e}", root_manifest.display()))?;
    let workspace = root
        .get("workspace")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{} has no [workspace] table", root_manifest.display()))?;
    let members: Vec<String> = workspace
        .get("members")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| {
            format!(
                "{} has no [workspace] members list",
                root_manifest.display()
            )
        })?
        .iter()
        .filter_map(toml::Value::as_str)
        .map(str::to_string)
        .collect();
    let excluded: BTreeSet<String> = workspace
        .get("exclude")
        .and_then(toml::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let mut violations = Vec::new();
    for manifest in member_manifests(workspace_root, &members, &excluded) {
        let raw = std::fs::read_to_string(&manifest)
            .map_err(|e| format!("Cannot read {}: {e}", manifest.display()))?;
        let manifest_tbl: toml::Table = toml::from_str(&raw)
            .map_err(|e| format!("Cannot parse {}: {e}", manifest.display()))?;
        if !is_consumer(&manifest_tbl) {
            continue;
        }
        let features = features_of(&manifest_tbl);
        let boot_consumer = is_boot_consumer(&manifest_tbl);
        let crate_dir = manifest.parent().unwrap_or(&manifest);
        let crate_path = rel_path(crate_dir, workspace_root);
        violations.extend(check_consumer(
            &crate_path,
            &gates,
            &features,
            boot_consumer,
        ));
    }

    Ok(violations)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gates(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn features(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(name, deps)| {
                (
                    name.to_string(),
                    deps.iter().map(|d| d.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn gates_default_excluded() {
        let mut table = toml::Table::new();
        table.insert("default".to_string(), toml::Value::Array(vec![]));
        table.insert("kafka".to_string(), toml::Value::Array(vec![]));
        table.insert("wasm".to_string(), toml::Value::Array(vec![]));

        assert_eq!(gates_of(&table), gates(&["kafka", "wasm"]));
    }

    #[test]
    fn shadow_feature_without_forwarding() {
        let gates = gates(&["kafka"]);
        let features = features(&[("kafka", &["dep:camel-component-kafka"])]);

        let violations = check_consumer("crates/x", &gates, &features, false);

        assert_eq!(
            violations,
            vec![
                "crates/x: feature 'kafka' shadows bundles gate 'kafka' but does not forward camel-bundles/kafka"
            ]
        );
    }

    #[test]
    fn transitive_forwarding_counts() {
        let gates = gates(&["kafka"]);
        let features = features(&[
            ("kafka", &["bundle-kafka"]),
            ("bundle-kafka", &["camel-bundles/kafka"]),
        ]);

        let violations = check_consumer("crates/x", &gates, &features, false);

        assert!(violations.is_empty());
    }

    #[test]
    fn boot_consumer_missing_gate() {
        let gates = gates(&["kafka", "mqtt"]);
        let features = features(&[("kafka", &["camel-bundles/kafka"])]);

        let violations = check_consumer("crates/x", &gates, &features, true);

        assert_eq!(
            violations,
            vec!["crates/x: boot consumer does not forward gate 'mqtt'"]
        );
    }

    #[test]
    fn unmarked_consumer_exempt() {
        let gates = gates(&["kafka"]);
        let features = features(&[("kafka", &["camel-bundles/kafka"])]);

        let violations = check_consumer("crates/x", &gates, &features, false);

        assert!(violations.is_empty());
    }

    #[test]
    fn non_gate_feature_ignored() {
        let gates = gates(&["kafka"]);
        let features = features(&[("otel", &["dep:tracing"])]);

        let violations = check_consumer("crates/x", &gates, &features, false);

        assert!(violations.is_empty());
    }

    #[test]
    fn dep_entries_skipped_in_closure() {
        let gates = gates(&["kafka"]);
        let features = features(&[
            ("kafka", &["dep:camel-component-kafka", "kafka-extra"]),
            ("kafka-extra", &["camel-bundles/kafka"]),
        ]);

        let violations = check_consumer("crates/x", &gates, &features, false);

        assert!(violations.is_empty());
    }

    #[test]
    fn non_consumer_ignored() {
        let manifest: toml::Table = toml::from_str(
            r#"
            [dependencies]
            serde = "1"
            "#,
        )
        .unwrap(); // allow-unwrap

        assert!(!is_consumer(&manifest));
    }

    #[test]
    fn dev_dependency_counts_as_consumer() {
        let manifest: toml::Table = toml::from_str(
            r#"
            [dependencies]

            [dev-dependencies]
            camel-bundles = { path = "../../crates/camel-bundles" }
            "#,
        )
        .unwrap(); // allow-unwrap

        assert!(is_consumer(&manifest));
    }

    #[test]
    fn members_expand_globs() {
        let root = tempfile::tempdir().unwrap(); // allow-unwrap
        for dir in ["crates/aa", "crates/ab", "crates/ac", "scripts/xt"] {
            let dir = root.path().join(dir);
            std::fs::create_dir_all(&dir).unwrap(); // allow-unwrap
            std::fs::write(dir.join("Cargo.toml"), "[package]\n").unwrap(); // allow-unwrap
        }
        let members = vec![
            "crates/a*".to_string(),
            "crates/?c".to_string(),
            "scripts/xt".to_string(),
        ];
        let excluded: BTreeSet<String> = ["crates/ab".to_string()].into();

        let manifests = member_manifests(root.path(), &members, &excluded);

        let dirs: Vec<String> = manifests
            .iter()
            .map(|m| rel_path(m.parent().unwrap(), root.path())) // allow-unwrap
            .collect();
        assert_eq!(dirs, vec!["crates/aa", "crates/ac", "scripts/xt"]);
    }
}
