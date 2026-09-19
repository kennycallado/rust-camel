//! Ratchet lint for production `CancellationToken::new()` in components.
//!
//! bd rc-pu2s (from rc-ibwa, ADR-0043 pipeline cancellation tree): a consumer's
//! task-lifetime token must come from `ConsumerContext::cancel_token()` — a
//! clone of the Runtime-owned token cancelled on route stop. Per-request work
//! derives from an existing token via `.child_token()`. A fresh
//! `CancellationToken::new()` in production component code is therefore a
//! lifecycle root that gets ratchet scrutiny.
//!
//! Enforcement is a soft count ratchet, NOT a build break for existing code:
//! `scripts/xtask/ratchet-cancel-tokens.max` holds the maximum allowed
//! production count. The number may only decrease (monotone). Lowering it is
//! the ratchet action; raising it is a review-visible regression signal.

use crate::scan_state::{self, Brace, ScanState};
use std::path::{Component, Path};
use walkdir::WalkDir;

/// A production `CancellationToken::new()` site found under
/// `crates/components/*/src/**`.
#[derive(Debug, PartialEq)]
pub struct CancelTokenSite {
    pub file: String,
    pub line: usize,
    pub snippet: String,
    /// Remedy text, per scope class: consumer-lifetime vs lifecycle root.
    pub remedy: String,
}

/// Ratchet outcome for the command layer to render.
#[derive(Debug, PartialEq)]
pub struct CancelTokensReport {
    /// Every production `CancellationToken::new()` site (the raw count).
    pub sites: Vec<CancelTokenSite>,
    /// Ceiling read from the ratchet file.
    pub max: usize,
}

/// Name of the ratchet file inside `scripts/xtask/`.
pub const RATCHET_FILE: &str = "ratchet-cancel-tokens.max";

/// Remedy for sites naming a `cancel_token` (the `ConsumerContext` contract
/// name): consumer-lifetime scope, the banned pattern from rc-ibwa.
fn consumer_lifetime_remedy() -> String {
    "consumer-lifetime token must come from ConsumerContext::cancel_token() \
     (ADR-0043, rc-ibwa); per-request fan-out: ctx.cancel_token().child_token())"
        .to_string()
}

/// Remedy for any other fresh-token root (server death signals, supervision
/// teardown, stream watchdog roots).
fn lifecycle_root_remedy() -> String {
    "fresh lifecycle root: derive from an existing token via .child_token() \
     when possible; otherwise lower the count elsewhere or justify raising \
     scripts/xtask/ratchet-cancel-tokens.max (review-visible)"
        .to_string()
}

/// Read the ratchet ceiling from `scripts/xtask/ratchet-cancel-tokens.max`.
/// First non-empty, non-`#` line must be a non-negative integer. A missing
/// file is an error: an accidentally deleted ratchet must not silently pass.
fn read_ratchet_max(workspace_root: &Path) -> Result<usize, String> {
    let path = workspace_root
        .join("scripts")
        .join("xtask")
        .join(RATCHET_FILE);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(|l| l.parse::<usize>().ok())
        .ok_or_else(|| format!("{} must contain one integer line", path.display()))
}

/// Scan `crates/components/*/src/**` production code for
/// `CancellationToken::new()` and compare against the ratchet ceiling.
pub fn lint_cancel_tokens(workspace_root: &Path) -> Result<CancelTokensReport, String> {
    let max = read_ratchet_max(workspace_root)?;

    let mut sites = Vec::new();
    for entry in WalkDir::new(workspace_root.join("crates").join("components"))
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        collect_sites_from_path(entry.path(), &mut sites);
    }
    sites.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    Ok(CancelTokensReport { sites, max })
}

