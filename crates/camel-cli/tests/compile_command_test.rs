//! Integration tests for `camel compile` (openspec changes `cli-compile`
//! and `multidoc` Task 1.2). The blessed tests exercise `run_compile` end
//! to end by spawning the real `camel` binary with a controlled working
//! directory and a cleared environment, so exit codes, stderr
//! diagnostics, and the absence or presence of the output artifact are
//! asserted exactly as the CLI contract specifies. multidoc Task 1.2
//! switches the writer to the v2 multi-document virtual store: every
//! artifact is decoded through `decode_artifact` and, since r2embed
//! Task 1.1 / Task 2.2, carries `store_schema: 2` with `manifest_schema: 3`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use camel_cli::compile::signature;
use camel_cli::compile::store::{StoreIndex, VirtualDocumentStore};
use camel_cli::compile::trailer::{self, DecodedArtifact, TrailerKind, TrailerV2};
use ed25519_dalek::SigningKey;

/// A minimal supported single-route document.
const ROUTE_DOC: &str = "\
routes:
  - id: demo
    from: timer:tick?period=1s
    steps:
      - to: log:demo
";

/// Spawn `camel compile <doc> -o <artifact>` in `dir` with a cleared
/// environment (no inherited `CAMEL_*`, no ambient values).
fn compile(dir: &Path, doc: &str, artifact: &str, target: Option<&str>) -> Output {
    compile_full(dir, doc, artifact, target, None, &[])
}

/// Full compile invocation with the multidoc source-selection flags: an
/// explicit `--config <Camel.toml>` and repeatable `--profile <name>`.
fn compile_full(
    dir: &Path,
    doc: &str,
    artifact: &str,
    target: Option<&str>,
    config: Option<&str>,
    profiles: &[&str],
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear().current_dir(dir);
    cmd.arg("compile").arg(doc).arg("-o").arg(artifact);
    if let Some(triple) = target {
        cmd.arg("--target").arg(triple);
    }
    if let Some(config) = config {
        cmd.arg("--config").arg(config);
    }
    for profile in profiles {
        cmd.arg("--profile").arg(profile);
    }
    cmd.output().expect("spawn `camel compile`")
}

/// stderr of a finished child, for assertion-failure context.
fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Decode a v2 artifact image into (trailer, validated store, manifest
/// JSON). Panics when the image is not a marked, valid v2 artifact.
fn decode_v2(bytes: &[u8]) -> (TrailerV2, VirtualDocumentStore, serde_json::Value) {
    let decoded = trailer::decode_artifact(bytes)
        .expect("trailer must be intact")
        .expect("terminal magic must mark the trailer present");
    let DecodedArtifact::V2(v2) = decoded else {
        panic!("compile must emit a v2 multi-document artifact");
    };
    let store =
        VirtualDocumentStore::decode(v2.content.clone(), &v2.index).expect("store must decode");
    let manifest: serde_json::Value =
        serde_json::from_slice(&v2.manifest).expect("manifest must be JSON");
    (v2, store, manifest)
}

/// Entry paths of a decoded store, in canonical order.
fn entry_paths(store: &VirtualDocumentStore) -> Vec<String> {
    store.index.entries.iter().map(|e| e.path.clone()).collect()
}

#[test]
fn compile_writes_executable_with_valid_trailer() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");

    let output = compile(dir.path(), "app.yaml", "app.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "supported document must compile: {}",
        stderr_of(&output)
    );

    let artifact = dir.path().join("app.bin");
    let bytes = std::fs::read(&artifact).expect("artifact must exist after exit 0");

    // Executable output on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&artifact)
            .expect("artifact metadata")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "artifact must carry the executable bit");
    }

    // Decodable v2 route artifact: normalized payload, canonical
    // schema-3 manifest, one-entry store.
    let (v2, store, manifest) = decode_v2(&bytes);
    assert_eq!(v2.kind, TrailerKind::Route);
    assert_eq!(store.read("app.yaml"), Some(ROUTE_DOC.as_bytes()));
    assert_eq!(store.index.entry_point, "app.yaml");
    assert_eq!(store.index.source_plan.references, vec!["app.yaml"]);
    let manifest = manifest.to_string();
    assert!(
        manifest.contains(r#""kind":"route""#),
        "manifest kind must be route: {manifest}"
    );
    assert!(
        manifest.contains(r#""source_name":"app.yaml""#),
        "manifest source name must be the logical entry path: {manifest}"
    );
    assert!(
        manifest.contains(r#""manifest_schema":3"#),
        "v2 artifacts carry manifest schema 3: {manifest}"
    );
}

#[test]
fn compile_rejects_external_assets_before_output() {
    // Endpoint asset field: certificate.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\nrest:\n  - port: 8443\n    cert: server.pem\n    key: server.key\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("cert"),
        "rejection must name the certificate field: {}",
        stderr_of(&output)
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Camel.toml in the compile working directory (no --config given).
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("Camel.toml"), "[default]\n").expect("write Camel.toml");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("Camel.toml"),
        "rejection must name Camel.toml: {}",
        stderr_of(&output)
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Dynamic asset path: forbidden field whose value is an ${env:} placeholder.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\nrest:\n  - port: 8443\n    cert: ${env:TLS_CERT_PATH}\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("cert"),
        "rejection must name the dynamic asset field: {}",
        stderr_of(&output)
    );
    assert!(
        stderr_of(&output).contains("${env:"),
        "dynamic placeholder rejections must surface the placeholder: {}",
        stderr_of(&output)
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
}

#[test]
fn compile_preserves_env_expression_not_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:${env:COMPILE_TEST_NAME}\n",
    )
    .expect("write document");

    // The compile environment holds the value; the artifact must keep the
    // expression, never the value.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear()
        .env("COMPILE_TEST_NAME", "classified-compile-value")
        .current_dir(dir.path())
        .args(["compile", "app.yaml", "-o", "app.bin"]);
    let output = cmd.output().expect("spawn `camel compile`");
    assert_eq!(
        output.status.code(),
        Some(0),
        "compile must succeed: {}",
        stderr_of(&output)
    );

    let bytes = std::fs::read(dir.path().join("app.bin")).expect("artifact exists");
    let needle = b"${env:COMPILE_TEST_NAME}";
    assert!(
        bytes.windows(needle.len()).any(|w| w == needle),
        "artifact bytes must contain the env expression"
    );
    assert!(
        !bytes
            .windows(b"classified-compile-value".len())
            .any(|w| w == b"classified-compile-value"),
        "artifact bytes must not contain the compile-time env value"
    );
}

#[test]
fn compile_rejects_invalid_utf8_and_oversize_payload() {
    // Invalid UTF-8.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), [0xFF, 0xFE, b'x']).expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("UTF-8"),
        "rejection must name UTF-8: {}",
        stderr_of(&output)
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Payload over the 16 MiB limit.
    let dir = tempfile::tempdir().expect("tempdir");
    let oversized = vec![b'a'; trailer::MAX_PAYLOAD_BYTES + 1];
    std::fs::write(dir.path().join("app.yaml"), oversized).expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("16 MiB"),
        "rejection must name the payload limit: {}",
        stderr_of(&output)
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
}

#[test]
fn compile_rejects_non_native_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");

    // A triple that differs from the native host on any Linux runner.
    let other = if cfg!(target_arch = "x86_64") {
        "aarch64-unknown-linux-gnu"
    } else {
        "x86_64-unknown-linux-gnu"
    };

    let output = compile(dir.path(), "app.yaml", "out.bin", Some(other));
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("native Linux"),
        "diagnostic must state the native-Linux-only scope: {}",
        stderr_of(&output)
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
}

