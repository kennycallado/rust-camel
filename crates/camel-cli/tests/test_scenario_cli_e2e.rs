//! CLI end-to-end: full-boot scenario execution through the real
//! `camel test` command (ADR-0069 sections 4-7).
//!
//! Spawns the actual `camel` binary — built with `integration-http`, as
//! this test is compiled — on the outbound-bridge fixture from
//! `camel-integration-test` (Task 3.2): the document boots the real
//! composition root (partner binds → layered environment →
//! `boot_scenario` → `run_scenario_document` → shutdown), and its own
//! `receive`/`validate` actions are the wire proof: the partner's
//! recorded request must match the sent body, method, path, and the
//! route-stamped header for the run to exit 0.
#![cfg(feature = "itest-e2e")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The Task 3.2 outbound fixture project: `Camel.toml`, `routes/`, and
/// the scenario document, under `camel-integration-test`'s test
/// fixtures.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../camel-integration-test/tests/fixtures/outbound")
}

/// The full-boot scenario runs through the real boot and the partner
/// listener receives on the wire: exit 0, the `[full]` tier annotation,
/// one PASS row per action (the wire-validating rows included), and the
/// all-pass summary.
#[cfg(feature = "integration-http")]
#[test]
fn cli_runs_full_boot_scenario() {
    let root = fixture_root();
    let doc = root.join("bridge.test.yaml");
    assert!(
        doc.is_file(),
        "fixture document must exist: {}",
        doc.display()
    );

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc)
        .current_dir(&root)
        .output()
        .expect("camel test must spawn"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        output.status.success(),
        "a full-boot pass must exit 0\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // Tier annotation: a `scenario:` document derives FULL.
    assert!(
        stdout.contains("[full]"),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // The wire-proof rows: `scenario[1] receive` extracts method, path,
    // and headers from the partner's wire arrival, and `scenario[6]
    // validate` compares the body last received on the wire against the
    // sent payload — both PASS only when the partner's recorded wire
    // request matches what the booted route sent.
    for row in [
        "scenario[0] send",
        "scenario[1] receive",
        "scenario[6] validate",
    ] {
        assert!(
            stdout.contains(&format!("#{row}")),
            "missing PASS row {row}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            !stdout.contains(&format!("FAIL #{row}")),
            "row {row} must not fail\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
    assert!(
        stdout.contains("7 passed, 0 failed"),
        "every action must pass\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// The `sql:`-only document boots FULL in a build that has
/// `integration-sql` — this suite compiles the spawned binary with
/// exactly that feature set when run as
/// `cargo test -p camel-cli --no-default-features --features
/// integration-sql,itest-e2e` — proving the full-boot path does not
/// require `integration-http` (bd rc-25lup.1). The document wires no
/// endpoints: its only action is a `sql:` seed against a shared-cache
/// in-memory sqlite datasource, and the run must exit 0.
#[cfg(feature = "integration-sql")]
#[test]
fn sql_only_doc_boots_full() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("Camel.toml"),
        // max_connections = 1 keeps every statement on one connection
        // so CREATE/INSERT state cannot split across pooled
        // connections (the camel-sql `:memory:` test precedent: with
        // the Any driver each pooled connection still gets a private
        // in-memory database, the split the executor stub in
        // `sql_action_test` pins the same way).
        r#"
[datasources.appdb]
db_url = "sqlite::memory:?cache=shared"
max_connections = 1
"#,
    )
    .expect("write Camel.toml");
    std::fs::write(
        dir.path().join("routes.yaml"),
        r#"
routes:
  - id: boot-route
    from: direct:start
    steps:
      - to: log:info
"#,
    )
    .expect("write route file");
    let doc = dir.path().join("seed.test.yaml");
    std::fs::write(
        &doc,
        r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare:
    - CREATE TABLE seeded (v TEXT)
    - INSERT INTO seeded VALUES ('e2e')
"#,
    )
    .expect("write document");

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc)
        .current_dir(dir.path())
        .output()
        .expect("camel test must spawn"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "an sql-only full-boot pass must exit 0\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
