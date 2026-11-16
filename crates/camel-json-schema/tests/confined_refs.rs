//! Confined file retrieval for `$ref` targets (ADR-0100).
//!
//! Every test builds its own fixture tree: an optional outer directory that
//! models the world outside the schema root, and a `root/` directory holding
//! the root schema plus the files it is allowed to reference.

use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
use std::time::Duration;

use camel_json_schema::CompiledSchema;
use camel_json_schema::FormatMode;
use camel_json_schema::SchemaError;
use camel_json_schema::SchemaSource;
use camel_json_schema::SchemaSyntax;
use camel_json_schema::compile;
use serde_json::json;
use tempfile::TempDir;

fn compile_file(path: &Path) -> Result<CompiledSchema, SchemaError> {
    compile(
        &SchemaSource::File(path.to_path_buf()),
        SchemaSyntax::Auto,
        FormatMode::Enforce,
    )
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("writes a schema fixture");
}

/// Build `outer/` and `outer/root/`; the second path is the schema root.
fn root_inside_outer() -> (TempDir, PathBuf) {
    let outer = tempfile::tempdir().expect("creates the fixture root");
    let root = outer.path().join("root");
    std::fs::create_dir(&root).expect("creates the schema root");
    (outer, root)
}

#[test]
fn sibling_defs_ref_resolves_without_dollar_id() {
    let dir = tempfile::tempdir().expect("creates the fixture root");
    write(
        &dir.path().join("root.json"),
        r#"{"type":"object","properties":{"x":{"$ref":"defs.json#/$defs/x"}}}"#,
    );
    write(
        &dir.path().join("defs.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );

    let compiled = compile_file(&dir.path().join("root.json"))
        .expect("a sibling ref inside the schema root must resolve");

    assert!(
        compiled.violations(&json!({"x": 1})).is_empty(),
        "1 is an integer per the sibling definition"
    );
    assert!(
        !compiled.violations(&json!({"x": "s"})).is_empty(),
        "a string violates the sibling definition"
    );
}

#[test]
fn schema_path_with_spaces_compiles_and_resolves_siblings() {
    let dir = tempfile::tempdir().expect("creates the fixture root");
    let spaced = dir.path().join("my schemas");
    std::fs::create_dir(&spaced).expect("creates the spaced schema root");
    write(
        &spaced.join("root.json"),
        r#"{"type":"object","properties":{"x":{"$ref":"defs.json#/$defs/x"}}}"#,
    );
    write(
        &spaced.join("defs.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );

    let compiled = compile_file(&spaced.join("root.json"))
        .expect("a schema path containing spaces must compile");

    assert!(
        compiled.violations(&json!({"x": 1})).is_empty(),
        "1 is an integer per the sibling definition"
    );
    assert!(
        !compiled.violations(&json!({"x": "s"})).is_empty(),
        "a string violates the sibling definition"
    );
}

#[test]
fn parent_directory_ref_rejected_naming_path() {
    let (outer, root) = root_inside_outer();
    write(&root.join("root.json"), r#"{"$ref":"../outside.json"}"#);
    write(
        &outer.path().join("outside.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );

    let error =
        compile_file(&root.join("root.json")).expect_err("a parent-directory ref must be rejected");

    assert!(
        matches!(&error, SchemaError::ExternalRefRejected { .. }),
        "unexpected error: {error:?}"
    );
    assert!(
        error.to_string().contains("outside.json"),
        "message must name the escaping ref: {error}"
    );
}

#[test]
fn percent_encoded_traversal_rejected() {
    let (outer, root) = root_inside_outer();
    write(
        &root.join("root.json"),
        r#"{"$ref":"%2e%2e%2foutside.json"}"#,
    );
    write(
        &outer.path().join("outside.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );

    let error = compile_file(&root.join("root.json"))
        .expect_err("a percent-encoded traversal must be rejected");

    let SchemaError::ExternalRefRejected { uri, .. } = &error else {
        panic!("unexpected error: {error:?}");
    };
    assert!(
        uri.contains("../outside.json"),
        "the rejection must name the decoded reference, not the encoded one: {uri}"
    );
    assert!(
        !uri.contains("%2e"),
        "percent-encoding must be decoded before the path is named: {uri}"
    );
    assert!(
        error.to_string().contains("outside.json"),
        "message must name the rejected path: {error}"
    );
}

#[cfg(unix)]
#[test]
fn symlink_escape_rejected() {
    let (outer, root) = root_inside_outer();
    write(&root.join("root.json"), r#"{"$ref":"link.json#/$defs/x"}"#);
    write(
        &outer.path().join("outside.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );
    std::os::unix::fs::symlink(outer.path().join("outside.json"), root.join("link.json"))
        .expect("creates the escaping symlink");

    let error =
        compile_file(&root.join("root.json")).expect_err("a symlink escape must be rejected");

    assert!(
        matches!(&error, SchemaError::ExternalRefRejected { .. }),
        "unexpected error: {error:?}"
    );
    assert!(
        error.to_string().contains("link.json"),
        "message must name the escaping ref: {error}"
    );
}

#[test]
fn absolute_file_ref_outside_root_rejected() {
    let dir = tempfile::tempdir().expect("creates the fixture root");
    write(
        &dir.path().join("root.json"),
        r#"{"$ref":"file:///etc/hostname"}"#,
    );

    let error = compile_file(&dir.path().join("root.json"))
        .expect_err("an absolute file ref outside the root must be rejected");

    assert!(
        matches!(&error, SchemaError::ExternalRefRejected { .. }),
        "unexpected error: {error:?}"
    );
    assert!(
        error.to_string().contains("/etc/hostname"),
        "message must name the escaping path: {error}"
    );
}

#[test]
fn remote_ref_rejected_with_local_files_only_error() {
    let dir = tempfile::tempdir().expect("creates the fixture root");
    write(
        &dir.path().join("root.json"),
        r#"{"$ref":"https://example.com/s.json"}"#,
    );

    let error =
        compile_file(&dir.path().join("root.json")).expect_err("a remote ref must be rejected");

    assert!(
        matches!(&error, SchemaError::RemoteRefRejected { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        error.to_string(),
        "remote $ref 'https://example.com/s.json' rejected: schemas resolve from local files only (ADR-0100); vendor the file"
    );
}

#[test]
fn remote_dollar_id_rebase_rejected_with_local_files_only_error() {
    let dir = tempfile::tempdir().expect("creates the fixture root");
    write(
        &dir.path().join("root.json"),
        r#"{"$id":"https://example.com/base.json","$ref":"s.json"}"#,
    );
    write(
        &dir.path().join("s.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );

    let error = compile_file(&dir.path().join("root.json"))
        .expect_err("a remote $id rebase must be rejected");

    assert!(
        matches!(&error, SchemaError::RemoteRefRejected { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        error.to_string(),
        "remote $ref 'https://example.com/s.json' rejected: schemas resolve from local files only (ADR-0100); vendor the file"
    );
}

#[test]
fn out_of_root_file_dollar_id_rebase_rejected() {
    let dir = tempfile::tempdir().expect("creates the fixture root");
    write(
        &dir.path().join("root.json"),
        r#"{"$id":"file:///tmp/outside-dir/base.json","$ref":"s.json"}"#,
    );
    write(
        &dir.path().join("s.json"),
        r#"{"$defs":{"x":{"type":"integer"}}}"#,
    );

    let error = compile_file(&dir.path().join("root.json"))
        .expect_err("a file $id rebase out of the root must be rejected");

    assert!(
        matches!(&error, SchemaError::ExternalRefRejected { .. }),
        "unexpected error: {error:?}"
    );
    assert!(
        error.to_string().contains("outside-dir/s.json"),
        "message must name the escaped path: {error}"
    );
}

#[cfg(unix)]
#[test]
fn fifo_rejected_without_blocking() {
    let dir = tempfile::tempdir().expect("creates the fixture root");
    let fifo = dir.path().join("pipe.json");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("runs mkfifo");
    assert!(status.success(), "mkfifo must create the FIFO");
    write(&dir.path().join("root.json"), r#"{"$ref":"pipe.json"}"#);

    let (sender, receiver) = mpsc::channel();
    let root = dir.path().join("root.json");
    let handle = std::thread::spawn(move || {
        let outcome = compile_file(&root).map(|_| ());
        let _ = sender.send(outcome);
    });

    let outcome = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("compile must return instead of blocking on the FIFO");
    let _ = handle.join();

    let error = outcome.expect_err("a FIFO schema path must be rejected");
    assert!(
        matches!(&error, SchemaError::NotARegularFile { .. }),
        "unexpected error: {error:?}"
    );
}
