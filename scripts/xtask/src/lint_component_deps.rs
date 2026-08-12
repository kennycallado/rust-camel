//! Lint: scan component crate source for `camel_core::` references outside
//! `#[cfg(test)]` modules. Enforces the hexagonal-architecture invariant:
//! components (adapters) must depend on ports (`camel-component-api`), never
//! on concrete adapter types from `camel-core`.
//!
//! Established by rc-x014: WasmComponent took `Arc<Mutex<camel_core::Registry>>`
//! in its constructor, creating a cfg(test) dual-compilation type mismatch
//! when camel-core dev-depped the wasm crate for catalog tests. The fix
//! replaced the concrete type with `Arc<dyn ComponentContext>` (the port).
//! This lint prevents regression.

use crate::Violation;
use regex::Regex;
use std::path::{Component, Path};

/// Scan all `.rs` files under `crates/components/` for `camel_core::`
/// references that fall OUTSIDE `#[cfg(test)]` / `#[test]` blocks.
///
/// Uses the same lexical brace-depth tracking as `lint_single_source` to
/// skip test-scoped code. Comment lines are also skipped.
pub fn lint_component_deps(workspace_root: &Path) -> Result<Vec<Violation>, String> {
    let re = Regex::new(r"\bcamel_core::").expect("valid regex"); // allow-unwrap

    let components_dir = workspace_root.join("crates").join("components");

    if !components_dir.exists() {
        return Ok(Vec::new());
    }

    let mut violations = Vec::new();

    for entry in walkdir::WalkDir::new(&components_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }

        if !path
            .components()
            .any(|c| c == Component::Normal("src".as_ref()))
        {
            continue;
        }

        if path
            .components()
            .any(|c| c == Component::Normal("target".as_ref()))
        {
            continue;
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;

        let path_str = path.to_string_lossy().to_string();
        let file_violations = scan_file_for_camel_core_deps(&content, &path_str, &re);
        violations.extend(file_violations);
    }

    Ok(violations)
}

/// Scan a single `.rs` file's content for `camel_core::` outside test scopes.
fn scan_file_for_camel_core_deps(src: &str, file_path: &str, re: &Regex) -> Vec<Violation> {
    let lines: Vec<&str> = src.lines().collect();

    let mut pending_test_attr = false;
    let mut test_scope_entry_depth: Option<i32> = None;
    let mut brace_depth: i32 = 0;
    let mut violations = Vec::new();

    for (line_idx, raw_line) in lines.iter().enumerate() {
        let trimmed = raw_line.trim();

        if test_scope_entry_depth.is_none()
            && (trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[test]"))
        {
            pending_test_attr = true;
        }

        let entering_test_scope = pending_test_attr && test_scope_entry_depth.is_none();

        for ch in trimmed.chars() {
            match ch {
                '{' => {
                    brace_depth += 1;
                    if pending_test_attr && test_scope_entry_depth.is_none() {
                        test_scope_entry_depth = Some(brace_depth - 1);
                        pending_test_attr = false;
                    }
                }
                '}' => {
                    brace_depth -= 1;
                    if let Some(entry) = test_scope_entry_depth
                        && brace_depth <= entry
                    {
                        test_scope_entry_depth = None;
                    }
                }
                _ => {}
            }
        }

        if pending_test_attr && test_scope_entry_depth.is_none() && trimmed.contains(';') {
            pending_test_attr = false;
        }

        if pending_test_attr || entering_test_scope || test_scope_entry_depth.is_some() {
            continue;
        }

        // Skip comment lines (line comments and block-comment continuations).
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
            continue;
        }

        if re.is_match(raw_line) {
            violations.push(Violation {
                file: file_path.to_string(),
                line: line_idx + 1,
                snippet: raw_line.to_string(),
            });
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_camel_core_ref_in_production_code() {
        let src = r#"
            use camel_core::Registry;
            fn production_code() {
                let r: camel_core::Registry = Registry::new();
            }
        "#;
        let re = Regex::new(r"\bcamel_core::").unwrap(); // allow-unwrap
        let violations = scan_file_for_camel_core_deps(src, "test.rs", &re);
        assert_eq!(violations.len(), 2);
    }

    #[test]
    fn allows_camel_core_inside_cfg_test_module() {
        let src = r#"
            fn production_code() {}

            #[cfg(test)]
            mod tests {
                use super::*;
                use camel_core::Registry;

                #[test]
                fn test_something() {
                    let _r = Registry::new();
                }
            }
        "#;
        let re = Regex::new(r"\bcamel_core::").unwrap(); // allow-unwrap
        let violations = scan_file_for_camel_core_deps(src, "test.rs", &re);
        assert_eq!(violations.len(), 0);
    }

    #[test]
    fn skips_comment_lines() {
        let src = r#"
            // use camel_core::Registry;
            /// Doc: see [`camel_core::Registry`]
            fn production_code() {}
        "#;
        let re = Regex::new(r"\bcamel_core::").unwrap(); // allow-unwrap
        let violations = scan_file_for_camel_core_deps(src, "test.rs", &re);
        assert_eq!(violations.len(), 0);
    }

    #[test]
    fn flags_after_closing_test_module() {
        let src = r#"
            #[cfg(test)]
            mod tests {
                use camel_core::Registry;
            }

            fn prod() {
                let _r = camel_core::Registry::new();
            }
        "#;
        let re = Regex::new(r"\bcamel_core::").unwrap(); // allow-unwrap
        let violations = scan_file_for_camel_core_deps(src, "test.rs", &re);
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn clean_file_has_no_violations() {
        let src = r#"
            use camel_component_api::Component;
            fn do_stuff() {
                let x = 42;
            }
        "#;
        let re = Regex::new(r"\bcamel_core::").unwrap(); // allow-unwrap
        let violations = scan_file_for_camel_core_deps(src, "test.rs", &re);
        assert_eq!(violations.len(), 0);
    }
}
