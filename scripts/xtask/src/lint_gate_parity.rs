//! Lint: enforce parity between the `AGENTS.md` `## QUALITY GATES` registry
//! and the workflow steps that mirror those commands
//! (`lint-gate-parity`, ci-clippy-parity / bd rc-2s1vm).
//!
//! The registry is the single source of truth for the local sweep
//! (`scripts/gates-local.sh` parses it), but two gates also appear,
//! hand-copied, as workflow steps: the `clippy` gate in
//! `.github/workflows/ci.yml` and the `doc-build` gate in
//! `.github/workflows/docs.yml`. A hand-copied command drifts silently —
//! rc-2s1vm found the ci.yml clippy step missing `--all-features` and
//! `--all-targets` relative to the registry.
//!
//! This lint parses both sides as text (no YAML or shell dependency), maps
//! each checked gate to its workflow file/job/step through a const table,
//! normalizes each run block into ordered statements, and compares them.
//! A `NAME=value` prefix on a registry statement (the `doc-build` gate's
//! `RUSTDOCFLAGS="-D warnings"`) is compared against the mapped step's
//! effective environment: step `env:` over job `env:` over workflow-level
//! `env:` over unset.
//!
//! Static only: no network, no cargo invocations, no new dependencies.

use std::path::Path;

/// Gates checked for parity: registry gate name, workflow file, job id,
/// step name. Only gates named here are checked; the `lint-gate-parity`
/// gate itself is deliberately absent, so the lint does not demand a
/// workflow step for itself.
const PARITY_GATES: [(&str, &str, &str, &str); 2] = [
    ("clippy", ".github/workflows/ci.yml", "quality", "clippy"),
    (
        "doc-build",
        ".github/workflows/docs.yml",
        "validate",
        "Build Rustdoc",
    ),
];

/// Number of gates the parity check covers. The dispatch layer prints this
/// in its success line, so the count cannot drift from `PARITY_GATES`.
pub(crate) fn parity_gate_count() -> usize {
    PARITY_GATES.len()
}

/// One parsed registry entry: gate name plus its raw `run:` block text.
struct Gate {
    name: String,
    run: String,
}

/// One mapped workflow step: its run text plus the three env scopes that
/// can supply a variable to it.
struct Step {
    run: String,
    step_env: Vec<(String, String)>,
    job_env: Vec<(String, String)>,
    workflow_env: Vec<(String, String)>,
}

impl Step {
    /// Effective value for `name` and the scope it came from. Step-level
    /// entries beat job-level entries, which beat workflow-level entries.
    fn resolve_env(&self, name: &str) -> (Option<String>, &'static str) {
        let scopes: [(&[(String, String)], &str); 3] = [
            (&self.step_env, "step env"),
            (&self.job_env, "job env"),
            (&self.workflow_env, "workflow env"),
        ];
        for (entries, source) in scopes {
            if let Some((_, value)) = entries.iter().rev().find(|(key, _)| key == name) {
                return (Some(value.clone()), source);
            }
        }
        (None, "unset")
    }
}

/// Leading-whitespace width of a line.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `name: value` -> `name`. Used to match job keys and `env:` blocks.
fn key_of(trimmed: &str) -> &str {
    trimmed
        .split_once(':')
        .map(|(key, _)| key.trim())
        .unwrap_or(trimmed)
}

/// Drop a YAML inline comment: `name: x   # note` -> `x`. Same rule as
/// `scripts/gates-local.sh`'s `strip_comment`: only `  #` starts a comment.
fn strip_comment(value: &str) -> &str {
    match value.find("  #") {
        Some(marker) => value[..marker].trim_end(),
        None => value.trim_end(),
    }
}

