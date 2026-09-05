//! `cargo xtask mutants` — run cargo-mutants scoped to this linked
//! worktree, with outputs isolated under `<worktree>/target-mutants/`.
//! Mutation testing is informational only (bd rc-eba8): survivor counts
//! never gate. Mirrors the `fuzz.rs` wrapper idiom: thin `run` + pure
//! helpers, tests on the helpers with synthetic paths.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

/// cargo-mutants version this wrapper is pinned to. The outcomes schema and
/// the exit-code table in `classify_exit` are only valid for this version;
/// a bump must update the pin, the parser, and the tests in one commit.
const PINNED_VERSION: &str = "27.1.0";

/// Install hint carried by every missing-tool / version-mismatch error.
const INSTALL_HINT: &str = "cargo install --locked cargo-mutants --version 27.1.0";

/// Wrapper flags, mirroring the clap fields of the `Mutants` command.
#[derive(Default)]
pub(crate) struct MutantsArgs {
    pub(crate) file: Option<String>,
    pub(crate) diff: bool,
    pub(crate) json: bool,
}

/// Mirror of `fuzz.rs::is_main_checkout` (kept local so fuzz.rs and its
/// tests stay untouched). True when `git_dir` and `git_common_dir` refer
/// to the same directory, i.e. the current checkout is the main checkout
/// rather than a linked worktree. Best-effort canonicalization: falls back
/// to the raw path when `fs::canonicalize` fails, so nonexistent test
/// paths still compare.
fn is_main_checkout(git_dir: &Path, git_common_dir: &Path) -> bool {
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(git_dir) == canon(git_common_dir)
}

/// Guard decision: `Some(message)` when this is the main checkout (cargo
/// xtask mutants must run in a linked worktree, so its `target-mutants/`
/// stays out of the shared main `target/`), `None` when allowed.
fn guard_error(git_dir: &Path, git_common_dir: &Path) -> Option<String> {
    is_main_checkout(git_dir, git_common_dir).then(|| {
        "refusing: cargo xtask mutants must run in a linked worktree, not the main checkout"
            .to_string()
    })
}

/// Error for an absent cargo-mutants installation.
fn missing_tool_error() -> String {
    format!("cargo-mutants missing — install with: {INSTALL_HINT}")
}

/// Worktree-local output root for mutation runs (never the shared main
/// `target/`).
fn target_dir(root: &Path) -> PathBuf {
    root.join("target-mutants")
}

/// cargo-mutants arguments: always isolate `--output` under the worktree;
/// `--file P` maps to `--no-config --file P` verbatim (bypasses the
/// baseline config's globs); `--diff` maps to `--in-diff`. The two source
/// selectors are mutually exclusive.
fn mutants_argv(root: &Path, args: &MutantsArgs) -> Result<Vec<String>, String> {
    if args.file.is_some() && args.diff {
        return Err(
            "usage: --file and --diff are mutually exclusive — pick one selector".to_string(),
        );
    }
    let mut argv = vec![
        "mutants".to_string(),
        "--output".to_string(),
        target_dir(root).display().to_string(),
    ];
    if let Some(file) = &args.file {
        argv.push("--no-config".to_string());
        argv.push("--file".to_string());
        argv.push(file.clone());
    }
    if args.diff {
        argv.push("--in-diff".to_string());
    }
    Ok(argv)
}

/// Environment for the child process: exactly the worktree-local
/// `CARGO_TARGET_DIR`, nothing else added or removed.
fn target_env(root: &Path) -> Vec<(String, String)> {
    vec![(
        "CARGO_TARGET_DIR".to_string(),
        target_dir(root).display().to_string(),
    )]
}

/// Extract the version token from `cargo-mutants <semver> ...` output.
fn parse_version_output(output: &str) -> Result<String, String> {
    let mut parts = output.split_whitespace();
    let program = parts.next().unwrap_or_default();
    if program != "cargo-mutants" {
        return Err(format!(
            "unrecognized version output (expected `cargo-mutants <version>`): {output:?}"
        ));
    }
    parts
        .next()
        .map(str::to_string)
        .ok_or_else(|| format!("malformed version output (no version field): {output:?}"))
}

/// Presence-check decision: accept only the pinned version; a mismatch or
/// a parse failure fails loudly so schema and exit codes cannot drift
/// silently.
fn presence_check(version: Result<String, String>) -> Result<(), String> {
    match version {
        Ok(v) if v == PINNED_VERSION => Ok(()),
        Ok(found) => Err(format!(
            "cargo-mutants {found} found, but {PINNED_VERSION} is pinned (outcomes schema + exit codes) — install with: {INSTALL_HINT}"
        )),
        Err(e) => Err(format!("{e}; install with: {INSTALL_HINT}")),
    }
}