#[test]
fn compile_rejects_job_with_external_dependencies() {
    // Without --config there is no selected root: the external route
    // source is rejected (compile performs no ambient configuration
    // discovery, so routeFilesFromRoot has no anchor).
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("ingest.job.yaml"),
        "\
routeFilesFromRoot:
  - routes/*.yaml
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:start
",
    )
    .expect("write document");

    let output = compile(dir.path(), "ingest.job.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("routeFilesFromRoot"),
        "rejection must name the external route source: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Document-level profile selection is a config dependency: profiles
    // are selected through --profile against an explicit --config, never
    // declared inside the job document.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("ingest.job.yaml"),
        "\
profiles:
  - prod
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:start
",
    )
    .expect("write document");

    let output = compile(dir.path(), "ingest.job.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("profiles"),
        "rejection must name the config dependency: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // With an explicitly selected --config, the job's resolved route
    // sources are embedded and the artifact carries them.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Camel.toml"),
        "[default]\nlog_level = \"info\"\n",
    )
    .expect("write config");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("routes").join("in.yaml"),
        "routes:\n  - id: in\n    from: direct:start\n    steps:\n      - to: log:in\n",
    )
    .expect("write route source");
    std::fs::write(
        dir.path().join("ingest.job.yaml"),
        "\
routeFiles:
  - routes/in.yaml
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:start
",
    )
    .expect("write document");

    let output = compile_full(
        dir.path(),
        "ingest.job.yaml",
        "out.bin",
        None,
        Some("Camel.toml"),
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "resolved job sources must compile: {}",
        stderr_of(&output)
    );
    let (_, store, _) = decode_v2(&std::fs::read(dir.path().join("out.bin")).expect("artifact"));
    assert_eq!(
        entry_paths(&store),
        vec!["Camel.toml", "ingest.job.yaml", "routes/in.yaml"]
    );
    assert_eq!(store.index.entry_point, "ingest.job.yaml");
    assert_eq!(
        store.index.source_plan.references,
        vec!["ingest.job.yaml", "routes/in.yaml"]
    );
}

/// r3jobs Task 1.1: for a job-kind entry document, configuration `routes`
/// patterns add no plan entries. `camel job` never consults them (the
/// job's route set comes only from its own `routeFiles` /
/// `routeFilesFromRoot` declarations), so a document set the CLI accepts
/// must compile: here the config pattern `routes/*.yaml` matches the
/// explicitly declared `routes/b.yaml` and would collide with it
/// (`duplicate source`, exit 2) if the patterns seeded the plan.
#[test]
fn compile_job_ignores_config_route_patterns() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("conf")).expect("mkdir conf");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("Camel.toml"),
        "\
include = [\"conf/base.toml\"]
routes = [\"routes/*.yaml\"]
[default]
log_level = \"info\"
",
    )
    .expect("write config");
    std::fs::write(
        dir.path().join("conf").join("base.toml"),
        "[default]\ndrain_timeout_ms = 5000\n",
    )
    .expect("write include fragment");
    std::fs::write(
        dir.path().join("routes").join("a.yaml"),
        "routes:\n  - id: unused\n    from: direct:unused\n    steps:\n      - to: log:unused\n",
    )
    .expect("write pattern-matched route source");
    std::fs::write(
        dir.path().join("routes").join("b.yaml"),
        "routes:\n  - id: job-consumer\n    from: direct:start\n    steps:\n      - to: log:job-consumer\n",
    )
    .expect("write declared route source");
    std::fs::write(
        dir.path().join("ingest.job.yaml"),
        "\
routeFiles:
  - routes/b.yaml
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:start
",
    )
    .expect("write document");

    // GIVEN: `camel job` accepts this document set — it ignores the
    // configuration `routes` pattern and runs with the declared
    // `routes/b.yaml` only.
    let mut job = Command::new(env!("CARGO_BIN_EXE_camel"));
    job.env_clear().current_dir(dir.path());
    job.arg("job")
        .arg("ingest.job.yaml")
        .arg("--config")
        .arg("Camel.toml")
        .arg("--report")
        .arg("cli-report.json");
    let job_output = job.output().expect("spawn `camel job`");
    assert_eq!(
        job_output.status.code(),
        Some(0),
        "`camel job` must accept the document set: {}",
        stderr_of(&job_output)
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("cli-report.json")).expect("report written"),
    )
    .expect("report is JSON");
    assert_eq!(report["outcome"], "Completed", "report: {report}");

    // WHEN: the same document set is compiled with the same config.
    let output = compile_full(
        dir.path(),
        "ingest.job.yaml",
        "out.bin",
        None,
        Some("Camel.toml"),
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "a document set `camel job` accepts must compile: {}",
        stderr_of(&output)
    );
    let (_, store, _) = decode_v2(&std::fs::read(dir.path().join("out.bin")).expect("artifact"));

    // THEN: the plan holds only the entry and its declared route
    // source; the pattern-matched `routes/a.yaml` appears in no store
    // entry, while the config and include fragments still embed.
    assert_eq!(store.index.entry_point, "ingest.job.yaml");
    assert_eq!(
        store.index.source_plan.references,
        vec!["ingest.job.yaml", "routes/b.yaml"]
    );
    assert_eq!(
        entry_paths(&store),
        vec![
            "Camel.toml",
            "conf/base.toml",
            "ingest.job.yaml",
            "routes/b.yaml"
        ]
    );
}

/// r3jobs Task 1.2: a job document whose file-form route source
/// (`routeFiles`) resolves zero route files must fail compilation.
/// `camel job` rejects the same declaration set with its job-safety rule
/// ("job route source resolved zero route definitions"), so compile must
/// not exit 0 and embed a dead artifact. A wildcard pattern matching
/// nothing is not itself a resolution error — the zero-entry outcome is.
#[test]
fn compile_job_rejects_zero_route_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("conf")).expect("mkdir conf");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    // The config's `routes` pattern WOULD mask the zero-entry count
    // pre-guard (r3jobs Task 1.1): the job plan must ignore it, so the
    // rejection below still fires with the pattern present.
    std::fs::write(
        dir.path().join("Camel.toml"),
        "\
include = [\"conf/base.toml\"]
routes = [\"routes/*.yaml\"]
[default]
log_level = \"info\"
",
    )
    .expect("write config");
    std::fs::write(
        dir.path().join("conf").join("base.toml"),
        "[default]\ndrain_timeout_ms = 5000\n",
    )
    .expect("write include fragment");
    std::fs::write(
        dir.path().join("routes").join("b.yaml"),
        "routes:\n  - id: job-consumer\n    from: direct:start\n    steps:\n      - to: log:job-consumer\n",
    )
    .expect("write unrelated route source");
    std::fs::write(
        dir.path().join("ingest.job.yaml"),
        "\
routeFiles:
  - routes/none/*.yaml
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:start
",
    )
    .expect("write document");

    let output = compile_full(
        dir.path(),
        "ingest.job.yaml",
        "out.bin",
        None,
        Some("Camel.toml"),
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "zero resolved route files must fail compilation: {}",
        stderr_of(&output)
    );
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("job route source resolved zero route definitions"),
        "rejection must use the `camel job` rule wording: {stderr}"
    );
    assert!(
        stderr.contains("ingest.job.yaml"),
        "rejection must name the job document: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
}

/// r3jobs Task 1.2: same zero-entry rejection for the root-anchored
/// form — `routeFilesFromRoot` resolving zero route files under the
/// selected `--config` root fails compilation with the `camel job`
/// rule wording.
#[test]
fn compile_job_rejects_zero_route_files_from_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("conf")).expect("mkdir conf");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    std::fs::write(
        dir.path().join("Camel.toml"),
        "\
include = [\"conf/base.toml\"]
[default]
log_level = \"info\"
",
    )
    .expect("write config");
    std::fs::write(
        dir.path().join("conf").join("base.toml"),
        "[default]\ndrain_timeout_ms = 5000\n",
    )
    .expect("write include fragment");
    std::fs::write(
        dir.path().join("routes").join("b.yaml"),
        "routes:\n  - id: job-consumer\n    from: direct:start\n    steps:\n      - to: log:job-consumer\n",
    )
    .expect("write unrelated route source");
    std::fs::write(
        dir.path().join("ingest.job.yaml"),
        "\
routeFilesFromRoot:
  - routes/missing/*.yaml
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:start
",
    )
    .expect("write document");

    let output = compile_full(
        dir.path(),
        "ingest.job.yaml",
        "out.bin",
        None,
        Some("Camel.toml"),
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "zero resolved route files must fail compilation: {}",
        stderr_of(&output)
    );
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("job route source resolved zero route definitions"),
        "rejection must use the `camel job` rule wording: {stderr}"
    );
    assert!(
        stderr.contains("ingest.job.yaml"),
        "rejection must name the job document: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
}

/// Pin the duplicate rule to the job document's OWN declared route-file
/// family (blessed spec `r3jobs/specs/cli-compile`, MODIFIED requirement
/// "Resolve and confine compile-time sources": "the duplicate rule
/// applies within the job document's own declared route-file family").
/// The configuration here declares NO `routes` patterns, so the only
/// possible duplicate source is inside the job document itself: the
/// literal `routes/b.yaml` and the glob `routes/*.yaml` both resolve the
/// same canonical file (the second route file keeps the glob
/// meaningful). That within-family overlap must still fail closed with
/// the route-source duplicate wording, exactly as two route-file
/// patterns in one family always have.
#[test]
fn compile_job_rejects_duplicate_declared_route_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("conf")).expect("mkdir conf");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    // Plain configuration: include + [default], NO `routes` patterns —
    // the config cannot be the duplicate's origin.
    std::fs::write(
        dir.path().join("Camel.toml"),
        "\
include = [\"conf/base.toml\"]
[default]
log_level = \"info\"
",
    )
    .expect("write config");
    std::fs::write(
        dir.path().join("conf").join("base.toml"),
        "[default]\ndrain_timeout_ms = 5000\n",
    )
    .expect("write include fragment");
    // Two route files: the glob must be meaningful (more than one hit).
    std::fs::write(
        dir.path().join("routes").join("a.yaml"),
        "routes:\n  - id: job-consumer\n    from: direct:start\n    steps:\n      - to: log:job-consumer\n",
    )
    .expect("write first route source");
    std::fs::write(
        dir.path().join("routes").join("b.yaml"),
        "routes:\n  - id: job-consumer\n    from: direct:start\n    steps:\n      - to: log:job-consumer\n",
    )
    .expect("write second route source");
    std::fs::write(
        dir.path().join("ingest.job.yaml"),
        "\
routeFiles:
  - routes/b.yaml
  - routes/*.yaml
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:start
",
    )
    .expect("write document");

    let output = compile_full(
        dir.path(),
        "ingest.job.yaml",
        "out.bin",
        None,
        Some("Camel.toml"),
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "overlapping declared route files must fail compilation: {}",
        stderr_of(&output)
    );
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("duplicate source"),
        "rejection must use the duplicate-source wording: {stderr}"
    );
    assert!(
        stderr.contains("routes/b.yaml"),
        "rejection must name the duplicated source: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
    assert!(
        !dir.path().join("out.bin.tmp").exists(),
        "no partial artifact"
    );
}

// --- r3jobs Task 1.3: multi-entry job plan order and determinism ---

/// r3jobs Task 1.3 fixtures: a job document declaring three route
/// sources in a deliberate non-alphabetical order (literal, glob,
/// literal) whose flat resolution yields four route files, plus the
/// selected configuration with one include fragment.
const MULTI_ENTRY_JOB_DOC: &str = "\
routeFiles:
  - routes/b.yaml
  - routes/c*.yaml
  - routes/a.yaml
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: direct:start
";

const MULTI_ENTRY_ROUTE_B: &str =
    "routes:\n  - id: b\n    from: direct:start\n    steps:\n      - to: log:b\n";
const MULTI_ENTRY_ROUTE_A: &str =
    "routes:\n  - id: a\n    from: direct:aux\n    steps:\n      - to: log:a\n";
const MULTI_ENTRY_ROUTE_C1: &str =
    "routes:\n  - id: c1\n    from: direct:c1\n    steps:\n      - to: log:c1\n";
const MULTI_ENTRY_ROUTE_C2: &str =
    "routes:\n  - id: c2\n    from: direct:c2\n    steps:\n      - to: log:c2\n";

const MULTI_ENTRY_CONFIG: &str =
    "include = [\"conf/base.toml\"]\n[default]\nlog_level = \"info\"\n";
const MULTI_ENTRY_BASE_TOML: &str = "[default]\ndrain_timeout_ms = 5000\n";

/// Write the multi-entry job fixture tree. Route files are created in a
/// scrambled order (b, a, c2, c1) so a passing plan cannot be explained
/// by directory-creation order; only declared-pattern order with
/// sorted glob matches produces the expected plan.
fn write_multi_entry_fixture(dir: &Path) {
    std::fs::write(dir.join("Camel.toml"), MULTI_ENTRY_CONFIG).expect("write config");
    std::fs::create_dir_all(dir.join("conf")).expect("mkdir conf");
    std::fs::create_dir_all(dir.join("routes")).expect("mkdir routes");
    std::fs::write(dir.join("conf").join("base.toml"), MULTI_ENTRY_BASE_TOML)
        .expect("write include fragment");
    std::fs::write(dir.join("routes").join("b.yaml"), MULTI_ENTRY_ROUTE_B).expect("write route b");
    std::fs::write(dir.join("routes").join("a.yaml"), MULTI_ENTRY_ROUTE_A).expect("write route a");
    std::fs::write(dir.join("routes").join("c2.yaml"), MULTI_ENTRY_ROUTE_C2)
        .expect("write route c2");
    std::fs::write(dir.join("routes").join("c1.yaml"), MULTI_ENTRY_ROUTE_C1)
        .expect("write route c1");
    std::fs::write(dir.join("ingest.job.yaml"), MULTI_ENTRY_JOB_DOC).expect("write job document");
}

/// A job document declaring multiple route sources compiles with the
/// source plan in declared pattern order — each literal in its
/// declaration position, each glob's matches sorted — and the store
/// embeds all four route files as kind `route`.
#[test]
fn compile_job_multi_entry_plan_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_multi_entry_fixture(dir.path());

    let output = compile_full(
        dir.path(),
        "ingest.job.yaml",
        "out.bin",
        None,
        Some("Camel.toml"),
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "multi-entry job must compile: {}",
        stderr_of(&output)
    );

    let bytes = std::fs::read(dir.path().join("out.bin")).expect("artifact exists");
    let (_, store, _) = decode_v2(&bytes);

    // Declared pattern order: the entry document first, then `b`
    // (literal), then the glob's matches sorted (`c1`, `c2`), then `a`
    // (literal) — not alphabetical plan order.
    assert_eq!(store.index.entry_point, "ingest.job.yaml");
    assert_eq!(
        store.index.source_plan.references,
        vec![
            "ingest.job.yaml",
            "routes/b.yaml",
            "routes/c1.yaml",
            "routes/c2.yaml",
            "routes/a.yaml",
        ]
    );

    // All four route files embed as kind `route` (canonical store order).
    let route_entries: Vec<&str> = store
        .index
        .entries
        .iter()
        .filter(|e| e.kind.as_str() == "route")
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(
        route_entries,
        vec![
            "routes/a.yaml",
            "routes/b.yaml",
            "routes/c1.yaml",
            "routes/c2.yaml",
        ],
        "store must embed all four route entries with kind `route`"
    );
}

/// Compiling the same multi-entry job document set twice produces
/// byte-identical artifacts.
#[test]
fn compile_job_multi_entry_deterministic_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_multi_entry_fixture(dir.path());

    let first = compile_full(
        dir.path(),
        "ingest.job.yaml",
        "out1.bin",
        None,
        Some("Camel.toml"),
        &[],
    );
    assert_eq!(
        first.status.code(),
        Some(0),
        "first compile must succeed: {}",
        stderr_of(&first)
    );
    let second = compile_full(
        dir.path(),
        "ingest.job.yaml",
        "out2.bin",
        None,
        Some("Camel.toml"),
        &[],
    );
    assert_eq!(
        second.status.code(),
        Some(0),
        "second compile must succeed: {}",
        stderr_of(&second)
    );

    let bytes1 = std::fs::read(dir.path().join("out1.bin")).expect("first artifact exists");
    let bytes2 = std::fs::read(dir.path().join("out2.bin")).expect("second artifact exists");
    assert_eq!(
        bytes1, bytes2,
        "two compiles of the identical input set must emit identical bytes"
    );
}

// --- Focused regressions (review findings, Task 1.2) ---

/// Regression: the forbidden `key` field is scoped to TLS/listener
/// contexts. Ordinary message-step `key:` fields (set/remove header) are
/// message keys, not private-key files, and must compile.
#[test]
fn compile_allows_ordinary_key_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: demo\n    from: timer:tick?period=1s\n    steps:\n      - set_header:\n          key: my-header\n          value: my-value\n      - remove_header:\n          key: other-header\n      - to: log:demo\n",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "app.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "ordinary key: fields must compile: {}",
        stderr_of(&output)
    );
    let bytes = std::fs::read(dir.path().join("app.bin")).expect("artifact exists");
    let (v2, _, _) = decode_v2(&bytes);
    assert_eq!(v2.kind, TrailerKind::Route);
}

/// Regression: the endpoint-scheme check must reach URI-bearing fields
/// beyond `from`/`to` — `wire_tap`, `poll_enrich` (shorthand and `{uri}`
/// forms), and route-level `dead_letter_channel`. Since r2embed Task 1.2
/// only `wasm:` operands stay forbidden there: `xslt:`/`validator:`
/// operands are collected assets and compile (their fixture files are
/// embedded).
#[test]
fn compile_rejects_forbidden_schemes_in_nested_uri_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - wire_tap: wasm:module.wasm\n",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("wasm:module.wasm"),
        "rejection must name the nested endpoint 'wasm:module.wasm': {stderr}"
    );
    assert!(
        !dir.path().join("out.bin").exists(),
        "no output artifact for a wasm operand"
    );

    // Collected operand schemes compile from nested URI fields.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("assets")).expect("mkdir assets");
    std::fs::write(dir.path().join("assets/style.xsl"), "<xsl:stylesheet/>")
        .expect("write stylesheet fixture");
    std::fs::write(dir.path().join("assets/shape.xsd"), "<xs:schema/>")
        .expect("write schema fixture");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - poll_enrich: xslt:assets/style.xsl\n    error_handler:\n      dead_letter_channel: validator:assets/shape.xsd\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "collected xslt/validator operands must compile: {}",
        stderr_of(&output)
    );

    // The full `{uri: ...}` enrich form is checked too; an allowed scheme
    // stays permitted.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - poll_enrich:\n          uri: direct:extra\n      - to: log:x\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "allowed nested enrich scheme must compile: {}",
        stderr_of(&output)
    );
}

/// Regression: the endpoint-scheme check must reach `scatter_gather.endpoints`
/// (a sequence of endpoint URI strings), not just scalar URI fields.
/// Since r2embed Task 1.2 only `wasm:` operands stay forbidden there;
/// `xslt:`/`validator:` operands are collected assets and compile.
#[test]
fn compile_rejects_forbidden_schemes_in_scatter_gather_endpoints() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - scatter_gather:\n          endpoints:\n            - wasm:module.wasm\n",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("wasm:module.wasm"),
        "rejection must name the scatter_gather endpoint 'wasm:module.wasm': {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Collected operand schemes compile from scatter_gather endpoints.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("assets")).expect("mkdir assets");
    std::fs::write(dir.path().join("assets/style.xsl"), "<xsl:stylesheet/>")
        .expect("write stylesheet fixture");
    std::fs::write(dir.path().join("assets/shape.xsd"), "<xs:schema/>")
        .expect("write schema fixture");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - scatter_gather:\n          endpoints:\n            - xslt:assets/style.xsl\n            - validator:assets/shape.xsd\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "collected scatter_gather operand schemes must compile: {}",
        stderr_of(&output)
    );

    // Allowed schemes and runtime ${env:} expressions in scatter_gather
    // endpoints stay permitted.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - scatter_gather:\n          endpoints:\n            - direct:a\n            - ${env:SCATTER_TARGET}\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "app.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "allowed scatter_gather endpoints must compile: {}",
        stderr_of(&output)
    );
}

/// Regression: `*.job.json` is rejected explicitly instead of silently
/// compiling as a route document.
#[test]
fn compile_rejects_job_json_suffix_explicitly() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.job.json"), ROUTE_DOC).expect("write document");

    let output = compile(dir.path(), "app.job.json", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains(".job.json"),
        "rejection must name the '.job.json' suffix: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
}

/// Regression: output writing is atomic (temp file + rename) and a failed
/// compile never deletes or truncates a pre-existing artifact.
#[test]
fn compile_preserves_existing_output_on_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");

    // First compile succeeds and leaves no temp file behind.
    let output = compile(dir.path(), "app.yaml", "app.bin", None);
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    assert!(
        !dir.path().join("app.bin.tmp").exists(),
        "temp file must be consumed by the rename"
    );

    // A later rejected compile to the same output leaves the artifact intact.
    std::fs::write(
        dir.path().join("bad.yaml"),
        "routeFilesFromRoot:\n  - routes/*.yaml\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "bad.yaml", "app.bin", None);
    assert_eq!(output.status.code(), Some(2));
    assert!(!dir.path().join("app.bin.tmp").exists(), "no temp leftover");

    let bytes = std::fs::read(dir.path().join("app.bin")).expect("pre-existing output intact");
    let (v2, store, _) = decode_v2(&bytes);
    assert_eq!(v2.kind, TrailerKind::Route);
    assert_eq!(
        store.read("app.yaml"),
        Some(ROUTE_DOC.as_bytes()),
        "artifact untouched"
    );
}

// --- multidoc Task 1.2: explicit multi-document source selection ---

/// Explicit `--config` with ordered includes, repeated `--profile`
/// selection, and route patterns: the v2 index carries typed entries,
/// the source plan preserves declared pattern order with each pattern's
/// matches sorted by normalized logical path, and two runs over the same
/// tree produce byte-identical artifacts.
#[test]
fn compile_embeds_ordered_routes_config_includes_and_profiles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    const CONFIG: &str = concat!(
        "include = [\"conf/base.toml\"]\n",
        "routes = [\"routes/*.yaml\"]\n",
        "[default]\n",
        "log_level = \"info\"\n",
        "[staging]\n",
        "log_level = \"debug\"\n",
        "[prod]\n",
        "include = [\"conf/prod.toml\"]\n",
        "log_level = \"warn\"\n",
    );
    std::fs::write(root.join("Camel.toml"), CONFIG).expect("write config");
    std::fs::create_dir_all(root.join("conf")).expect("mkdir conf");
    std::fs::create_dir_all(root.join("routes")).expect("mkdir routes");
    std::fs::write(
        root.join("conf").join("base.toml"),
        "[default]\ntimeout_ms = 30000\n",
    )
    .expect("write include");
    std::fs::write(
        root.join("conf").join("prod.toml"),
        "[default]\ntimeout_ms = 60000\n",
    )
    .expect("write profile include");
    // Create the route files in reverse logical order so raw directory
    // enumeration cannot accidentally match the expected sorted plan.
    for name in ["z.yaml", "m.yaml", "a.yaml"] {
        std::fs::write(root.join("routes").join(name), ROUTE_DOC).expect("write route");
    }
    std::fs::write(root.join("app.yaml"), ROUTE_DOC).expect("write entry document");

    let output = compile_full(
        root,
        "app.yaml",
        "app.bin",
        None,
        Some("Camel.toml"),
        &["staging", "prod"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "explicit multi-document selection must compile: {}",
        stderr_of(&output)
    );

    let bytes = std::fs::read(root.join("app.bin")).expect("artifact exists");
    let (v2, store, _) = decode_v2(&bytes);
    assert_eq!(v2.kind, TrailerKind::Route);

    // Typed entries in canonical path order.
    let typed: Vec<(String, &str)> = store
        .index
        .entries
        .iter()
        .map(|e| (e.path.clone(), e.kind.as_str()))
        .collect();
    let typed_refs: Vec<(&str, &str)> = typed.iter().map(|(p, k)| (p.as_str(), *k)).collect();
    assert_eq!(
        typed_refs,
        vec![
            ("Camel.toml", "config"),
            ("app.yaml", "route"),
            ("conf/base.toml", "include"),
            ("conf/prod.toml", "include"),
            ("prod.profile.toml", "profile"),
            ("routes/a.yaml", "route"),
            ("routes/m.yaml", "route"),
            ("routes/z.yaml", "route"),
            ("staging.profile.toml", "profile"),
        ],
        "typed entries in canonical path order"
    );

    // Logical entry point and configuration references in resolution
    // order: config, ordered includes (top-level then profile-scoped),
    // then the selected profile fragments in flag order.
    assert_eq!(store.index.entry_point, "app.yaml");
    assert_eq!(
        store.index.config_references,
        vec![
            "Camel.toml",
            "conf/base.toml",
            "conf/prod.toml",
            "staging.profile.toml",
            "prod.profile.toml",
        ]
    );

    // Source plan: entry document first, then the declared pattern's
    // matches sorted by normalized logical path.
    assert_eq!(
        store.index.source_plan.references,
        vec![
            "app.yaml",
            "routes/a.yaml",
            "routes/m.yaml",
            "routes/z.yaml"
        ]
    );

    // Embedded bytes round-trip normalized pre-interpolation text.
    assert_eq!(
        store.read("conf/prod.toml"),
        Some(&b"[default]\ntimeout_ms = 60000\n"[..])
    );
    assert_eq!(store.read("Camel.toml"), Some(CONFIG.as_bytes()));

    // Deterministic output: a second run produces identical bytes.
    let output = compile_full(
        root,
        "app.yaml",
        "app2.bin",
        None,
        Some("Camel.toml"),
        &["staging", "prod"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    let bytes2 = std::fs::read(root.join("app2.bin")).expect("second artifact exists");
    assert_eq!(bytes, bytes2, "two runs must emit identical artifact bytes");
}

/// Without `--config` the compiler performs no ambient configuration
/// discovery: a `Camel.toml` sitting beside the primary document is
/// neither read nor embedded, and the artifact carries no configuration
/// references.
#[test]
fn compile_without_config_does_not_discover_ambient_config() {
    let outer = tempfile::tempdir().expect("tempdir");
    let proj = outer.path().join("proj");
    std::fs::create_dir_all(proj.join("routes")).expect("mkdir routes");
    std::fs::write(
        proj.join("Camel.toml"),
        "routes = [\"routes/*.yaml\"]\n[default]\nlog_level = \"warn\"\n",
    )
    .expect("write ambient config");
    std::fs::write(proj.join("routes").join("extra.yaml"), ROUTE_DOC).expect("write ambient route");
    std::fs::write(proj.join("app.yaml"), ROUTE_DOC).expect("write document");

    // Compile from the parent directory so the ambient Camel.toml is
    // beside the document (not the cwd, whose presence is a v1
    // rejection of its own).
    let output = compile_full(outer.path(), "proj/app.yaml", "out.bin", None, None, &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "compile without --config must succeed beside an ambient config: {}",
        stderr_of(&output)
    );

    let bytes = std::fs::read(outer.path().join("out.bin")).expect("artifact exists");
    let (_, store, manifest) = decode_v2(&bytes);
    assert_eq!(
        entry_paths(&store),
        vec!["app.yaml"],
        "only the primary document is embedded"
    );
    assert!(
        store.index.config_references.is_empty(),
        "no configuration references without --config"
    );
    assert_eq!(store.index.source_plan.references, vec!["app.yaml"]);
    assert_eq!(store.index.store_schema, 2);
    let embedded = manifest["embedded_files"]
        .as_array()
        .expect("embedded_files");
    assert_eq!(embedded.len(), 1, "manifest lists only the entry document");
}

/// Confinement fail-closed matrix: traversal, absolute, symlink-escape,
/// missing, duplicate-overlap, and non-UTF-8 source names all exit 2
/// with a named diagnostic and leave no usable output.
#[test]
fn compile_rejects_path_escape_duplicate_and_invalid_name() {
    /// Fresh project: config with a route pattern, routes dir, entry doc.
    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Camel.toml"),
            "routes = [\"routes/*.yaml\"]\n",
        )
        .expect("write config");
        std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
        std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");
        dir
    }

    /// Run one rejection case; assert exit 2, stderr keyword, no output.
    fn reject(dir: &tempfile::TempDir, doc: &str, keyword: &str) {
        let output = compile_full(dir.path(), doc, "out.bin", None, Some("Camel.toml"), &[]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "case '{keyword}' must be rejected: {}",
            stderr_of(&output)
        );
        let stderr = stderr_of(&output);
        assert!(
            stderr.to_lowercase().contains(keyword),
            "case must be named with '{keyword}': {stderr}"
        );
        assert!(
            !dir.path().join("out.bin").exists(),
            "no usable output for '{keyword}'"
        );
    }

    // Traversal: a declared source path with a `..` component.
    let dir = project();
    std::fs::write(dir.path().parent().unwrap().join("outside.yaml"), ROUTE_DOC)
        .expect("write outside file");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routeFiles:\n  - ../outside.yaml\nroutes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\n",
    )
    .expect("write document");
    reject(&dir, "app.yaml", "../outside.yaml");

    // Absolute: a declared absolute source path.
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "routeFiles:\n  - /etc/hostname\nroutes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\n",
    )
    .expect("write document");
    reject(&dir, "app.yaml", "absolute");

    // Symlink escape: a matched file under root resolves outside it.
    let dir = project();
    let outside = dir.path().parent().unwrap().join("escaped.yaml");
    std::fs::write(&outside, ROUTE_DOC).expect("write outside file");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, dir.path().join("routes").join("escape.yaml"))
        .expect("symlink");
    reject(&dir, "app.yaml", "outside");

    // Missing: a literal source that resolves to nothing.
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "routeFiles:\n  - routes/nope.yaml\nroutes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\n",
    )
    .expect("write document");
    reject(&dir, "app.yaml", "nope.yaml");

    // Duplicate overlap: a literal and a wildcard claim the same file.
    let dir = project();
    std::fs::write(dir.path().join("routes").join("a.yaml"), ROUTE_DOC).expect("write route");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routeFiles:\n  - routes/a.yaml\n  - routes/*.yaml\nroutes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\n",
    )
    .expect("write document");
    reject(&dir, "app.yaml", "duplicate");

    // Non-UTF-8 source name: the primary document's on-disk name is not
    // UTF-8, so no canonical logical path exists for it. The argument is
    // passed as the raw OsString — the lossy form would name a different
    // (nonexistent) file.
    let dir = project();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        let name = std::ffi::OsString::from_vec(vec![0xFF, 0xFE, b'.', b'y', b'a', b'm', b'l']);
        let doc = dir.path().join(name);
        // Filesystems such as ntfs3 reject non-UTF-8 names outright
        // (EINVAL); the rejection path under test cannot even be reached
        // there, so skip the sub-case instead of failing the suite.
        if let Err(e) = std::fs::write(&doc, ROUTE_DOC) {
            assert!(
                e.raw_os_error() == Some(22),
                "unexpected error writing non-UTF-8 named document: {e}"
            );
        } else {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
            cmd.env_clear().current_dir(dir.path());
            cmd.args(["compile", "-o", "out.bin"])
                .arg(&doc)
                .arg("--config")
                .arg("Camel.toml");
            let output = cmd.output().expect("spawn `camel compile`");
            assert_eq!(
                output.status.code(),
                Some(2),
                "non-UTF-8 document name must be rejected: {}",
                stderr_of(&output)
            );
            assert!(
                stderr_of(&output).to_lowercase().contains("utf-8"),
                "rejection must name UTF-8: {}",
                stderr_of(&output)
            );
            assert!(
                !dir.path().join("out.bin").exists(),
                "no usable output for the non-UTF-8 name"
            );
        }
    }
}

/// The aggregate normalized embedded-byte cap: several valid sources,
/// each under the per-document limit, whose total exceeds 16 MiB.
#[test]
fn compile_embedded_bytes_enforce_aggregate_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Camel.toml"),
        "routes = [\"routes/*.yaml\"]\n",
    )
    .expect("write config");
    std::fs::create_dir_all(dir.path().join("routes")).expect("mkdir routes");
    // Nine 2 MiB route documents: 18 MiB aggregate over the 16 MiB cap,
    // each file individually legal.
    let big: Vec<u8> = std::iter::repeat_n(b'a', 2 * 1024 * 1024).collect();
    for idx in 0..9 {
        std::fs::write(
            dir.path().join("routes").join(format!("big{idx}.yaml")),
            &big,
        )
        .expect("write big route");
    }
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");

    let output = compile_full(
        dir.path(),
        "app.yaml",
        "out.bin",
        None,
        Some("Camel.toml"),
        &[],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("16 MiB"),
        "rejection must name the aggregate limit: {}",
        stderr_of(&output)
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
}

/// A supported multi-document route compiles to a v2 artifact: exit 0,
/// executable permission, v2 trailer, manifest schema 2, and every
/// embedded entry recoverable from the store.
#[test]
fn compile_copies_executable_with_v2_trailer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    const CONFIG: &str = "include = [\"conf/base.toml\"]\nroutes = [\"routes/*.yaml\"]\n[prod]\nlog_level = \"warn\"\n";
    std::fs::write(root.join("Camel.toml"), CONFIG).expect("write config");
    std::fs::create_dir_all(root.join("conf")).expect("mkdir conf");
    std::fs::create_dir_all(root.join("routes")).expect("mkdir routes");
    std::fs::write(
        root.join("conf").join("base.toml"),
        "[default]\ntimeout_ms = 30000\n",
    )
    .expect("write include");
    std::fs::write(
        root.join("routes").join("a.yaml"),
        "routes:\n  - id: a\n    from: timer:a\n    steps:\n      - to: log:a\n",
    )
    .expect("write route");
    std::fs::write(root.join("app.yaml"), ROUTE_DOC).expect("write document");

    let output = compile_full(
        root,
        "app.yaml",
        "app.bin",
        None,
        Some("Camel.toml"),
        &["prod"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "multi-document compile must succeed: {}",
        stderr_of(&output)
    );

    let artifact = root.join("app.bin");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&artifact)
            .expect("artifact metadata")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "artifact must carry the executable bit");
    }

    let bytes = std::fs::read(&artifact).expect("artifact exists");
    let (v2, store, manifest) = decode_v2(&bytes);
    assert_eq!(v2.kind, TrailerKind::Route);

    // v2 trailer framing: the image ends with the 76-byte footer's
    // terminal magic and decodes as a marked v2 artifact (decode_v2).
    assert_eq!(&bytes[bytes.len() - 8..], b"CAMELTR1");

    // Manifest schema 3 with embedded-file metadata mirroring the store.
    assert_eq!(manifest["manifest_schema"], 3);
    assert_eq!(manifest["kind"], "route");
    let embedded = manifest["embedded_files"]
        .as_array()
        .expect("embedded_files");
    let manifest_paths: Vec<&str> = embedded
        .iter()
        .map(|f| f["path"].as_str().expect("path"))
        .collect();
    assert_eq!(manifest_paths, entry_paths(&store));

    // Every embedded entry is recoverable with its source text.
    assert_eq!(store.read("Camel.toml"), Some(CONFIG.as_bytes()));
    assert_eq!(
        store.read("conf/base.toml"),
        Some(&b"[default]\ntimeout_ms = 30000\n"[..])
    );
    assert_eq!(store.read("app.yaml"), Some(ROUTE_DOC.as_bytes()));
    assert!(store.read("prod.profile.toml").is_some());
    assert_eq!(store.index.store_schema, 2);
    // Store index is decodable standalone from the artifact bytes too.
    assert!(StoreIndex::decode(&v2.index, v2.content.len()).is_ok());
}

/// Repeating a `--profile` flag selects the profile once: the synthesized
/// `<name>.profile.toml` fragment is deduplicated (first occurrence
/// preserved) instead of colliding on its logical path.
#[test]
fn compile_repeated_profile_flag_is_deduplicated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    const CONFIG: &str = "[prod]\nlog_level = \"warn\"\n";
    std::fs::write(root.join("Camel.toml"), CONFIG).expect("write config");
    std::fs::write(root.join("app.yaml"), ROUTE_DOC).expect("write document");

    let output = compile_full(
        root,
        "app.yaml",
        "app.bin",
        None,
        Some("Camel.toml"),
        &["prod", "prod"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "repeated profile flags must compile: {}",
        stderr_of(&output)
    );

    let bytes = std::fs::read(root.join("app.bin")).expect("artifact exists");
    let (_, store, _) = decode_v2(&bytes);
    let profiles: Vec<&str> = store
        .index
        .entries
        .iter()
        .filter(|e| e.kind.as_str() == "profile")
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(
        profiles,
        vec!["prod.profile.toml"],
        "repeated profile flags must synthesize exactly one fragment"
    );
    assert_eq!(
        store
            .index
            .config_references
            .iter()
            .filter(|p| p.as_str() == "prod.profile.toml")
            .count(),
        1,
        "the profile fragment must be referenced exactly once"
    );
}

// ---------------------------------------------------------------------------
// r4sign Task 1.2: compile-side signing. A signed compile emits the
// detached `CAMELSG1` envelope at `<artifact>.sig` and a schema-4
// manifest whose signing block records the algorithm, the `blake3:` key
// fingerprint, and the required bit — never any key material. Every
// signing-input violation is a named exit-2 rejection with no artifact
// left behind. Seed files are synthetic pattern bytes written at
// runtime; no committed key material exists.
// ---------------------------------------------------------------------------

/// Compile invocation with extra trailing arguments and injected
/// environment entries (the `CAMEL_COMPILE_SIGNING_KEY` signing input).
/// The environment is otherwise cleared, as in every other battery test.
fn compile_with(
    dir: &Path,
    doc: &str,
    artifact: &str,
    extra_args: &[&str],
    env: &[(&str, &str)],
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear().current_dir(dir);
    cmd.arg("compile").arg(doc).arg("-o").arg(artifact);
    for arg in extra_args {
        cmd.arg(arg);
    }
    for (name, value) in env {
        cmd.env(name, value);
    }
    cmd.output().expect("spawn `camel compile`")
}

/// Write a 32-byte synthetic-pattern ed25519 seed file (never real key
/// material) and return its path.
fn write_seed(dir: &Path, name: &str, byte: u8) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, [byte; 32]).expect("write seed file");
    path
}

/// Signing key derived from a [`write_seed`] pattern byte.
fn seed_key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

#[test]
fn sign_emits_envelope_alongside_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");
    write_seed(dir.path(), "seed.key", 0x52);

    let output = compile_with(
        dir.path(),
        "app.yaml",
        "app.bin",
        &["--sign", "--signing-key", "seed.key"],
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "signed compile must succeed: {}",
        stderr_of(&output)
    );

    let artifact = dir.path().join("app.bin");
    assert!(artifact.is_file(), "artifact must exist after exit 0");
    let sig_bytes = std::fs::read(dir.path().join("app.bin.sig"))
        .expect(".sig envelope must exist beside the artifact");
    assert_eq!(
        sig_bytes.len(),
        signature::ENVELOPE_LEN,
        "envelope must be exactly 148 bytes"
    );
    assert_eq!(&sig_bytes[..8], b"CAMELSG1", "leading magic");
    assert_eq!(
        &sig_bytes[sig_bytes.len() - 8..],
        b"CAMELSG1",
        "terminal magic"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.path().join("app.bin.sig"))
            .expect(".sig metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644, "envelope mode must be 0644");
    }
}

#[test]
fn signed_manifest_records_fingerprint_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");
    write_seed(dir.path(), "seed.key", 0x52);

    let output = compile_with(
        dir.path(),
        "app.yaml",
        "app.bin",
        &["--sign", "--signing-key", "seed.key"],
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));

    let expected_fingerprint = signature::fingerprint(&seed_key(0x52).verifying_key().to_bytes());

    let bytes = std::fs::read(dir.path().join("app.bin")).expect("artifact exists");
    let (_, _, manifest) = decode_v2(&bytes);
    assert_eq!(
        manifest["manifest_schema"], 4,
        "signed compiles emit manifest schema 4: {manifest}"
    );
    assert_eq!(manifest["signing"]["algorithm"], "ed25519ph", "{manifest}");
    assert_eq!(
        manifest["signing"]["key_fingerprint"], expected_fingerprint,
        "manifest records the blake3: fingerprint: {manifest}"
    );
    assert_eq!(manifest["signing"]["required"], false, "{manifest}");

    // `--manifest` prints the signing block without booting.
    let manifest_out = Command::new(dir.path().join("app.bin"))
        .env_clear()
        .arg("--manifest")
        .output()
        .expect("spawn artifact --manifest");
    assert_eq!(
        manifest_out.status.code(),
        Some(0),
        "{}",
        stderr_of(&manifest_out)
    );
    let manifest_text = String::from_utf8_lossy(&manifest_out.stdout).into_owned();
    assert!(
        manifest_text.contains(r#""manifest_schema":4"#),
        "manifest output carries schema 4: {manifest_text}"
    );
    assert!(
        manifest_text.contains("ed25519ph"),
        "manifest output names the algorithm: {manifest_text}"
    );
    assert!(
        manifest_text.contains(&expected_fingerprint),
        "manifest output names the fingerprint: {manifest_text}"
    );

    // No seed material anywhere: neither the hex form nor the raw
    // 32-byte pattern appears in the artifact, the envelope, or the
    // manifest output.
    let seed_hex = "52".repeat(32);
    assert!(
        !String::from_utf8_lossy(&bytes).contains(&seed_hex),
        "seed hex must not appear in the artifact"
    );
    assert!(
        !bytes.windows(32).any(|w| w == [0x52u8; 32]),
        "raw seed bytes must not appear in the artifact"
    );
    let sig_bytes = std::fs::read(dir.path().join("app.bin.sig")).expect(".sig exists");
    assert!(
        !String::from_utf8_lossy(&sig_bytes).contains(&seed_hex),
        "seed hex must not appear in the envelope"
    );
    assert!(
        !manifest_text.contains(&seed_hex),
        "seed hex must not appear in the manifest output"
    );
}

#[test]
fn unsigned_compile_stays_byte_identical() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");

    let first = compile(dir.path(), "app.yaml", "app1.bin", None);
    assert_eq!(first.status.code(), Some(0), "{}", stderr_of(&first));
    let second = compile(dir.path(), "app.yaml", "app2.bin", None);
    assert_eq!(second.status.code(), Some(0), "{}", stderr_of(&second));

    let first_bytes = std::fs::read(dir.path().join("app1.bin")).expect("first artifact");
    let second_bytes = std::fs::read(dir.path().join("app2.bin")).expect("second artifact");
    assert_eq!(
        first_bytes, second_bytes,
        "unsigned compiles must stay byte-identical"
    );
    assert!(
        !dir.path().join("app1.bin.sig").exists(),
        "unsigned compiles emit no envelope"
    );
    assert!(
        !dir.path().join("app2.bin.sig").exists(),
        "unsigned compiles emit no envelope"
    );

    let (_, _, manifest) = decode_v2(&first_bytes);
    assert_eq!(manifest["manifest_schema"], 3, "{manifest}");
    assert!(
        manifest.get("signing").is_none(),
        "unsigned manifest carries no signing key: {manifest}"
    );
}

/// One rejection case: exit 2, the diagnostic names the broken rule, and
/// neither an artifact nor an envelope (nor an envelope temp) is left
/// behind.
fn assert_signing_rejection(dir: &Path, extra_args: &[&str], env: &[(&str, &str)], phrase: &str) {
    let output = compile_with(dir, "app.yaml", "app.bin", extra_args, env);
    let stderr = stderr_of(&output);
    assert_eq!(
        output.status.code(),
        Some(2),
        "expected a named rejection: {stderr}"
    );
    assert!(
        stderr.contains(phrase),
        "diagnostic must name the broken rule ({phrase}): {stderr}"
    );
    assert!(
        !dir.join("app.bin").exists(),
        "no artifact may be left behind: {stderr}"
    );
    assert!(
        !dir.join("app.bin.sig").exists(),
        "no envelope may be left behind: {stderr}"
    );
    assert!(
        !dir.join("app.bin.sig.tmp").exists(),
        "no envelope temp may be left behind: {stderr}"
    );
}

#[test]
fn sign_input_validation_rejections() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");
    write_seed(dir.path(), "seed.key", 0x52);
    std::fs::write(dir.path().join("short.key"), [0x41; 31]).expect("write 31-byte key");
    std::fs::write(dir.path().join("long.key"), [0x41; 33]).expect("write 33-byte key");

    // --signing-key without --sign.
    assert_signing_rejection(
        dir.path(),
        &["--signing-key", "seed.key"],
        &[],
        "--signing-key requires --sign",
    );
    // --require-signature without --sign.
    assert_signing_rejection(
        dir.path(),
        &["--require-signature"],
        &[],
        "--require-signature requires --sign",
    );
    // --sign with neither key source.
    assert_signing_rejection(
        dir.path(),
        &["--sign"],
        &[],
        "--sign requires a signing key",
    );
    // Key file of 31 bytes: path and size named.
    assert_signing_rejection(
        dir.path(),
        &["--sign", "--signing-key", "short.key"],
        &[],
        "is 31 bytes",
    );
    assert_signing_rejection(
        dir.path(),
        &["--sign", "--signing-key", "short.key"],
        &[],
        "short.key",
    );
    // Key file of 33 bytes.
    assert_signing_rejection(
        dir.path(),
        &["--sign", "--signing-key", "long.key"],
        &[],
        "is 33 bytes",
    );
    // Stray signing environment variable without --sign: the variable
    // and --sign are both named.
    assert_signing_rejection(
        dir.path(),
        &[],
        &[("CAMEL_COMPILE_SIGNING_KEY", "seed.key")],
        "CAMEL_COMPILE_SIGNING_KEY",
    );
    assert_signing_rejection(
        dir.path(),
        &[],
        &[("CAMEL_COMPILE_SIGNING_KEY", "seed.key")],
        "--sign",
    );
    // The stray variable is still rejected when another CAMEL_* variable
    // is present too: every other CAMEL_* name rejects as today.
    let output = compile_with(
        dir.path(),
        "app.yaml",
        "app.bin",
        &[],
        &[
            ("CAMEL_COMPILE_SIGNING_KEY", "seed.key"),
            ("CAMEL_OTHER_OVERRIDE", "x"),
        ],
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("CAMEL_OTHER_OVERRIDE"),
        "other CAMEL_* variables keep the clean-environment rejection: {stderr}"
    );
}

/// Recompiling unsigned over a previously signed output removes the
/// stale `<output>.sig` so the fresh schema-3 artifact does not trip
/// the unpaired-envelope boot failure (r_glm holistic finding).
#[test]
fn unsigned_recompile_removes_stale_envelope() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");
    write_seed(dir.path(), "seed.key", 0x52);

    let signed = compile_with(
        dir.path(),
        "app.yaml",
        "app.bin",
        &["--sign", "--signing-key", "seed.key"],
        &[],
    );
    assert_eq!(
        signed.status.code(),
        Some(0),
        "signed compile must succeed: {}",
        stderr_of(&signed)
    );
    let sig = dir.path().join("app.bin.sig");
    assert!(sig.exists(), "signed compile emits the envelope");

    let unsigned = compile_with(dir.path(), "app.yaml", "app.bin", &[], &[]);
    assert_eq!(
        unsigned.status.code(),
        Some(0),
        "unsigned recompile must succeed: {}",
        stderr_of(&unsigned)
    );
    assert!(
        !sig.exists(),
        "unsigned recompile must remove the stale envelope"
    );
}

#[test]
fn sign_key_from_env() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");
    write_seed(dir.path(), "seed_a.key", 0x51);
    write_seed(dir.path(), "seed_b.key", 0x52);

    // Env-only signing works.
    let output = compile_with(
        dir.path(),
        "app.yaml",
        "env.bin",
        &["--sign"],
        &[("CAMEL_COMPILE_SIGNING_KEY", "seed_a.key")],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "env-only signing must succeed: {}",
        stderr_of(&output)
    );
    assert!(
        dir.path().join("env.bin.sig").is_file(),
        "env-only signing emits the envelope"
    );

    // When both sources are present, the argument wins.
    let output = compile_with(
        dir.path(),
        "app.yaml",
        "both.bin",
        &["--sign", "--signing-key", "seed_b.key"],
        &[("CAMEL_COMPILE_SIGNING_KEY", "seed_a.key")],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "arg-over-env signing must succeed: {}",
        stderr_of(&output)
    );
    let bytes = std::fs::read(dir.path().join("both.bin")).expect("artifact exists");
    let (_, _, manifest) = decode_v2(&bytes);
    let arg_fingerprint = signature::fingerprint(&seed_key(0x52).verifying_key().to_bytes());
    let env_fingerprint = signature::fingerprint(&seed_key(0x51).verifying_key().to_bytes());
    assert_ne!(
        arg_fingerprint, env_fingerprint,
        "the two seeds must produce distinct fingerprints"
    );
    assert_eq!(
        manifest["signing"]["key_fingerprint"], arg_fingerprint,
        "the argument key must win over the environment key: {manifest}"
    );
}

#[test]
fn require_signature_flag_flows_to_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");
    write_seed(dir.path(), "seed.key", 0x52);

    let output = compile_with(
        dir.path(),
        "app.yaml",
        "required.bin",
        &["--sign", "--require-signature", "--signing-key", "seed.key"],
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    let bytes = std::fs::read(dir.path().join("required.bin")).expect("artifact exists");
    let (_, _, manifest) = decode_v2(&bytes);
    assert_eq!(
        manifest["signing"]["required"], true,
        "--require-signature must set the required bit: {manifest}"
    );

    let output = compile_with(
        dir.path(),
        "app.yaml",
        "plain.bin",
        &["--sign", "--signing-key", "seed.key"],
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    let bytes = std::fs::read(dir.path().join("plain.bin")).expect("artifact exists");
    let (_, _, manifest) = decode_v2(&bytes);
    assert_eq!(
        manifest["signing"]["required"], false,
        "plain --sign leaves required unset: {manifest}"
    );
}

#[test]
fn envelope_write_failure_removes_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");
    write_seed(dir.path(), "seed.key", 0x52);
    // A directory at the envelope path makes the publish rename fail.
    std::fs::create_dir(dir.path().join("app.bin.sig")).expect("pre-create .sig directory");

    let output = compile_with(
        dir.path(),
        "app.yaml",
        "app.bin",
        &["--sign", "--signing-key", "seed.key"],
        &[],
    );
    let stderr = stderr_of(&output);
    assert_eq!(
        output.status.code(),
        Some(2),
        "envelope-write failure must reject the compile: {stderr}"
    );
    assert!(
        stderr.contains("signature envelope"),
        "diagnostic must name the envelope failure: {stderr}"
    );
    assert!(
        !dir.path().join("app.bin").exists(),
        "no artifact may survive a failed envelope emission"
    );
    assert!(
        !dir.path().join("app.bin.sig.tmp").exists(),
        "the envelope temp file must be cleaned up"
    );
}
