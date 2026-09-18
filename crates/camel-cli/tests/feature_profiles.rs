//! Feature-profile guards for the `clidiet` change.
//!
//! Task 1.1 plants the profile-test scaffolding: a golden snapshot of the
//! default feature closure plus allocator-graph assertions. The
//! `--all-features` mimalloc check was planted red; task 1.2's mimalloc
//! feature removal turned it green.
//!
//! Regenerate the golden fixture from a clean tree at the base commit.
//! The `CARGO_TERM_COLOR=never` prefix is load-bearing: a colored
//! `cargo tree` styles the `(*)` repeat marker with ANSI sequences, which
//! the plain-text normalization cannot strip (rc-k6dln — CI exports
//! `CARGO_TERM_COLOR=always` workflow-wide and the test used to inherit
//! it, producing 1200 spurious extras).
//!
//! ```text
//! CARGO_TERM_COLOR=never cargo tree -p camel-cli -e features,no-dev \
//!   --prefix none --locked \
//!   | sed -E 's| \(/[^)]*\)||g; s| \(\*\)||g; s| \[\*\]||g' | LC_ALL=C sort -u \
//!   > crates/camel-cli/tests/fixtures/default-deptree.txt
//! ```

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Workspace root: two levels above this crate's manifest directory.
fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crate_dir = manifest.parent().expect("manifest dir has a parent");
    let workspace = crate_dir.parent().expect("crate dir has a parent");
    workspace.to_path_buf()
}

/// Shared `cargo tree` argument list used by [`tree_command`].
const TREE_BASE_ARGS: &[&str] = &[
    "tree",
    "-p",
    "camel-cli",
    "-e",
    "features,no-dev",
    "--prefix",
    "none",
    "--locked",
];

/// Build the `cargo tree -p camel-cli -e features,no-dev --prefix none
/// --locked` command (plus `extra_args`) with the shared spawn plumbing:
/// hoisted so the "exact same invocation" property between
/// [`tree_lines`] and [`tree_fails_with`] is structural, not disciplinary.
fn tree_command(extra_args: &[&str]) -> Command {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut command = Command::new(&cargo);
    command
        .args(TREE_BASE_ARGS)
        .args(extra_args)
        // Force uncolored child output regardless of the inherited
        // environment: CI sets CARGO_TERM_COLOR=always workflow-wide, and
        // a colored tree styles the `(*)` repeat marker so the
        // plain-text normalization in `tree_lines` cannot strip it
        // (rc-k6dln).
        .env("CARGO_TERM_COLOR", "never")
        .current_dir(workspace_root());
    command
}

/// Run `cargo tree -p camel-cli -e features,no-dev --prefix none --locked`
/// (plus `extra_args`) from the workspace root and return the normalized,
/// sorted, deduplicated output lines.
fn tree_lines(extra_args: &[&str]) -> Vec<String> {
    let output = tree_command(extra_args)
        .output()
        .unwrap_or_else(|error| panic!("failed to spawn `cargo tree`: {error}"));
    if !output.status.success() {
        panic!(
            "`cargo tree` failed with {}:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines: Vec<String> = stdout.lines().map(normalize_tree_line).collect();
    lines.sort();
    lines.dedup();
    lines
}

/// Assert that `cargo tree` — the exact invocation [`tree_lines`] uses,
/// plus `extra_args` — exits non-zero and its stderr contains `fragment`:
/// a removed feature name must be rejected by cargo itself.
fn tree_fails_with(extra_args: &[&str], fragment: &str) {
    let output = tree_command(extra_args)
        .output()
        .unwrap_or_else(|error| panic!("failed to spawn `cargo tree`: {error}"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        panic!(
            "`cargo tree` unexpectedly succeeded (expected failure naming \
             `{fragment}`); status {}, stderr:\n{stderr}",
            output.status
        );
    }
    assert!(
        stderr.contains(fragment),
        "`cargo tree` failed with status {} but its stderr does not name \
         `{fragment}`; stderr:\n{stderr}",
        output.status
    );
}

/// Rust port of the fixture pipeline
/// `sed -E 's| \(/[^)]*\)||g; s| \(\*\)||g; s| \[\*\]||g'`, preceded by an
/// ANSI-escape strip: colored `cargo tree` output wraps the `(*)` marker
/// in escape sequences, and stripping them first lets the plain-text
/// `" (*)"` removal still collapse those lines onto their clean twins.
fn normalize_tree_line(line: &str) -> String {
    strip_ansi_escapes(&strip_paren_paths(line))
        .replace(" (*)", "")
        .replace(" [*]", "")
}

/// Remove ANSI escape sequences (defense in depth for environments that
/// force color despite the `CARGO_TERM_COLOR=never` pin on the child).
/// Escape sequences never occur in legitimate `cargo tree` text: package
/// names, versions, and feature names cannot contain the ESC byte.
fn strip_ansi_escapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            // CSI sequence: parameter/intermediate bytes, then a final
            // byte in 0x40..=0x7E (e.g. `\x1b[33m\x1b[2m(*)\x1b[39m\x1b[22m`).
            Some('[') => {
                for final_byte in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&final_byte) {
                        break;
                    }
                }
            }
            // Two-byte escape: the byte after ESC is consumed with it.
            Some(_) => {}
            None => break,
        }
    }
    out
}

