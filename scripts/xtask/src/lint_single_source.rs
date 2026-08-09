//! Lint: scan component crate source for `UriOption::new` calls outside
//! `#[cfg(test)]` modules. Enforces the single-source-of-truth invariant
//! established by the `consolidate-uri-metadata` ADR-0041 amendment:
//! metadata MUST be macro-derived; hand-written `UriOption::new` lists
//! are forbidden in production code.

use crate::Violation;
use regex::Regex;
use std::path::{Component, Path};

/// Scan all `.rs` files under `crates/components/` for `UriOption::new` calls
/// that fall OUTSIDE `#[cfg(test)]` / `mod tests` blocks.
///
/// Uses the same lexical brace-depth tracking as `lint_unwrap` to skip
/// test-scoped code. Only reports violations in `src/` files (not `tests/`).
pub fn lint_single_source(workspace_root: &Path) -> Result<Vec<Violation>, String> {
    let uri_re = Regex::new(r"UriOption::new").expect("valid regex"); // allow-unwrap

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

        // Only .rs files
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }

        // Only src/ files (not tests/)
        if !path
            .components()
            .any(|c| c == Component::Normal("src".as_ref()))
        {
            continue;
        }

        // Skip target/ directories
        if path
            .components()
            .any(|c| c == Component::Normal("target".as_ref()))
        {
            continue;
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;

        let path_str = path.to_string_lossy().to_string();
        let file_violations = scan_file_for_uri_option_new(&content, &path_str, &uri_re);
        violations.extend(file_violations);
    }

    Ok(violations)
}

/// Scan a single `.rs` file's content for `UriOption::new` outside test scopes.
fn scan_file_for_uri_option_new(src: &str, file_path: &str, uri_re: &Regex) -> Vec<Violation> {
    let lines: Vec<&str> = src.lines().collect();

    let mut pending_test_attr = false;
    let mut test_scope_entry_depth: Option<i32> = None;
    let mut brace_depth: i32 = 0;
    let mut violations = Vec::new();

    for (line_idx, raw_line) in lines.iter().enumerate() {
        let trimmed = raw_line.trim();

        // Detect test attributes only when not already inside a test scope.
        if test_scope_entry_depth.is_none()
            && (trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[test]"))
        {
            pending_test_attr = true;
        }

        let entering_test_scope = pending_test_attr && test_scope_entry_depth.is_none();

        // Brace counting (same state-machine approach as lint_unwrap).
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

        // Clear pending_test_attr if no brace was opened on a semicolon line.
        if pending_test_attr && test_scope_entry_depth.is_none() && trimmed.contains(';') {
            pending_test_attr = false;
        }

        // Skip: the attribute line itself, the line that opens a test scope,
        // and all lines inside a test scope.
        if pending_test_attr || entering_test_scope || test_scope_entry_depth.is_some() {
            continue;
        }

        // Check for UriOption::new on this line.
        if uri_re.is_match(raw_line) {
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
    fn flags_uri_option_new_outside_test_module() {
        let src = r#"
            fn production_code() {
                let opt = UriOption::new("foo", "desc", OptionKind::String);
            }
        "#;
        let re = Regex::new(r"UriOption::new").unwrap(); // allow-unwrap
        let violations = scan_file_for_uri_option_new(src, "test.rs", &re);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].snippet.contains("UriOption::new"));
    }

    #[test]
    fn allows_uri_option_new_inside_cfg_test_module() {
        let src = r#"
            fn production_code() {
                // nothing here
            }

            #[cfg(test)]
            mod tests {
                use super::*;

                #[test]
                fn test_something() {
                    let opt = UriOption::new("foo", "desc", OptionKind::String);
                }
            }
        "#;
        let re = Regex::new(r"UriOption::new").unwrap(); // allow-unwrap
        let violations = scan_file_for_uri_option_new(src, "test.rs", &re);
        assert_eq!(violations.len(), 0);
    }

    #[test]
    fn flags_after_closing_test_module() {
        let src = r#"
            #[cfg(test)]
            mod tests {
                #[test]
                fn helper() {
                    let _x = UriOption::new("a", "b", OptionKind::String);
                }
            }

            fn prod() {
                let opt = UriOption::new("prod", "desc", OptionKind::String);
            }
        "#;
        let re = Regex::new(r"UriOption::new").unwrap(); // allow-unwrap
        let violations = scan_file_for_uri_option_new(src, "test.rs", &re);
        // One inside tests (skipped), one outside (flagged).
        assert_eq!(violations.len(), 1);
        assert!(violations[0].snippet.contains("prod"));
    }

    #[test]
    fn allows_uri_option_new_inside_test_fn() {
        let src = r#"
            #[test]
            fn my_test() {
                let opt = UriOption::new("t", "d", OptionKind::Bool);
            }
        "#;
        let re = Regex::new(r"UriOption::new").unwrap(); // allow-unwrap
        let violations = scan_file_for_uri_option_new(src, "test.rs", &re);
        assert_eq!(violations.len(), 0);
    }

    #[test]
    fn clean_file_has_no_violations() {
        let src = r#"
            fn do_stuff() {
                let x = 42;
                println!("hello");
            }
        "#;
        let re = Regex::new(r"UriOption::new").unwrap(); // allow-unwrap
        let violations = scan_file_for_uri_option_new(src, "test.rs", &re);
        assert_eq!(violations.len(), 0);
    }

    #[test]
    fn handles_nested_test_modules() {
        let src = r#"
            #[cfg(test)]
            mod tests {
                mod nested {
                    use super::super::*;

                    #[test]
                    fn deep() {
                        let _ = UriOption::new("nested", "d", OptionKind::String);
                    }
                }
            }

            fn outside() {
                let _ = UriOption::new("outside", "d", OptionKind::String);
            }
        "#;
        let re = Regex::new(r"UriOption::new").unwrap(); // allow-unwrap
        let violations = scan_file_for_uri_option_new(src, "test.rs", &re);
        assert_eq!(violations.len(), 1);
    }
}