/// Strip ONE pair of matching surrounding quotes. YAML quoting is syntax,
/// so the parsed value carries none; a quoted value inside a parsed string
/// is content and must survive. Never recursive.
fn strip_one_quote_pair(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let double = bytes[0] == b'"' && bytes[value.len() - 1] == b'"';
        let single = bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'';
        if double || single {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// Lines belonging to a nested block: blank lines are skipped and the
/// block ends at the first non-blank line indented at or below
/// `parent_indent`.
fn block_span<'a>(lines: &[&'a str], start: usize, parent_indent: usize) -> Vec<&'a str> {
    let mut span = Vec::new();
    for line in lines.iter().skip(start) {
        if line.trim().is_empty() {
            continue;
        }
        if indent_of(line) <= parent_indent {
            break;
        }
        span.push(*line);
    }
    span
}

/// Parse a YAML mapping block (`KEY: value` lines) holding environment
/// entries. `key_indent` pins the `env:` line's indentation so a
/// workflow-level lookup cannot pick up a job-level block. Values keep
/// their YAML spelling (quotes included) and are unquoted at compare time.
fn env_entries(lines: &[&str], key_indent: usize) -> Vec<(String, String)> {
    let Some(position) = lines
        .iter()
        .position(|line| indent_of(line) == key_indent && key_of(line.trim()) == "env")
    else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for line in block_span(lines, position + 1, key_indent) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        entries.push((
            key.trim().to_string(),
            strip_comment(value).trim().to_string(),
        ));
    }
    entries
}

/// Locate `- name: <step>` inside `  <job>:` under `jobs:` and return its
/// run text plus the three env scopes.
fn find_step(workflow: &str, job: &str, step: &str) -> Option<Step> {
    let lines: Vec<&str> = workflow.lines().collect();
    let jobs_position = lines
        .iter()
        .position(|line| indent_of(line) == 0 && key_of(line.trim()) == "jobs")?;

    let mut job_position = None;
    for (position, line) in lines.iter().enumerate().skip(jobs_position + 1) {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if indent_of(line) == 0 {
            break;
        }
        if indent_of(line) == 2 && key_of(line.trim()) == job {
            job_position = Some(position);
            break;
        }
    }
    let job_lines = block_span(&lines, job_position? + 1, 2);

    let step_position = job_lines.iter().position(|line| {
        line.trim_start()
            .strip_prefix("- name:")
            .is_some_and(|rest| strip_comment(rest).trim() == step)
    })?;
    let step_indent = indent_of(job_lines[step_position]);
    let step_lines = block_span(&job_lines, step_position + 1, step_indent);

    let run_position = step_lines
        .iter()
        .position(|line| line.trim_start().starts_with("run:"))?;
    let run_indent = indent_of(step_lines[run_position]);
    let value = step_lines[run_position]
        .trim_start()
        .strip_prefix("run:")?
        .trim_start();
    let content = block_span(&step_lines, run_position + 1, run_indent);
    let run = if value.starts_with('|') || value.starts_with('>') {
        // Literal/folded block: one shell line per content line.
        content
            .iter()
            .map(|line| line.trim().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        // Plain scalar wrapped over several lines folds with spaces.
        let mut parts = Vec::new();
        if !value.is_empty() {
            parts.push(value.to_string());
        }
        parts.extend(content.iter().map(|line| line.trim().to_string()));
        parts.join(" ")
    };

    let workflow_env = env_entries(&lines, 0);
    let job_env = env_entries(
        &job_lines,
        job_lines
            .iter()
            .map(|line| indent_of(line))
            .min()
            .unwrap_or(2),
    );
    let step_env = env_entries(
        &step_lines,
        step_lines
            .iter()
            .map(|line| indent_of(line))
            .min()
            .unwrap_or(step_indent + 2),
    );

    Some(Step {
        run,
        step_env,
        job_env,
        workflow_env,
    })
}

/// Parse the `## QUALITY GATES` fenced block into gate name -> raw run
/// block. Mirrors `scripts/gates-local.sh`: `- name:` starts an entry,
/// `run:` carries its value, and 4-space-indented lines continue it. A
/// `run: |` literal block keeps newline separators between commands,
/// while a plain-scalar `run:` value folds its wrapped lines with spaces.
fn parse_registry(text: &str) -> Result<Vec<Gate>, String> {
    let heading = text
        .find("## QUALITY GATES")
        .ok_or("AGENTS.md has no ## QUALITY GATES heading")?;
    let block = &text[heading..];
    let open = block
        .find("```")
        .ok_or("## QUALITY GATES has no fenced registry block")?
        + 3;
    let close = block[open..]
        .find("```")
        .ok_or("## QUALITY GATES registry fence is not closed")?
        + open;
    let body = &block[open..close];

    let mut gates = Vec::new();
    let mut name: Option<String> = None;
    let mut run: Vec<String> = Vec::new();
    let mut separator = " ";

    let flush =
        |gates: &mut Vec<Gate>, name: &mut Option<String>, run: &[String], separator: &str| {
            if let Some(name) = name.take()
                && !name.is_empty()
            {
                gates.push(Gate {
                    name,
                    run: run.join(separator).trim().to_string(),
                });
            }
        };

    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("- name:") {
            flush(&mut gates, &mut name, &run, separator);
            run.clear();
            separator = " ";
            name = Some(strip_comment(rest).trim().to_string());
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            let trimmed = line.trim_start();
            if let Some(value) = trimmed.strip_prefix("run:") {
                let value = value.trim_start();
                run.clear();
                separator = " ";
                if !(value.starts_with('|') || value.starts_with('>')) {
                    run.push(strip_comment(value).to_string());
                } else {
                    separator = "\n";
                }
                continue;
            }
        }
        if name.is_some() && line.starts_with("    ") {
            let continuation = line.trim();
            if !continuation.is_empty() {
                run.push(continuation.to_string());
            }
        }
    }
    flush(&mut gates, &mut name, &run, separator);

    Ok(gates)
}

