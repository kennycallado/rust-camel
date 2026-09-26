//! E2E integration tests for the `openspec archive` wrapper
//! (OpenSpec change `archengine`, Task 4).
//!
//! Each test builds a scratch openspec root in a tempdir — canon spec plus a
//! minimal valid change — and drives the REAL wrapper flow (`run`) plus the
//! REAL `openspec` binary (nix-pinned 1.7.0). Requires `openspec` on PATH;
//! tests skip loudly when it is absent. Run with:
//! `cargo test -p xtask --test archive_e2e -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use xtask::archive::{GuardOutcome, check_guard, parse_directive_comment, run};

const CAP: &str = "http";

/// The directive comment carried by the rename+drop delta; asserted
/// verbatim in the canonical spec after the wrapper's splice.
const DIRECTIVE: &str = "<!-- openspec-scenario-ops\nrenamed: Alpha -> Alpha2\ndropped: Beta | superseded by Alpha2\n-->";

/// Canon fixture: requirement `Feature X` with the three scenarios
/// Alpha, Beta, Gamma (purpose long enough for upstream validate).
const CANON: &str = "\
# http Specification

## Purpose
The http capability governs request handling across the broker pipeline and defines scenario rename and drop semantics for archives.

## Requirements

### Requirement: Feature X
The system SHALL support feature X for all requests.

#### Scenario: Alpha
- **WHEN** a request arrives
- **THEN** alpha behavior applies

#### Scenario: Beta
- **WHEN** a request arrives
- **THEN** beta behavior applies

#### Scenario: Gamma
- **WHEN** a request arrives
- **THEN** gamma behavior applies
";

/// Delta MODIFIED block: renames Alpha -> Alpha2, drops Beta with a
/// justification, carries Gamma. The block the wrapper splices into canon
/// verbatim (directive comment included).
fn delta_directive() -> String {
    format!(
        "## MODIFIED Requirements\n\n\
         ### Requirement: Feature X\n\
         The system SHALL support feature X for all requests.\n\n\
         {DIRECTIVE}\n\n\
         #### Scenario: Alpha2\n\
         - **WHEN** a request arrives\n\
         - **THEN** alpha behavior applies\n\n\
         #### Scenario: Gamma\n\
         - **WHEN** a request arrives\n\
         - **THEN** gamma behavior applies\n"
    )
}

/// Delta MODIFIED block that SILENTLY omits Beta: no directive comment at
/// all, so the wrapper plans a passthrough and upstream must refuse.
fn delta_silent() -> String {
    "## MODIFIED Requirements\n\n\
     ### Requirement: Feature X\n\
     The system SHALL support feature X for all requests.\n\n\
     #### Scenario: Alpha\n\
     - **WHEN** a request arrives\n\
     - **THEN** alpha behavior applies\n\n\
     #### Scenario: Gamma\n\
     - **WHEN** a request arrives\n\
     - **THEN** gamma behavior applies\n"
        .to_owned()
}

/// Delta with the SAME requirement name in both MODIFIED and REMOVED —
/// upstream validate must report the conflict (AC#2).
fn delta_conflict() -> String {
    "## MODIFIED Requirements\n\n\
     ### Requirement: Feature X\n\
     The system SHALL support feature X for all requests.\n\n\
     #### Scenario: Alpha\n\
     - **WHEN** a request arrives\n\
     - **THEN** alpha behavior applies\n\n\
     ## REMOVED Requirements\n\n\
     ### Requirement: Feature X\n\n\
     **Reason**: No longer needed.\n\n\
     **Impact**: None.\n"
        .to_owned()
}

/// Scratch openspec root in a tempdir; the root IS the workspace root
/// (`openspec/` directly under it).
struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn openspec_on_path() -> bool {
    Command::new("openspec")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn skip_without_openspec() -> bool {
    if openspec_on_path() {
        return false;
    }
    println!("[SKIPPED: openspec not on PATH]");
    true
}

fn canon_path(root: &Path) -> PathBuf {
    root.join("openspec")
        .join("specs")
        .join(CAP)
        .join("spec.md")
}

fn scratch() -> Scratch {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("openspec").join("specs").join(CAP))
        .expect("mkdir canon dir"); // allow-unwrap
    std::fs::write(canon_path(&root), CANON).expect("write canon"); // allow-unwrap
    Scratch { _dir: dir, root }
}

fn write_change(root: &Path, name: &str, delta_body: &str) {
    let change = root.join("openspec").join("changes").join(name);
    std::fs::create_dir_all(change.join("specs").join(CAP)).expect("mkdir change"); // allow-unwrap
    std::fs::write(change.join("proposal.md"), "## Why\n\nE2E probe change.\n")
        .expect("write proposal"); // allow-unwrap
    std::fs::write(change.join("tasks.md"), "## 1. Adapter\n\n- [x] Task\n").expect("write tasks"); // allow-unwrap
    std::fs::write(change.join("specs").join(CAP).join("spec.md"), delta_body)
        .expect("write delta"); // allow-unwrap
}

/// Entry names under `openspec/changes/archive/` (upstream archives as
/// `<date>-<change>`).
fn archived_names(root: &Path) -> Vec<String> {
    let archive = root.join("openspec").join("changes").join("archive");
    let mut names: Vec<String> = std::fs::read_dir(&archive)
        .expect("read archive dir") // allow-unwrap
        .filter_map(|entry| entry.ok()) // allow-unwrap
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn captured(openspec_args: &[&str], root: &Path) -> (Option<i32>, String) {
    let out = Command::new("openspec")
        .args(openspec_args)
        .current_dir(root)
        .output()
        .expect("exec openspec"); // allow-unwrap
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code(), text)
}

