//! CLI-level coverage for fail-closed walk errors.
//!
//! The missing scan root is the injected failure: a scratch workspace that
//! satisfies root discovery but has no `crates/components` subtree, so
//! `lint-cancel-tokens` must exit 1 and name the affected path on stderr.

use std::process::Command;

#[test]
fn lint_cancel_tokens_missing_root_exits_1() {
    let dir = tempfile::tempdir().expect("temporary dir");
    let root = dir.path();

    std::fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write Cargo.toml");
    std::fs::create_dir(root.join("bridges")).expect("create bridges/");
    let ratchet = root.join("scripts/xtask/ratchet-cancel-tokens.max");
    std::fs::create_dir_all(ratchet.parent().expect("ratchet parent")).expect("create scripts/");
    std::fs::write(&ratchet, "0\n").expect("write ratchet");

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["lint-cancel-tokens"])
        .current_dir(root)
        .output()
        .expect("run lint-cancel-tokens");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "missing scan root must exit 1; stderr: {stderr}"
    );
    assert!(
        stderr.contains("cannot walk"),
        "stderr must name the walk failure: {stderr}"
    );
    assert!(
        stderr.contains("components"),
        "stderr must name the affected path: {stderr}"
    );
}