/// Map a cargo-mutants exit code to (survivors found?, error). Pinned to
/// cargo-mutants 27.1.0: 0 = all mutants caught, 2 = missed mutants found
/// (informational success, never an error), everything else is an
/// operational failure. `None` means the child was killed by a signal.
fn classify_exit(code: Option<i32>) -> Result<bool, String> {
    match code {
        Some(0) => Ok(false),
        Some(2) => Ok(true),
        Some(c @ (1 | 3 | 4 | 5 | 6 | 70)) => {
            Err(format!("cargo-mutants operational failure (exit code {c})"))
        }
        None => Err("cargo-mutants terminated by signal (no exit code)".to_string()),
        Some(other) => Err(format!(
            "cargo-mutants operational failure (unknown exit code {other})"
        )),
    }
}

/// Render survivor JSON lines from `mutants.out/outcomes.json` bytes
/// (pinned cargo-mutants 27.1.0 schema): root object, outcomes at
/// `.outcomes[]`; a survivor is an entry with `.summary == "MissedMutant"`
/// and a `.scenario.Mutant` object. Each line carries file / function /
/// mutation / status; `function` renders as null when the mutated
/// construct has no owning function. Schema drift fails loudly.
fn survivor_lines(outcomes_json: &[u8]) -> Result<Vec<String>, String> {
    let root: Value = serde_json::from_slice(outcomes_json)
        .map_err(|e| format!("malformed outcomes JSON: {e}"))?;
    let outcomes = root
        .get("outcomes")
        .ok_or_else(|| "outcomes schema drift: missing `outcomes` array".to_string())?
        .as_array()
        .ok_or_else(|| "outcomes schema drift: `outcomes` is not an array".to_string())?;
    let mut lines = Vec::new();
    for entry in outcomes {
        let summary = entry
            .get("summary")
            .and_then(Value::as_str)
            .ok_or_else(|| "outcomes schema drift: entry missing string `summary`".to_string())?;
        if summary != "MissedMutant" {
            // Caught / timeout / baseline outcomes are not survivors; they
            // may legitimately carry other scenario shapes, so they are
            // skipped regardless of their scenario.
            continue;
        }
        let Some(mutant) = entry.get("scenario").and_then(|s| s.get("Mutant")) else {
            // A MissedMutant entry MUST carry a Mutant scenario; its
            // absence is schema drift and must fail loudly.
            return Err(
                "outcomes schema drift: MissedMutant entry missing `scenario.Mutant`".to_string(),
            );
        };
        let file = mutant.get("file").and_then(Value::as_str).ok_or_else(|| {
            "outcomes schema drift: scenario.Mutant missing string `file`".to_string()
        })?;
        let name = mutant.get("name").and_then(Value::as_str).ok_or_else(|| {
            "outcomes schema drift: scenario.Mutant missing string `name`".to_string()
        })?;
        let function = mutant
            .pointer("/function/function_name")
            .and_then(Value::as_str);
        lines.push(
            json!({
                "file": file,
                "function": function,
                "mutation": name,
                "status": summary,
            })
            .to_string(),
        );
    }
    Ok(lines)
}