/// Remove every ` (/<path>)` group: one space, an open paren, a path that
/// starts with `/`, and the closing paren (cargo tree directory annotations).
fn strip_paren_paths(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find(" (/") {
        match rest[start..].find(')') {
            Some(close) => {
                out.push_str(&rest[..start]);
                rest = &rest[start + close + 1..];
            }
            // No closing paren: the sed pattern would not match either.
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// The golden snapshot, sorted + deduplicated. It is produced by the same
/// normalization pipeline as [`tree_lines`] (see the module docs).
fn golden_fixture_lines() -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("default-deptree.txt");
    let raw = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("failed to read golden fixture {}: {error}", path.display())
    });
    let mut lines: Vec<String> = raw.lines().map(str::to_string).collect();
    lines.sort();
    lines.dedup();
    lines
}

/// Panic listing every offending line (one per line, indented) unless no
/// line in `lines` starts with any of `prefixes`.
fn assert_absent(lines: &[String], prefixes: &[&str], context: &str) {
    let offenders: Vec<&String> = lines
        .iter()
        .filter(|line| prefixes.iter().any(|prefix| line.starts_with(prefix)))
        .collect();
    assert!(
        offenders.is_empty(),
        "{context}:\n{}",
        offenders
            .iter()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// camel-core feature-edge lines for the language features: the `lang-*`
/// forwards and the `camel-language-*` declarations.
fn is_lang_feature_edge(line: &str) -> bool {
    line.starts_with("camel-core feature \"lang-")
        || line.starts_with("camel-core feature \"camel-language-")
}

#[test]
fn default_closure_matches_golden() {
    let actual = tree_lines(&[]);
    let expected = golden_fixture_lines();
    // Bilateral lang-feature filter: cargo tree renders feature nodes for
    // dep-declaration activation but never for feature-forwarding, so the
    // task 2.2 rework (lang features moved from declaration to forwarding)
    // erases ten golden lines mechanically with zero closure change. Both
    // comparison sides drop those edges.
    let actual_set: HashSet<&String> = actual
        .iter()
        .filter(|line| !is_lang_feature_edge(line))
        .collect();
    let expected_set: HashSet<&String> = expected
        .iter()
        .filter(|line| !is_lang_feature_edge(line))
        .collect();
    if actual_set == expected_set {
        // Package-level presence survives the feature-edge filter: the
        // camel-language-* crates must remain in the default closure
        // (minijinja via both the forward and camel-template).
        for prefix in [
            "camel-language-js v",
            "camel-language-rhai v",
            "camel-language-jsonpath v",
            "camel-language-xpath v",
            "camel-language-minijinja v",
        ] {
            assert!(
                actual.iter().any(|line| line.starts_with(prefix)),
                "default closure must still contain `{prefix}`"
            );
        }
        return;
    }
    let mut missing: Vec<&String> = expected_set.difference(&actual_set).copied().collect();
    let mut extra: Vec<&String> = actual_set.difference(&expected_set).copied().collect();
    missing.sort();
    extra.sort();
    let missing_total = missing.len();
    let extra_total = extra.len();
    let missing_block = diff_block(&missing, '-');
    let extra_block = diff_block(&extra, '+');
    panic!(
        "default feature closure drifted from the golden fixture \
         (crates/camel-cli/tests/fixtures/default-deptree.txt)\n\
         only in golden [{missing_total} total, first 20]:\n{missing_block}\n\
         only in current tree [{extra_total} total, first 20]:\n{extra_block}"
    );
}

/// Render up to 20 lines of a diff direction with `marker` prefixes.
fn diff_block(lines: &[&String], marker: char) -> String {
    lines
        .iter()
        .take(20)
        .map(|line| format!("  {marker} {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn mimalloc_stack_absent_under_all_features() {
    // Lines starting with `tikv-jemallocator` are EXPECTED under
    // --all-features and ignored: jemalloc is the surviving allocator
    // override. Only the mimalloc stack is asserted away (task 1.2's
    // mimalloc feature removal turned this formerly red test green).
    let lines = tree_lines(&["--all-features"]);
    assert_absent(
        &lines,
        &["mimalloc v", "libmimalloc-sys v"],
        "mimalloc stack must be absent under --all-features",
    );
}

#[test]
fn default_build_has_no_allocator_crate() {
    let lines = tree_lines(&[]);
    assert_absent(
        &lines,
        &[
            "mimalloc v",
            "libmimalloc-sys v",
            "tikv-jemallocator v",
            "tikv-jemalloc-sys v",
        ],
        "default build must not contain any allocator crate",
    );
}

/// The slim profile's controllable exclusion set: crates the `clidiet`
/// feature-table rework (task 2.2) must be able to drop from the
/// `--no-default-features` closure. `ariadne` is deliberately absent —
/// `camel lint` keeps it non-optional.
const GRPC_PREFIX: &str = "camel-component-grpc v";
const SLIM_FORBIDDEN_PREFIXES: &[&str] = &[
    "camel-component-kafka v",
    GRPC_PREFIX,
    "camel-component-wasm v",
    "camel-component-llm v",
    "camel-component-mcp v",
    "camel-component-mqtt v",
    "camel-component-surrealdb v",
    "camel-component-exec v",
    "camel-lsp v",
    "tower-lsp v",
    "camel-language-js v",
    "camel-language-rhai v",
    "camel-language-jsonpath v",
    "camel-language-xpath v",
    // camel-language-minijinja stays: camel-template hard-depends on it (non-optional via camel-cli + camel-bundles; deferred family, bundles-internal bridges).
];

#[test]
fn slim_closure_excludes_controllable_set() {
    let lines = tree_lines(&["--no-default-features"]);
    assert_absent(
        &lines,
        SLIM_FORBIDDEN_PREFIXES,
        "slim closure (--no-default-features) must exclude the controllable set",
    );
}

#[test]
fn slim_plus_grpc_resolves_grpc_only() {
    let lines = tree_lines(&[
        "--no-default-features",
        "--features",
        "slim-benchmarks,grpc",
    ]);
    for prefix in [GRPC_PREFIX, "tonic v"] {
        assert!(
            lines.iter().any(|line| line.starts_with(prefix)),
            "slim-benchmarks,grpc closure must contain `{prefix}`"
        );
    }
    let other_thirteen: Vec<&str> = SLIM_FORBIDDEN_PREFIXES
        .iter()
        .copied()
        .filter(|prefix| *prefix != GRPC_PREFIX)
        .collect();
    assert_absent(
        &lines,
        &other_thirteen,
        "slim-benchmarks,grpc closure must still exclude the other thirteen forbidden prefixes",
    );
}

#[test]
fn slim_alias_resolves_identically() {
    let alias_lines = tree_lines(&["--no-default-features", "--features", "slim-http"]);
    let canonical_lines = tree_lines(&["--no-default-features", "--features", "slim-benchmarks"]);
    let alias_set: HashSet<&str> = alias_lines.iter().map(String::as_str).collect();
    let canonical_set: HashSet<&str> = canonical_lines.iter().map(String::as_str).collect();
    assert_eq!(
        alias_set, canonical_set,
        "slim-http alias closure must resolve identically to slim-benchmarks"
    );
    assert_absent(
        &alias_lines,
        SLIM_FORBIDDEN_PREFIXES,
        "slim-http alias closure must exclude the controllable set",
    );
}

#[test]
fn kafka_feature_table_implies_capability() {
    // The `dynamic-linking => kafka` implication is asserted at the
    // feature-table level: feature-forwarding edges never render in
    // cargo tree (see `default_closure_matches_golden`), so a
    // closure-based implication test would be vacuous.
    let manifest = fs::read_to_string(workspace_root().join("crates/camel-cli/Cargo.toml"))
        .expect("failed to read crates/camel-cli/Cargo.toml");
    const DYNAMIC_LINKING_LINE: &str =
        r#"dynamic-linking = ["kafka", "camel-component-kafka/dynamic-linking"]"#;
    // The `[features]` section stretches from its header to the next
    // section header (a line starting with `[`). The kafka surface is
    // exactly two features — `kafka` and `dynamic-linking` — so the
    // section must contain precisely those two lines and no other
    // `kafka`- or `dynamic-linking`-prefixed line (a reappearing
    // removed name would break the exact-set assertion).
    let mut in_features = false;
    let mut kafka_lines: Vec<&str> = Vec::new();
    for line in manifest.lines() {
        if line.starts_with('[') {
            in_features = line == "[features]";
            continue;
        }
        if in_features && (line.starts_with("kafka") || line.starts_with("dynamic-linking")) {
            kafka_lines.push(line);
        }
    }
    assert_eq!(
        kafka_lines,
        vec![
            r#"kafka = ["dep:camel-component-kafka", "camel-bundles/kafka", "camel-component-kafka/cmake-build"]"#,
            DYNAMIC_LINKING_LINE,
        ],
        "the kafka feature surface must be exactly `kafka` and `dynamic-linking`"
    );
}

#[test]
fn dynamic_linking_closure_resolves_kafka() {
    let lines = tree_lines(&["--no-default-features", "--features", "dynamic-linking"]);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("camel-component-kafka")),
        "dynamic-linking closure must contain `camel-component-kafka`"
    );
    let remaining: Vec<&str> = SLIM_FORBIDDEN_PREFIXES
        .iter()
        .copied()
        .filter(|prefix| *prefix != "camel-component-kafka v")
        .collect();
    assert_absent(
        &lines,
        &remaining,
        "dynamic-linking closure must still exclude the remaining controllable set",
    );
}

#[test]
fn removed_kafka_feature_names_rejected() {
    tree_fails_with(&["--features", "cmake-build"], "cmake-build");
    tree_fails_with(&["--features", "kafka-static"], "kafka-static");
}
