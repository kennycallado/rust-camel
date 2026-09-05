//! `cargo xtask fuzz` — run a cargo-fuzz target, isolated to the current
//! linked worktree (corpus and artifacts under `<worktree>/target-fuzz/`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const KNOWN_TARGETS: &[&str] = &["dsl_yaml", "dsl_json", "dsl_template", "dsl_parity"];

/// True when `git_dir` and `git_common_dir` refer to the same directory,
/// i.e. the current checkout is the main checkout rather than a linked
/// worktree. Best-effort canonicalization: falls back to the raw path when
/// `fs::canonicalize` fails, so nonexistent test paths still compare.
fn is_main_checkout(git_dir: &Path, git_common_dir: &Path) -> bool {
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(git_dir) == canon(git_common_dir)
}

/// Corpus directory for a fuzz target, inside the worktree only.
fn corpus_dir(worktree: &Path, target: &str) -> PathBuf {
    worktree.join("target-fuzz").join("corpus").join(target)
}

/// Artifact directory for a fuzz target, inside the worktree only.
fn artifacts_dir(worktree: &Path, target: &str) -> PathBuf {
    worktree.join("target-fuzz").join("artifacts").join(target)
}

/// libFuzzer `-artifact_prefix` value for a target (trailing slash present).
fn artifact_prefix(worktree: &Path, target: &str) -> String {
    format!("{}/", artifacts_dir(worktree, target).display())
}

/// libFuzzer arguments: bounded run time and worktree-local crash artifacts.
fn libfuzzer_args(time: u64, prefix: &str) -> Vec<String> {
    vec![
        format!("-max_total_time={time}"),
        format!("-artifact_prefix={prefix}"),
    ]
}

/// libFuzzer argument suffix for `cargo fuzz tmin`: separator plus the
/// artifact prefix (so the minimized output lands in the scanned
/// `target-fuzz/artifacts/<target>/` — cargo-fuzz's default is
/// `fuzz/artifacts/`, which the wrapper never scans) plus a per-round
/// time cap (each minimization round is bounded; total may span several
/// rounds — the job's timeout is the hard ceiling).
fn tmin_arg_suffix(prefix: &str, max_total_time: u64) -> Vec<String> {
    vec![
        "--".to_string(),
        format!("-artifact_prefix={prefix}"),
        format!("-max_total_time={max_total_time}"),
    ]
}

/// Copy every regular file from `seeds_dir` into `corpus_dir` when the
/// corpus dir is empty or missing; returns the count written. A no-op
/// returning 0 when the corpus already has entries (existing corpora are
/// accumulated fuzzing state and must not be reset).
fn copy_seeds(seeds_dir: &Path, corpus_dir: &Path) -> std::io::Result<usize> {
    let has_entries = match fs::read_dir(corpus_dir) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => false,
    };
    if has_entries {
        return Ok(0);
    }
    fs::create_dir_all(corpus_dir)?;
    let mut written = 0;
    for entry in fs::read_dir(seeds_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            fs::copy(&path, corpus_dir.join(entry.file_name()))?;
            written += 1;
        }
    }
    Ok(written)
}

/// Entries of `after` absent from `before` (path-equality diff).
fn new_artifacts(before: &[PathBuf], after: &[PathBuf]) -> Vec<PathBuf> {
    after
        .iter()
        .filter(|p| !before.contains(p))
        .cloned()
        .collect()
}