#[test]
#[ignore]
fn e2e_rename_and_drop_archives() {
    if skip_without_openspec() {
        return;
    }
    let s = scratch();
    write_change(&s.root, "demo", &delta_directive());

    let code = run(&s.root, "demo", false);
    assert_eq!(code, 0, "wrapper must exit 0, got {code}");

    let canon = std::fs::read_to_string(canon_path(&s.root)).expect("read canon"); // allow-unwrap
    assert!(
        canon.contains("#### Scenario: Alpha2"),
        "renamed scenario missing from canon: {canon}"
    );
    assert!(
        canon.contains("#### Scenario: Gamma"),
        "carried scenario missing from canon: {canon}"
    );
    assert!(
        !canon.contains("#### Scenario: Alpha\n"),
        "old scenario name still in canon: {canon}"
    );
    assert!(
        !canon.contains("#### Scenario: Beta"),
        "dropped scenario still in canon: {canon}"
    );
    assert!(
        canon.contains(DIRECTIVE),
        "directive comment must be copied verbatim into canon: {canon}"
    );

    assert!(
        !s.root
            .join("openspec")
            .join("changes")
            .join("demo")
            .exists(),
        "change dir must have moved out of changes/"
    );
    let archived = archived_names(&s.root);
    assert!(
        archived.iter().any(|name| name.ends_with("demo")),
        "expected the change under changes/archive/, got {archived:?}"
    );
}

#[test]
#[ignore]
fn e2e_strict_refusal_preserved() {
    if skip_without_openspec() {
        return;
    }
    let s = scratch();
    write_change(&s.root, "demo", &delta_silent());
    let canon_file = canon_path(&s.root);
    let before = std::fs::read(&canon_file).expect("read canon"); // allow-unwrap

    // No directive anywhere: the wrapper passes through and upstream must
    // refuse the silent drop with a non-zero exit.
    let code = run(&s.root, "demo", false);
    assert_ne!(code, 0, "silent scenario drop must be refused, got {code}");
    assert!(
        s.root
            .join("openspec")
            .join("changes")
            .join("demo")
            .exists(),
        "a refused change must stay in changes/"
    );

    // Re-exec the same refusal with captured output to pin the upstream
    // message text (the wrapper's own passthrough inherits stdio).
    let (exit_code, text) = captured(&["archive", "demo", "--json", "--yes"], &s.root);
    assert_ne!(exit_code, Some(0), "upstream must keep refusing: {text}");
    assert!(
        text.contains("not present in the modified block"),
        "upstream refusal semantics missing: {text}"
    );

    let after = std::fs::read(&canon_file).expect("read canon"); // allow-unwrap
    assert_eq!(before, after, "canon must stay byte-identical on refusal");
}

#[test]
#[ignore]
fn e2e_validate_modified_removed_conflict() {
    if skip_without_openspec() {
        return;
    }
    let s = scratch();
    write_change(&s.root, "demo", &delta_conflict());

    // AC#2: the wrapper adds no leniency — upstream validate itself must
    // still flag a requirement present in both MODIFIED and REMOVED.
    let (exit_code, text) = captured(&["validate", "demo", "--json"], &s.root);
    assert_ne!(
        exit_code,
        Some(0),
        "validate must reject the MODIFIED+REMOVED conflict: {text}"
    );
    assert!(
        text.contains("both MODIFIED and REMOVED"),
        "conflict not reported by upstream validate: {text}"
    );
}

#[test]
#[ignore]
fn e2e_idempotent_rerun() {
    if skip_without_openspec() {
        return;
    }
    let s = scratch();
    write_change(&s.root, "demo", &delta_directive());
    assert_eq!(run(&s.root, "demo", false), 0, "first archive must succeed");

    // Second change re-asserting the archived canon state byte-for-byte:
    // its MODIFIED block IS the current canon block (directive included).
    let canon = std::fs::read_to_string(canon_path(&s.root)).expect("read canon"); // allow-unwrap
    let block_start = canon
        .find("### Requirement: Feature X")
        .expect("Feature X block in archived canon"); // allow-unwrap
    let block = &canon[block_start..];
    let mut delta = String::from("## MODIFIED Requirements\n\n");
    delta.push_str(block);
    write_change(&s.root, "demo2", &delta);

    // Guard level: the re-assertion must classify as already applied.
    let block_lines: Vec<String> = block.split('\n').map(str::to_owned).collect();
    let directive = match parse_directive_comment(&block_lines) {
        Ok(Some(directive)) => directive,
        other => panic!("expected the directive in the re-asserted block, got {other:?}"),
    };
    let outcome = match check_guard(block, block, &directive, "Feature X") {
        Ok(outcome) => outcome,
        Err(errors) => panic!("guard must accept the re-assertion: {errors:?}"),
    };
    assert_eq!(outcome, GuardOutcome::AlreadyApplied);

    // Wrapper level: zero writes, upstream no-op archive, exit 0.
    let canon_file = canon_path(&s.root);
    let before = std::fs::read(&canon_file).expect("read canon"); // allow-unwrap
    let code = run(&s.root, "demo2", false);
    assert_eq!(code, 0, "idempotent re-run must succeed, got {code}");
    let after = std::fs::read(&canon_file).expect("read canon"); // allow-unwrap
    assert_eq!(
        before, after,
        "canon must be byte-identical across the idempotent re-run"
    );
    assert!(
        !s.root
            .join("openspec")
            .join("changes")
            .join("demo2")
            .exists(),
        "the re-asserted change must have been archived as a no-op"
    );
}