/// Collapse whitespace runs outside quotes, leaving quoted spans verbatim,
/// and trim the result.
fn collapse_whitespace(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut in_single = false;
    let mut in_double = false;
    let mut pending_space = false;
    for ch in value.chars() {
        if in_single {
            collapsed.push(ch);
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            collapsed.push(ch);
            if ch == '"' {
                in_double = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_double = true;
                collapsed.push(ch);
                pending_space = false;
            }
            '\'' => {
                in_single = true;
                collapsed.push(ch);
                pending_space = false;
            }
            _ if ch.is_whitespace() => pending_space = !collapsed.is_empty(),
            _ => {
                if pending_space {
                    collapsed.push(' ');
                    pending_space = false;
                }
                collapsed.push(ch);
            }
        }
    }
    collapsed.trim_end().to_string()
}

/// Normalize a run block into ordered shell statements. A line ending in
/// `\` joins the next line; any other newline separates statements.
/// Full-line comments and `  #` inline comments are dropped. Quotes and
/// command separators survive verbatim so output stays paste-ready.
fn statements(block: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut pending: Vec<&str> = Vec::new();

    let finish = |pending: &mut Vec<&str>, statements: &mut Vec<String>| {
        if pending.is_empty() {
            return;
        }
        let statement = collapse_whitespace(&pending.join(" "));
        pending.clear();
        if !statement.is_empty() {
            statements.push(statement);
        }
    };

    for raw in block.lines() {
        let line = strip_comment(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut piece = trimmed;
        let continues = piece.ends_with('\\');
        if continues {
            piece = piece[..piece.len() - 1].trim_end();
        }
        if !piece.is_empty() {
            pending.push(piece);
        }
        if !continues {
            finish(&mut pending, &mut statements);
        }
    }
    finish(&mut pending, &mut statements);

    statements
}

/// Split a `NAME="value"` / `NAME=value` prefix off a statement, if any.
/// The value keeps its shell spelling (quotes included) so the caller can
/// decide which quoting is syntax.
fn split_env_prefix(statement: &str) -> Option<((String, String), &str)> {
    let (name, rest) = statement.split_once('=')?;
    let valid_name = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if !valid_name {
        return None;
    }

    let (value, remainder) = if let Some(inner) = rest.strip_prefix('"') {
        let close = inner.find('"')?;
        (&rest[..close + 2], &rest[close + 2..])
    } else if let Some(inner) = rest.strip_prefix('\'') {
        let close = inner.find('\'')?;
        (&rest[..close + 2], &rest[close + 2..])
    } else {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        (&rest[..end], &rest[end..])
    };
    if !remainder.is_empty() && !remainder.starts_with(char::is_whitespace) {
        return None;
    }

    Some(((name.to_string(), value.to_string()), remainder.trim()))
}

/// Compare the two mapped gates against the registry and return every
/// violation as a paste-ready block. An empty vector means parity holds.
fn check_parity(registry_text: &str, workflows: &[(&str, &str)]) -> Vec<String> {
    let registry = match parse_registry(registry_text) {
        Ok(registry) => registry,
        Err(error) => return vec![format!("lint-gate-parity: {error}")],
    };

    let mut violations = Vec::new();
    for (gate, file, job, step) in PARITY_GATES {
        let header = format!("lint-gate-parity: MISMATCH gate '{gate}' ({file})");

        let Some(entry) = registry.iter().find(|entry| entry.name == gate) else {
            violations.push(format!(
                "{header}\n  gate '{gate}' is missing from the AGENTS.md ## QUALITY GATES registry"
            ));
            continue;
        };
        let Some((_, workflow)) = workflows.iter().find(|(path, _)| *path == file) else {
            violations.push(format!(
                "{header}\n  mapped workflow file {file} was not read"
            ));
            continue;
        };
        let Some(mapped) = find_step(workflow, job, step) else {
            violations.push(format!(
                "{header}\n  mapped workflow step '{job}' / '{step}' is missing from {file}"
            ));
            continue;
        };

        let mut expected_env: Vec<(String, String)> = Vec::new();
        let mut expected = Vec::new();
        for statement in statements(&entry.run) {
            match split_env_prefix(&statement) {
                Some(((name, value), remainder)) => {
                    expected_env.push((name, value));
                    expected.push(remainder.to_string());
                }
                None => expected.push(statement),
            }
        }

        for (name, registry_value) in &expected_env {
            let (actual_raw, source) = mapped.resolve_env(name);
            let matches = actual_raw.as_deref().is_some_and(|value| {
                strip_one_quote_pair(value) == strip_one_quote_pair(registry_value)
            });
            if matches {
                continue;
            }
            let actual = actual_raw
                .as_deref()
                .map(strip_one_quote_pair)
                .unwrap_or("unset");
            violations.push(format!(
                "{header}\nexpected {name}={registry_value} (registry prefix)\nactual {name}: {actual} (source: {source})"
            ));
        }

        let actual = statements(&mapped.run);
        if expected != actual {
            let mut block = header;
            block.push_str("\nexpected (AGENTS.md registry):");
            for statement in &expected {
                block.push_str("\n  ");
                block.push_str(statement);
            }
            block.push_str("\nactual (");
            block.push_str(file);
            block.push_str(" '");
            block.push_str(step);
            block.push_str("'):");
            for statement in &actual {
                block.push_str("\n  ");
                block.push_str(statement);
            }
            violations.push(block);
        }
    }

    violations
}

/// Run the gate-parity lint over the repo at `workspace_root`.
///
/// Reads `AGENTS.md` and the two mapped workflow files, then returns every
/// drift violation as a paste-ready block. An empty vector means the
/// mapped gates match the registry.
pub fn lint_gate_parity(workspace_root: &Path) -> Result<Vec<String>, String> {
    let agents_path = workspace_root.join("AGENTS.md");
    let agents = std::fs::read_to_string(&agents_path)
        .map_err(|e| format!("Cannot read {}: {e}", agents_path.display()))?;

    let mut files: Vec<(&'static str, String)> = Vec::new();
    for (_, file, _, _) in PARITY_GATES {
        if files.iter().any(|(path, _)| *path == file) {
            continue;
        }
        let path = workspace_root.join(file);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        files.push((file, text));
    }
    let workflows: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (*path, text.as_str()))
        .collect();

    Ok(check_parity(&agents, &workflows))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CI_PATH: &str = ".github/workflows/ci.yml";
    const DOCS_PATH: &str = ".github/workflows/docs.yml";

    const CLIPPY_STEP: &str = r#"      - name: clippy
        run: |
          cargo clippy --workspace --all-features \
            --exclude camel-cli \
            --all-targets \
            -- -D warnings
          # a full-line comment between statements
          cargo clippy -p camel-component-kafka --all-targets -- -D warnings
          cargo clippy -p camel-cli -- -D warnings
          cargo clippy -p camel-cli --no-default-features \
            --features flavor-regular,exec --all-targets -- -D warnings
"#;

    fn registry() -> String {
        r#"# AGENTS

## QUALITY GATES

```
- name: clippy
  run: |
    cargo clippy --workspace --all-features \
      --exclude camel-cli \
      --all-targets \
      -- -D warnings
    cargo clippy -p camel-component-kafka --all-targets -- -D warnings
    cargo clippy -p camel-cli -- -D warnings
    cargo clippy -p camel-cli --no-default-features \
      --features flavor-regular,exec --all-targets -- -D warnings
- name: doc-build
  run: RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core
    -p camel-xslt --no-deps
```
"#
        .to_string()
    }

    fn registry_without_doc_build() -> String {
        let full = registry();
        let cut = full
            .find("- name: doc-build")
            .expect("fixture registry has a doc-build gate");
        format!("{}```\n", &full[..cut])
    }

    fn ci_yml() -> String {
        format!(
            r#"name: CI

on:
  push:

jobs:
  quality:
    runs-on: ubuntu-latest
    steps:
      - name: fmt check
        run: cargo fmt --check --all
{CLIPPY_STEP}      - name: lint-unwrap
        run: cargo xtask lint-unwrap
  other:
    runs-on: ubuntu-latest
    steps:
      - name: clippy
        run: cargo clippy --workspace
"#
        )
    }

    fn docs_yml() -> String {
        r#"name: Documentation

env:
  RUSTDOCFLAGS: "-D warnings"

jobs:
  validate:
    runs-on: ubuntu-latest
    steps:
      - name: Build Rustdoc
        run: |
          cargo doc \
            -p camel-api \
            -p camel-core \
            -p camel-xslt \
            --no-deps
"#
        .to_string()
    }

    fn run(registry_text: &str, ci: &str, docs: &str) -> Vec<String> {
        check_parity(registry_text, &[(CI_PATH, ci), (DOCS_PATH, docs)])
    }

    #[test]
    fn clean_tree_passes() {
        let violations = run(&registry(), &ci_yml(), &docs_yml());

        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn clippy_flag_drift_fails_with_expected_and_actual() {
        let ci = ci_yml().replace("            --all-targets \\\n", "");
        let violations = run(&registry(), &ci, &docs_yml());

        assert_eq!(violations.len(), 1, "{violations:?}");
        let violation = &violations[0];
        assert!(
            violation.contains("MISMATCH gate 'clippy' (.github/workflows/ci.yml)"),
            "{violation}"
        );
        assert!(
            violation.contains(
                "cargo clippy --workspace --all-features --exclude camel-cli --all-targets -- -D warnings"
            ),
            "{violation}"
        );
        assert!(
            violation.contains(
                "cargo clippy --workspace --all-features --exclude camel-cli -- -D warnings"
            ),
            "{violation}"
        );
    }

    #[test]
    fn missing_mapped_step_fails() {
        // Variant A: ci.yml has no clippy step.
        let ci = ci_yml().replace(CLIPPY_STEP, "");
        let violations = run(&registry(), &ci, &docs_yml());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains("MISMATCH gate 'clippy' (.github/workflows/ci.yml)"),
            "{violations:?}"
        );
        assert!(
            violations[0].contains("mapped workflow step 'quality' / 'clippy' is missing"),
            "{violations:?}"
        );

        // Variant B: the registry lost the doc-build gate, docs.yml kept it.
        let violations = run(&registry_without_doc_build(), &ci_yml(), &docs_yml());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains("MISMATCH gate 'doc-build' (.github/workflows/docs.yml)"),
            "{violations:?}"
        );
        assert!(
            violations[0].contains("gate 'doc-build' is missing from the AGENTS.md"),
            "{violations:?}"
        );
    }

    #[test]
    fn doc_build_crate_set_drift_fails() {
        let docs = docs_yml().replace("            -p camel-xslt \\\n", "");
        let violations = run(&registry(), &ci_yml(), &docs);

        assert_eq!(violations.len(), 1, "{violations:?}");
        let violation = &violations[0];
        assert!(
            violation.contains("MISMATCH gate 'doc-build' (.github/workflows/docs.yml)"),
            "{violation}"
        );
        assert!(
            violation.contains("cargo doc -p camel-api -p camel-core -p camel-xslt --no-deps"),
            "{violation}"
        );
        assert!(
            violation.contains("cargo doc -p camel-api -p camel-core --no-deps"),
            "{violation}"
        );
    }

    #[test]
    fn dropped_command_separator_fails() {
        let ci = ci_yml().replace(
            "-- -D warnings\n          # a full-line comment between statements\n          cargo clippy -p camel-component-kafka",
            "-- -D warnings cargo clippy -p camel-component-kafka",
        );
        let violations = run(&registry(), &ci, &docs_yml());

        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains(
                "cargo clippy --workspace --all-features --exclude camel-cli --all-targets -- -D warnings cargo clippy -p camel-component-kafka --all-targets -- -D warnings"
            ),
            "{violations:?}"
        );
    }

    #[test]
    fn comment_only_difference_passes() {
        let step = r#"      - name: clippy
        # a comment above the run block
        run: |
          # a leading comment inside the run block
          cargo clippy --workspace --all-features --exclude camel-cli \
            --all-targets -- -D warnings
          cargo clippy -p camel-component-kafka --all-targets -- -D warnings
          cargo clippy -p camel-cli -- -D warnings  # inline note
          cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec \
            --all-targets -- -D warnings
"#;
        let ci = ci_yml().replace(CLIPPY_STEP, step);
        let violations = run(&registry(), &ci, &docs_yml());

        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn inherited_workflow_env_satisfies_registry_prefix() {
        let violations = run(&registry(), &ci_yml(), &docs_yml());

        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn missing_or_overridden_env_fails() {
        // Variant A: no RUSTDOCFLAGS anywhere.
        let docs = docs_yml().replace("env:\n  RUSTDOCFLAGS: \"-D warnings\"\n\n", "");
        let violations = run(&registry(), &ci_yml(), &docs);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains("expected RUSTDOCFLAGS=\"-D warnings\" (registry prefix)"),
            "{violations:?}"
        );
        assert!(
            violations[0].contains("actual RUSTDOCFLAGS: unset (source: unset)"),
            "{violations:?}"
        );

        // Variant B: job-level override wins over the registry prefix.
        let docs = docs_yml()
            .replace("env:\n  RUSTDOCFLAGS: \"-D warnings\"\n\n", "")
            .replace(
                "  validate:\n    runs-on: ubuntu-latest\n",
                "  validate:\n    runs-on: ubuntu-latest\n    env:\n      RUSTDOCFLAGS: \"-D warnings -D rustdoc::broken_intra_doc_links\"\n",
            );
        let violations = run(&registry(), &ci_yml(), &docs);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains(
                "actual RUSTDOCFLAGS: -D warnings -D rustdoc::broken_intra_doc_links (source: job env)"
            ),
            "{violations:?}"
        );
    }
}
