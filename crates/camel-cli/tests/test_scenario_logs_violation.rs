//! CLI end-to-end (rc-tdgh5 carry-over fix): a scenario document whose
//! route emits a WARN under a `noLevelAbove: info` ceiling must FAIL —
//! a `logs` verdict-class row carrying the diagnostic naming the
//! violated clause and the offending level — never report PASS with
//! exit 0.
//!
//! Spawns the real `camel test` command in a fresh process (the
//! `logs:` block needs the harness log-capture subscriber, which owns
//! the process's tracing seat first-wins; in-process runs race the
//! seat against every other booting test, a subprocess cannot): the
//! document boots through the embedded FULL-tier path, the send action
//! passes, and the logs violation surfaces as the driver's `failed`
//! counter — verdict class, exit 1.
#![cfg(feature = "integration-http")]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "camel-scenario-logs-violation-{tag}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir"); // allow-unwrap
    dir
}

const CAMEL_TOML: &str = "log_level = \"info\"\n";

/// The route logs the exchange body at WARN — the severity the
/// document's `info` ceiling must catch.
const ROUTE: &str = r#"
routes:
  - id: marker-warn
    from: direct:start
    steps:
      - to: log:marker?level=WARN
"#;

const DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
  - send:
      to: direct:start
      body: route warn marker body
logs:
  noLevelAbove: info
"#;

#[test]
fn logs_violation_fails_document_at_cli() {
    let dir = temp_dir("warn-over-info");
    fs::write(dir.join("Camel.toml"), CAMEL_TOML).expect("write Camel.toml"); // allow-unwrap
    fs::write(dir.join("routes.yaml"), ROUTE).expect("write routes.yaml"); // allow-unwrap
    let doc_path = dir.join("scenario.test.yaml");
    fs::write(&doc_path, DOC).expect("write scenario doc"); // allow-unwrap

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc_path)
        .current_dir(&dir)
        .output()
        .expect("camel test must spawn"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        output.status.code(),
        Some(1),
        "a logs violation is verdict class: exit 1, not 0\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // The send action passed; only the document-level logs slot failed.
    assert!(
        stdout.contains("1 passed, 1 failed"),
        "the logs row must increment the driver's failed counter\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("#logs"),
        "the FAIL row must carry the `logs` label\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("`logs.noLevelAbove`"),
        "the FAIL row must carry the diagnostic naming the violated clause\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("WARN"),
        "the diagnostic must name the offending level\nstdout:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
