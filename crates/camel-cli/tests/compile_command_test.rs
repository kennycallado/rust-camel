//! Integration tests for `camel compile` (openspec change `cli-compile`,
//! Task 1.2). The blessed tests exercise `run_compile` end to end by
//! spawning the real `camel` binary with a controlled working directory
//! and a cleared environment, so exit codes, stderr diagnostics, and the
//! absence or presence of the output artifact are asserted exactly as the
//! CLI contract specifies.

use std::path::Path;
use std::process::{Command, Output};

use camel_cli::compile::trailer::{self, TrailerKind};

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
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear().current_dir(dir);
    cmd.arg("compile").arg(doc).arg("-o").arg(artifact);
    if let Some(triple) = target {
        cmd.arg("--target").arg(triple);
    }
    cmd.output().expect("spawn `camel compile`")
}

/// stderr of a finished child, for assertion-failure context.
fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
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

    // Decodable route artifact: normalized payload and canonical manifest.
    let decoded = trailer::decode(&bytes)
        .expect("trailer must be intact")
        .expect("terminal magic must mark the trailer present");
    assert_eq!(decoded.kind, TrailerKind::Route);
    assert_eq!(decoded.payload, ROUTE_DOC.as_bytes());
    let manifest = String::from_utf8(decoded.manifest).expect("manifest is UTF-8 JSON");
    assert!(
        manifest.contains(r#""kind":"route""#),
        "manifest kind must be route: {manifest}"
    );
    assert!(
        manifest.contains(r#""source_name":"app.yaml""#),
        "manifest source name must be the input path relative to the cwd: {manifest}"
    );
}

#[test]
fn compile_rejects_external_assets_before_output() {
    // routeFilesFromRoot: external route source.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("routes.yaml"),
        "routeFilesFromRoot:\n  - routes/*.yaml\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "routes.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("routeFilesFromRoot"),
        "rejection must name the field: {}",
        stderr_of(&output)
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Camel.toml in the compile working directory.
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

    // Certificate asset field.
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
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("ingest.job.yaml"),
        "\
routeFilesFromRoot:
  - routes/*.yaml
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
        stderr.contains("routeFilesFromRoot"),
        "rejection must name the external route source: {stderr}"
    );
    assert!(
        stderr.contains("profiles"),
        "rejection must name the config dependency: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
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
    let decoded = trailer::decode(&bytes).expect("trailer").expect("marked");
    assert_eq!(decoded.kind, TrailerKind::Route);
}

/// Regression: the endpoint-scheme check must reach URI-bearing fields
/// beyond `from`/`to` — `wire_tap`, `poll_enrich` (shorthand and `{uri}`
/// forms), and route-level `dead_letter_channel`.
#[test]
fn compile_rejects_forbidden_schemes_in_nested_uri_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - wire_tap: wasm:module.wasm\n      - poll_enrich: xslt:style.xsl\n    error_handler:\n      dead_letter_channel: validator:shape.xsd\n",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    for uri in ["wasm:module.wasm", "xslt:style.xsl", "validator:shape.xsd"] {
        assert!(
            stderr.contains(uri),
            "rejection must name the nested endpoint '{uri}': {stderr}"
        );
    }
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

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
#[test]
fn compile_rejects_forbidden_schemes_in_scatter_gather_endpoints() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - scatter_gather:\n          endpoints:\n            - wasm:module.wasm\n            - xslt:style.xsl\n            - validator:shape.xsd\n",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    for uri in ["wasm:module.wasm", "xslt:style.xsl", "validator:shape.xsd"] {
        assert!(
            stderr.contains(uri),
            "rejection must name the scatter_gather endpoint '{uri}': {stderr}"
        );
    }
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

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
    let decoded = trailer::decode(&bytes).expect("trailer").expect("marked");
    assert_eq!(decoded.kind, TrailerKind::Route);
    assert_eq!(decoded.payload, ROUTE_DOC.as_bytes(), "artifact untouched");
}