/// Scan one candidate file (no-op for non-`.rs`, test files, or files
/// outside a `src` tree). Extracted for unit-testability.
fn collect_sites_from_path(path: &Path, sites: &mut Vec<CancelTokenSite>) {
    if path.extension().and_then(|e| e.to_str()) != Some("rs") {
        return;
    }
    if is_test_file(path) {
        return;
    }
    if !path
        .components()
        .any(|c| c == Component::Normal("src".as_ref()))
    {
        return;
    }
    if path
        .components()
        .any(|c| c == Component::Normal("target".as_ref()))
    {
        return;
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let file = path.to_string_lossy().to_string();
    collect_sites_from_src(&content, &file, sites);
}

/// Core scanner: count `CancellationToken::new(` in production scope —
/// outside test-scope attribute blocks (`#[cfg(test)]`, `#[test]`,
/// `#[tokio::test]`, and `#[cfg(all/any(..., test, ...))]` conjunctions)
/// and full-line comments. Brace-depth tracking uses the shared
/// literal/comment-aware scanner (`scan_state`, extracted from
/// `lint_unwrap_src` by rc-drcgf): braces inside strings, chars, raw
/// strings, and comments do not skew test-scope tracking.
///
/// Known limitation (same class as the other line-based lints): trailing
/// `// ...` comments and string-literal mentions on a code line are
/// counted — site detection is a line-level `contains`, not state-aware.
fn collect_sites_from_src(src: &str, file: &str, sites: &mut Vec<CancelTokenSite>) {
    let lines: Vec<&str> = src.lines().collect();
    let mut state = ScanState::Normal;
    let mut pending_test_attr = false;
    let mut test_scope_entry_depth: Option<i32> = None;
    let mut brace_depth: i32 = 0;

    for (line_idx, raw_line) in lines.iter().enumerate() {
        let trimmed = raw_line.trim();

        if test_scope_entry_depth.is_none() && is_test_attr_line(trimmed) {
            pending_test_attr = true;
        }

        scan_state::scan_line(trimmed, &mut state, &mut |brace| match brace {
            Brace::Open => {
                brace_depth += 1;
                if pending_test_attr && test_scope_entry_depth.is_none() {
                    test_scope_entry_depth = Some(brace_depth - 1);
                    pending_test_attr = false;
                }
            }
            Brace::Close => {
                brace_depth -= 1;
                if let Some(entry) = test_scope_entry_depth
                    && brace_depth <= entry
                {
                    test_scope_entry_depth = None;
                }
            }
        });

        // An attribute on a non-block item (`#[test] fn f();`-style) never
        // opens a scope.
        if pending_test_attr && test_scope_entry_depth.is_none() && trimmed.contains(';') {
            pending_test_attr = false;
        }

        if pending_test_attr || test_scope_entry_depth.is_some() {
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }

        if trimmed.contains("CancellationToken::new(") {
            let remedy = if trimmed.contains("cancel_token") {
                consumer_lifetime_remedy()
            } else {
                lifecycle_root_remedy()
            };
            sites.push(CancelTokenSite {
                file: file.to_string(),
                line: line_idx + 1,
                snippet: trimmed.to_string(),
                remedy,
            });
        }
    }
}

/// True for attribute lines that open a test scope: `#[cfg(test)]`,
/// `#[test]`, `#[tokio::test]`, and `#[cfg(all(...))]` / `#[cfg(any(...))]`
/// conjunctions whose DIRECT predicate list contains `test` (e.g.
/// `#[cfg(all(test, feature = "llm"))]`, used by camel-component-api).
/// Nested predicates do not count: `#[cfg(not(test))]` compiles its body
/// in non-test builds and stays production scope.
fn is_test_attr_line(trimmed: &str) -> bool {
    trimmed.starts_with("#[cfg(test)]")
        || trimmed.starts_with("#[test]")
        || trimmed.starts_with("#[tokio::test]")
        || cfg_conjunction_has_direct_test_predicate(trimmed)
}

/// `#[cfg(all/any(...))]` where a top-level predicate of the conjunction
/// is exactly `test`. Nested conjunctions (e.g. `all(test, any(...))`)
/// still count — the `test` predicate is direct; `not(test)` does not.
fn cfg_conjunction_has_direct_test_predicate(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("#[cfg(") else {
        return false;
    };
    let Some(body) = rest
        .strip_prefix("all(")
        .or_else(|| rest.strip_prefix("any("))
    else {
        return false;
    };
    // The conjunction body runs to its matching close paren; predicates
    // are its top-level comma-separated segments.
    let mut depth = 1usize;
    let mut end = body.len();
    for (i, ch) in body.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    body[..end].split(',').any(|pred| pred.trim() == "test")
}

/// Same test-file rule as the other lints (`lint_unwrap`, `lint_log_levels`):
/// `tests/` dirs, `test_*`, `*_test.rs`, `*_tests.rs`, `tests.rs`, `build.rs`.
fn is_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    path.components()
        .any(|c| c == Component::Normal("tests".as_ref()))
        || name.starts_with("test_")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || name == "tests.rs"
        || name == "build.rs"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// Build a throwaway workspace with the given files plus a seeded
    /// ratchet file. Mirrors `tmp_workspace_log` in main.rs.
    fn tmp_workspace_tokens(files: &[(&str, &str)], max: usize) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xtask-cancel-tokens-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        for (rel_path, content) in files {
            let full = dir.join(rel_path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, content).unwrap();
        }
        fs::create_dir_all(dir.join("crates").join("components")).unwrap();
        fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        let xtask = dir.join("scripts").join("xtask");
        fs::create_dir_all(&xtask).unwrap();
        fs::write(xtask.join(RATCHET_FILE), format!("# ratchet\n{max}\n")).unwrap();
        dir
    }

    #[test]
    fn detects_consumer_lifetime_new_in_production() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/consumer.rs",
                "fn build() -> Consumer {\n    Consumer {\n        cancel_token: CancellationToken::new(),\n    }\n}\n",
            )],
            1,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert_eq!(report.sites.len(), 1, "got: {:?}", report.sites);
        let site = &report.sites[0];
        assert!(
            site.file
                .ends_with("crates/components/fake-comp/src/consumer.rs")
        );
        assert_eq!(site.line, 3);
        assert!(site.remedy.contains("ctx.cancel_token()"));
        assert!(site.remedy.contains("child_token"));
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn classifies_fresh_root_site_with_root_remedy() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/registry.rs",
                "fn spawn() {\n    let server_exited = CancellationToken::new();\n}\n",
            )],
            1,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert_eq!(report.sites.len(), 1);
        assert!(report.sites[0].remedy.contains("lifecycle root"));
        assert!(report.sites[0].remedy.contains(RATCHET_FILE));
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn ignores_child_token_pattern() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/consumer.rs",
                "fn start(ctx: &ConsumerContext) {\n    let t = ctx.cancel_token().child_token();\n    let u = self.cancel_token.child_token();\n}\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn ignores_cfg_test_scope() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/consumer.rs",
                "fn prod() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() {\n        let x = CancellationToken::new();\n    }\n}\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn counts_site_after_closed_cfg_test_block() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "fn prod_a() { let _ = CancellationToken::new(); }\n\n#[cfg(test)]\nmod tests {\n    fn t() { let _ = CancellationToken::new(); }\n}\n\nfn prod_b() { let _ = CancellationToken::new(); }\n",
            )],
            2,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert_eq!(report.sites.len(), 2, "got: {:?}", report.sites);
        assert_eq!(
            report.sites.iter().map(|s| s.line).collect::<Vec<_>>(),
            vec![1, 8]
        );
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn ignores_test_named_files_and_tests_dir() {
        let ws = tmp_workspace_tokens(
            &[
                (
                    "crates/components/fake-comp/src/consumer_tests.rs",
                    "fn f() { let _ = CancellationToken::new(); }\n",
                ),
                (
                    "crates/components/fake-comp/src/tests.rs",
                    "fn f() { let _ = CancellationToken::new(); }\n",
                ),
                (
                    "crates/components/fake-comp/tests/integration.rs",
                    "fn f() { let _ = CancellationToken::new(); }\n",
                ),
            ],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn ignores_comments() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "/// let cancel = CancellationToken::new();\n// let x = CancellationToken::new();\nfn f() {}\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn scans_only_components_crates() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/camel-core/src/lib.rs",
                "fn f() { let _ = CancellationToken::new(); }\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn ignores_non_src_and_target_paths() {
        let ws = tmp_workspace_tokens(
            &[
                (
                    "crates/components/fake-comp/examples/demo.rs",
                    "fn f() { let _ = CancellationToken::new(); }\n",
                ),
                (
                    "crates/components/fake-comp/target/debug/x.rs",
                    "fn f() { let _ = CancellationToken::new(); }\n",
                ),
            ],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn ratchet_exceeded_when_count_over_max() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "fn a() { let _ = CancellationToken::new(); }\nfn b() { let _ = CancellationToken::new(); }\n",
            )],
            1,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.len() > report.max);
        fs::remove_dir_all(&ws).unwrap();
    }

    #[test]
    fn missing_ratchet_file_is_error() {
        let dir = std::env::temp_dir().join(format!(
            "xtask-cancel-tokens-nomax-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(dir.join("crates").join("components")).unwrap();
        let result = lint_cancel_tokens(&dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains(RATCHET_FILE));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_ratchet_file_is_error() {
        let dir = std::env::temp_dir().join(format!(
            "xtask-cancel-tokens-badmax-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(dir.join("crates").join("components")).unwrap();
        let xtask = dir.join("scripts").join("xtask");
        fs::create_dir_all(&xtask).unwrap();
        fs::write(xtask.join(RATCHET_FILE), "# only comments\n").unwrap();
        assert!(lint_cancel_tokens(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A `#[tokio::test]` fn outside any `#[cfg(test)]` mod is test scope too.
    #[test]
    fn ignores_tokio_test_outside_cfg_test_mod() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[tokio::test]\nasync fn t() {\n    let _ = CancellationToken::new();\n}\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// cfg(test)-inside-cfg(test): the outer scope already excludes the site,
    /// and the tracker must return to production scope only after the OUTER
    /// block closes.
    #[test]
    fn ignores_nested_cfg_test_blocks() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[cfg(test)]\nmod outer {\n    #[cfg(test)]\n    mod inner {\n        fn t() { let _ = CancellationToken::new(); }\n    }\n\n    fn also_test() { let _ = CancellationToken::new(); }\n}\n\nfn prod() {}\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// Documented limitation: a trailing comment on a code line IS counted
    /// (line-based scan, same as the other xtask lints). If this ever breaks
    /// a real tree, the escape hatch is rewording the comment — the ratchet
    /// ceiling is not silently raised for it.
    #[test]
    fn counts_trailing_comment_reference() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "fn f() {} // prefer CancellationToken::new() never here\n",
            )],
            1,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert_eq!(report.sites.len(), 1, "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// rc-drcgf: a `}` inside a string literal must not close the test
    /// scope early. With the naive brace counter this line exited
    /// `mod tests` mid-line and every following test-fn site was counted
    /// as production (fail-closed overcount).
    #[test]
    fn string_brace_does_not_exit_test_scope_early() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[cfg(test)]\nmod tests {\n    fn t() { let s = \"}\"; let _ = CancellationToken::new(); }\n    fn u() { let _ = CancellationToken::new(); }\n}\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// rc-drcgf: a `{` inside a full-line comment inside a test mod must
    /// not pin the brace depth above the entry depth. With the naive
    /// counter the scope never closed and later production sites were
    /// silently skipped (fail-open undercount).
    #[test]
    fn comment_brace_does_not_trap_test_scope_open() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[cfg(test)]\nmod tests {\n    // { stray brace drifts naive depth\n    fn t() { let _ = CancellationToken::new(); }\n}\n\nfn prod() { let _ = CancellationToken::new(); }\n",
            )],
            1,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert_eq!(report.sites.len(), 1, "got: {:?}", report.sites);
        assert_eq!(report.sites[0].line, 7);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// rc-drcgf: same premature-exit failure mode as strings, via a char
    /// literal `'}'`.
    #[test]
    fn char_literal_brace_does_not_exit_test_scope_early() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[cfg(test)]\nmod tests {\n    fn t() { let c = '}'; let _ = CancellationToken::new(); }\n    fn u() { let _ = CancellationToken::new(); }\n}\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// rc-drcgf: raw strings hide braces from the naive counter too.
    #[test]
    fn raw_string_brace_does_not_exit_test_scope_early() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[cfg(test)]\nmod tests {\n    fn t() { let s = r#\"}\"#; let _ = CancellationToken::new(); }\n    fn u() { let _ = CancellationToken::new(); }\n}\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// rc-drcgf: `#[cfg(all(test, ...))]` opens a test scope when `test`
    /// is a direct predicate of the conjunction (shape used in
    /// camel-component-api, camel-cxf). Previously treated as production.
    #[test]
    fn recognizes_cfg_all_test_conjunction_as_test_scope() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[cfg(all(test, feature = \"llm\"))]\nmod gated_tests {\n    fn t() { let _ = CancellationToken::new(); }\n}\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// rc-drcgf: `#[cfg(any(test, ...))]` — same rule as `all`.
    #[test]
    fn recognizes_cfg_any_test_conjunction_as_test_scope() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[cfg(any(test, feature = \"native\"))]\nmod gated_tests {\n    fn t() { let _ = CancellationToken::new(); }\n}\n",
            )],
            0,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert!(report.sites.is_empty(), "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// Control: `not(test)` inverts the gate — the body is compiled in
    /// NON-test builds, so it must stay production scope.
    #[test]
    fn cfg_not_test_stays_production_scope() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[cfg(not(test))]\nfn offline_only() { let _ = CancellationToken::new(); }\n",
            )],
            1,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert_eq!(report.sites.len(), 1, "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// Control: an all/any conjunction WITHOUT a direct `test` predicate
    /// stays production scope.
    #[test]
    fn cfg_conjunction_without_test_predicate_stays_production() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[cfg(all(feature = \"a\", feature = \"b\"))]\nfn feats() { let _ = CancellationToken::new(); }\n",
            )],
            1,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert_eq!(report.sites.len(), 1, "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// Control: `test` nested inside a sub-conjunction (e.g.
    /// `all(feature, any(test, ...))`) is NOT a direct predicate — the
    /// gate does not vanish in test builds, so the body stays production
    /// scope.
    #[test]
    fn cfg_nested_test_predicate_stays_production() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "#[cfg(all(feature = \"a\", any(test, feature = \"b\")))]\nfn feats() { let _ = CancellationToken::new(); }\n",
            )],
            1,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert_eq!(report.sites.len(), 1, "got: {:?}", report.sites);
        fs::remove_dir_all(&ws).unwrap();
    }

    /// Control: balanced braces inside string literals in production code
    /// do not shift depth, and real production sites around a test mod
    /// are still counted (real-drift detection must survive the fix).
    #[test]
    fn counts_prod_sites_around_test_mod_with_literals() {
        let ws = tmp_workspace_tokens(
            &[(
                "crates/components/fake-comp/src/lib.rs",
                "fn a() { let _ = CancellationToken::new(); }\n\n#[cfg(test)]\nmod tests {\n    fn t() { let s = \"{}{}\"; let _ = CancellationToken::new(); }\n}\n\nfn b() { let _ = CancellationToken::new(); }\n",
            )],
            2,
        );
        let report = lint_cancel_tokens(&ws).unwrap();
        assert_eq!(report.sites.len(), 2, "got: {:?}", report.sites);
        assert_eq!(
            report.sites.iter().map(|s| s.line).collect::<Vec<_>>(),
            vec![1, 8]
        );
        fs::remove_dir_all(&ws).unwrap();
    }
}
