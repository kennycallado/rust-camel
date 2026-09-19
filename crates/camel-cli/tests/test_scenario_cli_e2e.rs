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

use std::path::Path;
use std::process::Command;

#[cfg(feature = "integration-http")]
use std::path::PathBuf;

/// The Task 3.2 outbound fixture project: `Camel.toml`, `routes/`, and
/// the scenario document, under `camel-integration-test`'s test
/// fixtures.
#[cfg(feature = "integration-http")]
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
        // Named shared-memory URI: every pool connection shares the
        // same in-process database (the sqlx provider pin is required
        // because no automatic prefix matches `sqlite:file:`), so
        // max_connections = 1 is a conservative fixture choice, not a
        // correctness requirement.
        r#"
[datasources.appdb]
db_url = "sqlite:file:memdb_cli_e2e_prepare?mode=memory&cache=shared"
provider = "sqlx"
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

/// A shared datasource config for the sql validate e2e fixtures: a
/// named shared-cache in-memory sqlite (explicit sqlx provider pin —
/// no automatic prefix matches `sqlite:file:`) with the conservative
/// `max_connections = 1` kept from the sql-only precedent.
const SQL_VALIDATE_CAMEL_TOML: &str = r#"
[datasources.appdb]
db_url = "sqlite:file:memdb_cli_e2e_validate?mode=memory&cache=shared"
provider = "sqlx"
max_connections = 1
"#;

/// The shared route file for the sql validate e2e fixtures: the
/// documents wire no endpoints, the boot only needs a route to load.
const SQL_VALIDATE_ROUTES_YAML: &str = r#"
routes:
  - id: boot-route
    from: direct:start
    steps:
      - to: log:info
"#;

/// Writes the Task 3.3 sql validate fixture into `dir` and returns the
/// document path: a `sql:` prepare action creates and seeds the table,
/// and a `validate` sql target asserts the seeded rows through the
/// expectation text. The `expected_rows` argument is what varies
/// between the passing and the redacting failing fixture.
fn write_sql_validate_doc(dir: &Path, expected_rows: &str) -> std::io::Result<std::path::PathBuf> {
    std::fs::write(dir.join("Camel.toml"), SQL_VALIDATE_CAMEL_TOML)?;
    std::fs::write(dir.join("routes.yaml"), SQL_VALIDATE_ROUTES_YAML)?;
    let doc = dir.join("validate.test.yaml");
    std::fs::write(
        &doc,
        format!(
            r#"
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare:
    - CREATE TABLE seeded (id INTEGER, name TEXT)
    - INSERT INTO seeded VALUES (1, 'alice')
    - INSERT INTO seeded VALUES (2, 'bob')
- validate:
    deadline: 300ms
    target:
      sql:
        datasource: appdb
        query: SELECT id, name FROM seeded ORDER BY id
    expectation:
      rows: {expected_rows}
"#
        ),
    )?;
    Ok(doc)
}

/// The Task 3.3 passing fixture (bd rc-25lup.2): the full-boot run
/// executes the `sql:` seed and then the `validate` sql target, whose
/// ordered `rows` shape matches the seeded table exactly — exit 0 with
/// a PASS row per action and the all-pass summary. Runs in the same
/// feature build as `sql_only_doc_boots_full` above.
#[cfg(feature = "integration-sql")]
#[test]
fn sql_validate_e2e_pass() {
    let dir = tempfile::tempdir().expect("temp dir");
    let doc = write_sql_validate_doc(dir.path(), "[[1, \"alice\"], [2, \"bob\"]]")
        .expect("write validate document");

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
        "a matching sql validate must exit 0\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    for row in ["scenario[0] action", "scenario[1] validate"] {
        assert!(
            stdout.contains(&format!("#{row}")),
            "missing PASS row {row}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
    assert!(
        stdout.contains("2 passed, 0 failed"),
        "every action must pass\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// The Task 3.3 failing fixture (bd rc-25lup.2): the same document
/// with a second-row expectation that does not match the seeded state
/// must exit nonzero, and the mismatch report must obey the ADR-0051
/// redaction law — it names the datasource (`sql appdb`) but carries
/// neither the resolved `db_url` nor any seeded cell literal. The
/// `300ms` deadline exercises the poll window: sql row sets are
/// non-monotone, so the verdict waits out the full window and decides
/// on the final snapshot.
#[cfg(feature = "integration-sql")]
#[test]
fn sql_validate_e2e_fail_redacts() {
    let dir = tempfile::tempdir().expect("temp dir");
    let doc = write_sql_validate_doc(dir.path(), "[[1, \"alice\"], [2, \"charlie\"]]")
        .expect("write validate document");

    let output = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("test")
        .arg(&doc)
        .current_dir(dir.path())
        .output()
        .expect("camel test must spawn"); // allow-unwrap
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "a mismatching sql validate must exit nonzero\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    // The datasource NAME renders in the verdict-class mismatch report.
    let report = combined
        .lines()
        .find(|line| line.contains("validation-mismatch"))
        .expect("a validation-mismatch report must render")
        .to_string();
    assert!(
        report.contains("sql appdb"),
        "mismatch must name the datasource\nreport: {report}"
    );
    // Redaction law (ADR-0051): the report never carries the resolved
    // db_url, the query text, or any actual cell value — the seeded
    // literals ('alice', 'bob') and the unmatched expectation literal
    // ('charlie') must all be absent from it. (Counts and column names
    // render; numeric seed cells deliberately share their spelling with
    // the counts, so only text literals are asserted.) The db_url
    // (sqlite:file:memdb_cli_e2e_validate?mode=memory&cache=shared) is
    // held against the WHOLE output — no subsystem may render it —
    // while the cell literals are held against the report line only:
    // sqlx's own DEBUG statement logs echo the doc-authored prepare
    // statements to stdout (boot-time subsystem logging, outside the
    // harness report whose text the redaction law governs). The guard
    // matches the URL's distinctive `memdb_cli_e2e_validate` fragment:
    // it appears nowhere in the doc-authored SQL or expectation text,
    // so it can only surface through a db_url leak.
    let leaked = "memdb_cli_e2e_validate";
    assert!(
        !combined.contains(leaked),
        "output must not leak {leaked:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    for leaked in ["alice", "bob", "charlie", "INSERT INTO", "SELECT id, name"] {
        assert!(
            !report.contains(leaked),
            "mismatch report must not leak {leaked:?}\nreport: {report}"
        );
    }
}