/// Run one cargo-fuzz target inside this worktree. Refuses the main
/// checkout, unknown targets, and a missing nightly/cargo-fuzz toolchain
/// before any build starts; on a crashing run, minimizes the newest new
/// artifact and points at it for a `#[test]` regression case.
pub(crate) fn run(root: &Path, target: &str, time: u64) -> Result<(), String> {
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
    if is_main_checkout(&to_abs(&git_dir), &to_abs(&git_common_dir)) {
        return Err(
            "refusing: cargo xtask fuzz must run in a linked worktree, not the main checkout"
                .to_string(),
        );
    }

    if !KNOWN_TARGETS.contains(&target) {
        return Err(format!(
            "unknown fuzz target `{target}` — known targets: {}",
            KNOWN_TARGETS.join(", ")
        ));
    }

    let probe = Command::new("cargo")
        .args(["+nightly", "fuzz", "--version"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to probe cargo-fuzz: {e}"))?;
    if !probe.status.success() {
        return Err("cargo-fuzz or nightly toolchain missing — install with: rustup toolchain install nightly && cargo install cargo-fuzz".to_string());
    }

    let corpus = corpus_dir(root, target);
    fs::create_dir_all(&corpus)
        .map_err(|e| format!("failed to create corpus dir {}: {e}", corpus.display()))?;
    let seeds_dir = root.join("fuzz").join("seeds").join(target);
    let copied = copy_seeds(&seeds_dir, &corpus)
        .map_err(|e| format!("failed to copy seeds from {}: {e}", seeds_dir.display()))?;
    println!(
        "fuzz: corpus at {} ({} seed(s) copied)",
        corpus.display(),
        copied
    );

    let artifacts = artifacts_dir(root, target);
    fs::create_dir_all(&artifacts).map_err(|e| {
        format!(
            "failed to create artifacts dir {}: {e}",
            artifacts.display()
        )
    })?;
    let before = list_files(&artifacts)?;

    let target_dir = root.join("target-fuzz");
    let status = Command::new("cargo")
        .args(["+nightly", "fuzz", "run", target])
        .arg(&corpus)
        .arg("--")
        .args(libfuzzer_args(time, &artifact_prefix(root, target)))
        .current_dir(root)
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .map_err(|e| format!("failed to spawn cargo-fuzz: {e}"))?;
    if status.success() {
        return Ok(());
    }

    let exit_code = status.code().unwrap_or(-1);
    let after = list_files(&artifacts)?;
    let fresh = new_artifacts(&before, &after);
    if fresh.is_empty() {
        return Err(format!(
            "fuzz target `{target}` failed with exit code: {exit_code} (no new artifacts)"
        ));
    }
    println!("new artifact(s):");
    for path in &fresh {
        println!("  {}", path.display());
    }

    let mut minimized_note = String::new();
    if let Some(artifact) = newest_file(&fresh) {
        match minimize(root, target, &target_dir, &artifact, &artifacts) {
            Ok(Some(minimized)) => {
                println!("minimized artifact: {}", minimized.display());
                println!(
                    "promote this input into a #[test] regression case; do not commit the raw artifact"
                );
                minimized_note = format!("; minimized artifact: {}", minimized.display());
            }
            Ok(None) => eprintln!(
                "fuzz: tmin succeeded but no new artifact detected in {}",
                artifacts.display()
            ),
            Err(e) => return Err(e),
        }
    }
    Err(format!(
        "fuzz target `{target}` failed with exit code: {exit_code}{minimized_note}"
    ))
}

/// Run `git <args>` in `root` and return trimmed stdout.
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

/// Regular files directly inside `dir` (not recursive).
fn list_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let entries =
        fs::read_dir(dir).map_err(|e| format!("failed to list {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry in {}: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_file() {
            files.push(path);
        }
    }
    Ok(files)
}

/// Timestamp of `path`: creation time with modification-time fallback,
/// epoch as last resort.
fn file_ts(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|m| m.created().or_else(|_| m.modified()))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Regular files in `dir` whose creation (fallback: modification) time is
/// at or after `since`. cargo-fuzz's tmin output naming is not hardcoded —
/// the timestamp window is the contract.
fn entries_created_after(dir: &Path, since: SystemTime) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for path in list_files(dir)? {
        if file_ts(&path) >= since {
            files.push(path);
        }
    }
    Ok(files)
}