/// Run `git <args>` in `root` and return trimmed stdout. Mirror of
/// `fuzz.rs::git_output` (kept local so fuzz.rs stays untouched).
fn git_output(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run cargo-mutants inside this linked worktree. Refuses the main
/// checkout and a missing / version-drifted cargo-mutants before any run
/// starts. Survivors (exit 2) are informational success; only guard,
/// missing-tool, spawn, and operational-failure exits produce `Err`. With
/// `json`, stdout carries only this wrapper's survivor JSONL — the child's
/// human-readable output is forwarded to stderr.
pub fn run(root: &Path, args: &MutantsArgs) -> Result<(), String> {
    let git_dir = git_output(root, &["rev-parse", "--git-dir"])?;
    let git_common_dir = git_output(root, &["rev-parse", "--git-common-dir"])?;
    let to_abs = |raw: &str| -> PathBuf {
        let p = Path::new(raw);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        }
    };
    if let Some(msg) = guard_error(&to_abs(&git_dir), &to_abs(&git_common_dir)) {
        return Err(msg);
    }

    // Version probe pinned to PINNED_VERSION so the outcomes schema and the
    // exit-code table cannot drift silently.
    let probe = Command::new("cargo")
        .args(["mutants", "--version"])
        .current_dir(root)
        .output()
        .map_err(|_| missing_tool_error())?;
    if !probe.status.success() {
        return Err(missing_tool_error());
    }
    presence_check(parse_version_output(
        String::from_utf8_lossy(&probe.stdout).trim(),
    ))?;

    let mut command = Command::new("cargo");
    command.args(mutants_argv(root, args)?).current_dir(root);
    for (key, value) in target_env(root) {
        command.env(key, value);
    }

    let exit_code = if args.json {
        // STDOUT OWNERSHIP: in JSON mode the child's stdout is captured (not
        // inherited) and its human-readable output is forwarded to stderr, so
        // stdout carries only this wrapper's JSONL and `tee` sees a clean,
        // parseable stream.
        let output = command
            .output()
            .map_err(|e| format!("failed to spawn cargo mutants: {e}"))?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            eprint!("{stderr}");
        }
        // Forward the child's human-readable stdout to stderr too: in JSON
        // mode stdout carries only this wrapper's JSONL, so the child's
        // report must not leak into the `tee`-captured stream.
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.trim().is_empty() {
            eprint!("{stdout}");
        }
        output.status.code()
    } else {
        command
            .status()
            .map_err(|e| format!("failed to spawn cargo mutants: {e}"))?
            .code()
    };

    // Survivors (exit 2) are informational: the bool is deliberately unused.
    classify_exit(exit_code)?;

    if args.json {
        let outcomes = target_dir(root).join("mutants.out").join("outcomes.json");
        let bytes = fs::read(&outcomes)
            .map_err(|e| format!("failed to read {}: {e}", outcomes.display()))?;
        // An Err here on a successful run is itself an operational failure:
        // the pinned outcomes schema drifted.
        for line in survivor_lines(&bytes)? {
            println!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutants_guard_error_main_checkout() {
        let msg = guard_error(Path::new("/r/.git"), Path::new("/r/.git")).unwrap(); // allow-unwrap
        assert!(msg.contains("main checkout"));
        assert!(msg.contains("worktree"));
        // A linked worktree has differing git_dir / git_common_dir paths.
        assert_eq!(
            guard_error(Path::new("/r/.git/worktrees/w"), Path::new("/r/.git")),
            None
        );
    }

    #[test]
    fn mutants_missing_tool_error_contains_hint() {
        assert!(missing_tool_error().contains("cargo install --locked cargo-mutants"));
    }

    #[test]
    fn mutants_argv_default_run() {
        let argv = mutants_argv(Path::new("/wt"), &MutantsArgs::default()).unwrap(); // allow-unwrap
        assert!(argv.contains(&"--output".to_string()));
        assert!(argv.contains(&"/wt/target-mutants".to_string()));
        assert!(!argv.contains(&"--file".to_string()));
        assert!(!argv.contains(&"--no-config".to_string()));
        assert!(!argv.contains(&"--in-diff".to_string()));
    }

    #[test]
    fn mutants_argv_file_maps_verbatim() {
        let args = MutantsArgs {
            file: Some("crates/components/camel-http/src/lib.rs".to_string()),
            ..MutantsArgs::default()
        };
        let argv = mutants_argv(Path::new("/wt"), &args).unwrap(); // allow-unwrap
        assert!(argv.contains(&"--no-config".to_string()));
        assert!(argv.contains(&"--file".to_string()));
        // The path is forwarded verbatim, no rewriting.
        assert!(argv.contains(&"crates/components/camel-http/src/lib.rs".to_string()));
        assert!(!argv.contains(&"--in-diff".to_string()));
        // --output unchanged.
        assert!(argv.contains(&"--output".to_string()));
        assert!(argv.contains(&"/wt/target-mutants".to_string()));
    }

    #[test]
    fn mutants_argv_diff_maps() {
        let args = MutantsArgs {
            diff: true,
            ..MutantsArgs::default()
        };
        let argv = mutants_argv(Path::new("/wt"), &args).unwrap(); // allow-unwrap
        assert!(argv.contains(&"--in-diff".to_string()));
        assert!(!argv.contains(&"--file".to_string()));
        assert!(!argv.contains(&"--no-config".to_string()));
    }

    #[test]
    fn mutants_argv_rejects_combined_flags() {
        let args = MutantsArgs {
            file: Some("p".to_string()),
            diff: true,
            ..MutantsArgs::default()
        };
        let err = mutants_argv(Path::new("/wt"), &args).unwrap_err(); // allow-unwrap
        assert!(err.contains("usage"));
    }

    #[test]
    fn mutants_target_dir_derivation() {
        assert_eq!(
            target_env(Path::new("/wt")),
            vec![(
                "CARGO_TARGET_DIR".to_string(),
                "/wt/target-mutants".to_string()
            )]
        );
    }

    #[test]
    fn survivor_lines_renders_missed_only() {
        let fixture = br#"{"outcomes":[
            {"summary":"CaughtMutant","scenario":{"Mutant":{"file":"crates/x/src/lib.rs","name":"eq_true","function":{"function_name":"parse"}}}},
            {"summary":"MissedMutant","scenario":{"Mutant":{"file":"crates/y/src/conf.rs","name":"replace_or_with_and","function":{"function_name":"validate"}}}},
            {"summary":"MissedMutant","scenario":{"Mutant":{"file":"crates/z/src/lib.rs","name":"eq_true","function":{}}}}
        ]}"#;
        let lines = survivor_lines(fixture).unwrap(); // allow-unwrap
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            r#"{"file":"crates/y/src/conf.rs","function":"validate","mutation":"replace_or_with_and","status":"MissedMutant"}"#
        );
        // A mutated construct with no owning function renders function:null.
        assert_eq!(
            lines[1],
            r#"{"file":"crates/z/src/lib.rs","function":null,"mutation":"eq_true","status":"MissedMutant"}"#
        );
    }

    #[test]
    fn survivor_lines_rejects_malformed() {
        // Invalid JSON bytes.
        let err = survivor_lines(b"not json").unwrap_err(); // allow-unwrap
        assert!(err.contains("malformed"));
        // Valid JSON of an unknown shape: no `outcomes` array.
        let err = survivor_lines(br#"{"foo": 1}"#).unwrap_err(); // allow-unwrap
        assert!(err.contains("schema drift"));
        // An outcome entry whose Mutant scenario is missing `file`.
        let fixture = br#"{"outcomes":[{"summary":"MissedMutant","scenario":{"Mutant":{"name":"eq_true","function":{"function_name":"f"}}}}]}"#;
        let err = survivor_lines(fixture).unwrap_err(); // allow-unwrap
        assert!(err.contains("schema drift"));
        assert!(err.contains("file"));
        // A MissedMutant entry with no Mutant scenario at all is schema
        // drift and must fail loudly rather than be skipped.
        let fixture = br#"{"outcomes":[{"summary":"MissedMutant","scenario":{"Baseline":{"duration_secs":1.0}}}]}"#;
        let err = survivor_lines(fixture).unwrap_err(); // allow-unwrap
        assert!(err.contains("schema drift"));
        assert!(err.contains("scenario.Mutant"));
    }

    #[test]
    fn classify_exit_maps_classes() {
        assert_eq!(classify_exit(Some(0)), Ok(false));
        assert_eq!(classify_exit(Some(2)), Ok(true));
        for code in [1, 3, 4, 5, 6, 70] {
            let err = classify_exit(Some(code)).unwrap_err(); // allow-unwrap
            assert!(
                err.contains("operational"),
                "exit code {code} must be an operational error: {err}"
            );
        }
        // 101 is outside the pinned table: the catch-all arm treats any
        // unknown exit code as an operational failure too.
        let err = classify_exit(Some(101)).unwrap_err(); // allow-unwrap
        assert!(
            err.contains("operational"),
            "unknown exit code must be an operational error: {err}"
        );
        let err = classify_exit(None).unwrap_err(); // allow-unwrap
        assert!(err.contains("signal"));
    }

    #[test]
    fn mutants_version_probe_accepts_only_27_1_0() {
        assert_eq!(
            parse_version_output("cargo-mutants 27.1.0 (...)").unwrap(), // allow-unwrap
            "27.1.0"
        );
        assert!(
            presence_check(parse_version_output("cargo-mutants 27.1.0 (...)")).is_ok(),
            "the pinned version must be accepted"
        );
        let err = presence_check(parse_version_output("cargo-mutants 26.0.0 (...)")).unwrap_err(); // allow-unwrap
        assert!(err.contains("26.0.0"));
        assert!(err.contains("cargo install --locked cargo-mutants"));
        let err = presence_check(parse_version_output("oops")).unwrap_err(); // allow-unwrap
        assert!(err.contains("version output"));
    }

    #[test]
    fn mutants_baseline_globs_pinned() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.cargo/mutants.toml");
        let raw = fs::read_to_string(path).unwrap(); // allow-unwrap
        let parsed: toml::Value = toml::from_str(&raw).unwrap(); // allow-unwrap
        let globs = parsed.get("examine_globs").unwrap(); // allow-unwrap
        let globs: Vec<String> = globs
            .as_array()
            .unwrap() // allow-unwrap
            .iter()
            .map(|v| v.as_str().unwrap().to_string()) // allow-unwrap
            .collect();
        assert_eq!(
            globs,
            vec![
                "crates/camel-api/src/ssrf.rs",
                "crates/components/camel-mqtt/src/config.rs",
                "crates/components/camel-jms/src/config.rs",
            ]
        );
    }
}
