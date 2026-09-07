//! CLI end-to-end: the scenario boot root is the nearest `Camel.toml`
//! ancestor (rc-jjzy5, scenario-harness-ergonomics task 2.1).
//!
//! Spawns the real `camel test` command (default build, so
//! `integration-http` is active and scenario documents take the
//! embedded FULL-tier boot) against nested fixture trees: the
//! document lives in a subdirectory, the `Camel.toml` at an ancestor.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "camel-scenario-boot-root-{tag}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
    dir
}

const CAMEL_TOML: &str = "# minimal\n";

const ROUTE: &str = r#"
routes:
  - id: nested-route
    from: direct:start
    steps:
      - to: log:info
"#;

const DOC: &str = r#"
routeFiles: [local.yaml]
scenario:
  - send:
      to: direct:start
"#;

/// A scenario document nested under its project root boots through
/// the ancestor `Camel.toml`: exit 0, the one action passes (before
/// the fix the boot sealed against the document's own directory and
/// failed cannot-stat with exit 2).
#[test]
fn nested_scenario_doc_boots_via_cli() {
    let root = temp_dir("nested");
    let sub = root.join("sub");
    fs::create_dir_all(&sub).expect("mkdir sub"); // allow-unwrap
    fs::write(root.join("Camel.toml"), CAMEL_TOML).expect("write Camel.toml"); // allow-unwrap
    fs::write(sub.join("local.yaml"), ROUTE).expect("write route file"); // allow-unwrap
    let doc = sub.join("doc.test.yaml");
    fs::write(&doc, DOC).expect("write document"); // allow-unwrap

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "nested scenario must boot through the ancestor root\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("1 passed, 0 failed"),
        "the scenario action must pass\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// A scenario document with no `Camel.toml` in any ancestor fails
/// named with exit 2 (before the fix the boot failed with a
/// cannot-stat config error instead of naming the missing root).
#[test]
fn no_root_fails_named_exit_2() {
    let dir = temp_dir("no-root");
    let doc = dir.join("case.test.yaml");
    fs::write(&doc, DOC).expect("write document"); // allow-unwrap

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc)
        .output()
        .expect("spawn camel test"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "a scenario document without a Camel.toml ancestor must exit 2\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("no Camel.toml ancestor"),
        "stderr must name the missing project root\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