/// The newest entry of `paths` by creation (fallback: modification) time;
/// entries with unreadable timestamps sort as epoch.
fn newest_file(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().max_by_key(|p| file_ts(p)).cloned()
}

/// Note logged when `tmin` succeeds but writes no minimized copy because the
/// crash input is already minimal (0 bytes). The string doubles as the CI
/// drill's `already minimal` marker — keep both in sync.
const ALREADY_MINIMAL_NOTE: &str =
    "fuzz: tmin wrote no minimized copy (input already minimal: 0 bytes); keeping original";

/// Decide whether a `tmin` run that produced no fresh artifact should fall
/// back to the original crash input. A zero-byte input cannot be minimized
/// (there is nothing smaller than empty), and libFuzzer writes no
/// `minimized-from-*` file for it, so the original is its own minimization.
fn use_original_when_already_minimal(status_ok: bool, original_len: u64) -> bool {
    status_ok && original_len == 0
}

/// Minimize a crash artifact with `cargo fuzz tmin`; returns the minimized
/// file (written into the artifacts dir after `tmin` started) when found.
fn minimize(
    root: &Path,
    target: &str,
    target_dir: &Path,
    artifact: &Path,
    artifacts: &Path,
) -> Result<Option<PathBuf>, String> {
    let started = SystemTime::now();
    let output = Command::new("cargo")
        .args(["+nightly", "fuzz", "tmin", target])
        .arg(artifact)
        .args(tmin_arg_suffix(&artifact_prefix(root, target), 120))
        .current_dir(root)
        .env("CARGO_TARGET_DIR", target_dir)
        .output()
        .map_err(|e| format!("fuzz minimization failed to launch: {e}"))?;
    let fresh = entries_created_after(artifacts, started)?;
    if let Some(minimized) = newest_file(&fresh) {
        if !output.status.success() {
            eprintln!(
                "fuzz: tmin exited non-zero (code {}) but produced a minimized artifact; using it",
                output.status.code().unwrap_or(-1)
            );
        }
        return Ok(Some(minimized));
    }
    let original_len = artifact
        .metadata()
        .map(|m| m.len())
        .map_err(|e| format!("fuzz minimization cannot stat {}: {e}", artifact.display()))?;
    if use_original_when_already_minimal(output.status.success(), original_len) {
        eprintln!("{ALREADY_MINIMAL_NOTE}");
        return Ok(Some(artifact.to_path_buf()));
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stderr_note = if stderr.is_empty() {
            String::new()
        } else {
            format!("; stderr: {stderr}")
        };
        return Err(format!(
            "fuzz minimization (`cargo fuzz tmin`) failed with exit code: {} — original crash artifact: {}{stderr_note}",
            output.status.code().unwrap_or(-1),
            artifact.display()
        ));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_checkout_detected() {
        assert!(is_main_checkout(Path::new("/r/.git"), Path::new("/r/.git")));
    }

    #[test]
    fn linked_worktree_detected() {
        assert!(!is_main_checkout(
            Path::new("/r/.git/worktrees/w"),
            Path::new("/r/.git")
        ));
    }

    #[test]
    fn artifact_prefix_format() {
        assert_eq!(
            artifact_prefix(Path::new("/wt"), "dsl_yaml"),
            "/wt/target-fuzz/artifacts/dsl_yaml/"
        );
    }

    #[test]
    fn libfuzzer_args_shape() {
        let args = libfuzzer_args(90, "/wt/target-fuzz/artifacts/dsl_yaml/");
        assert!(args.contains(&"-max_total_time=90".to_string()));
        assert!(args.contains(&"-artifact_prefix=/wt/target-fuzz/artifacts/dsl_yaml/".to_string()));
    }

    #[test]
    fn tmin_arg_suffix_forwards_prefix_and_cap() {
        assert_eq!(
            tmin_arg_suffix("/wt/target-fuzz/artifacts/dsl_yaml/", 120),
            vec![
                "--".to_string(),
                "-artifact_prefix=/wt/target-fuzz/artifacts/dsl_yaml/".to_string(),
                "-max_total_time=120".to_string(),
            ]
        );
    }

    #[test]
    fn already_minimal_only_for_empty_input_on_success() {
        assert!(use_original_when_already_minimal(true, 0));
        // tmin failed: keep the honest failure path, never claim minimality
        assert!(!use_original_when_already_minimal(false, 0));
        // non-empty input with no fresh artifact is unexpected, not minimal
        assert!(!use_original_when_already_minimal(true, 4));
        assert!(!use_original_when_already_minimal(false, 4));
    }

    #[test]
    fn already_minimal_note_is_ci_drill_marker() {
        // The CI drill greps tmin.log for "already minimal"; keep the
        // wording anchored so the workflow and this note cannot drift.
        assert!(ALREADY_MINIMAL_NOTE.contains("already minimal"));
    }

    #[test]
    fn tmin_arg_suffix_separator_first() {
        let args = tmin_arg_suffix("/wt/target-fuzz/artifacts/dsl_yaml/", 120);
        assert_eq!(args[0], "--");
    }

    #[test]
    fn seeds_copied_into_empty_corpus() {
        let temp = tempfile::tempdir().unwrap(); // allow-unwrap
        let seeds = temp.path().join("seeds");
        let corpus = temp.path().join("corpus");
        fs::create_dir_all(&seeds).unwrap(); // allow-unwrap
        fs::create_dir_all(&corpus).unwrap(); // allow-unwrap
        fs::write(seeds.join("seed.yaml"), "routes: []\n").unwrap(); // allow-unwrap

        let written = copy_seeds(&seeds, &corpus).unwrap(); // allow-unwrap

        assert_eq!(written, 1);
        assert!(corpus.join("seed.yaml").exists());
    }

    #[test]
    fn seeds_skipped_when_corpus_has_files() {
        let temp = tempfile::tempdir().unwrap(); // allow-unwrap
        let seeds = temp.path().join("seeds");
        let corpus = temp.path().join("corpus");
        fs::create_dir_all(&seeds).unwrap(); // allow-unwrap
        fs::create_dir_all(&corpus).unwrap(); // allow-unwrap
        fs::write(seeds.join("seed.yaml"), "routes: []\n").unwrap(); // allow-unwrap
        fs::write(corpus.join("existing"), "x").unwrap(); // allow-unwrap

        let written = copy_seeds(&seeds, &corpus).unwrap(); // allow-unwrap

        assert_eq!(written, 0);
        assert!(!corpus.join("seed.yaml").exists());
    }

    #[test]
    fn new_artifacts_diff() {
        let before = vec![PathBuf::from("/a/crash-1")];
        let after = vec![PathBuf::from("/a/crash-1"), PathBuf::from("/a/oom-2")];
        assert_eq!(
            new_artifacts(&before, &after),
            vec![PathBuf::from("/a/oom-2")]
        );
    }

    #[test]
    fn known_targets_cover_all_four() {
        // The four targets must be listed exactly once each: present, no
        // duplicates, and no fifth distinct entry. The per-name count
        // catches missing/duplicated names; the length check catches any
        // extra entry beyond the four.
        for name in ["dsl_yaml", "dsl_json", "dsl_template", "dsl_parity"] {
            let occurrences = KNOWN_TARGETS.iter().filter(|known| **known == name).count();
            assert_eq!(
                occurrences, 1,
                "KNOWN_TARGETS must contain `{name}` exactly once, got {occurrences}"
            );
        }
        assert_eq!(KNOWN_TARGETS.len(), 4);
    }

    #[test]
    fn known_targets_seeds_dirs_exist() {
        let seeds_root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fuzz/seeds"));
        for name in KNOWN_TARGETS {
            let dir = seeds_root.join(name);
            assert!(dir.is_dir(), "missing seeds dir {}", dir.display());
        }
    }
}
