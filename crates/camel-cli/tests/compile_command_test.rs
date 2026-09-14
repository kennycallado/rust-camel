//! Integration tests for `camel compile` (openspec changes `cli-compile`
//! and `multidoc` Task 1.2). The blessed tests exercise `run_compile` end
//! to end by spawning the real `camel` binary with a controlled working
//! directory and a cleared environment, so exit codes, stderr
//! diagnostics, and the absence or presence of the output artifact are
//! asserted exactly as the CLI contract specifies. multidoc Task 1.2
//! switches the writer to the v2 multi-document virtual store: every
//! artifact is decoded through `decode_artifact` and carries
//! `manifest_schema: 2`.

use std::path::Path;
use std::process::{Command, Output};

use camel_cli::compile::store::{StoreIndex, VirtualDocumentStore};
use camel_cli::compile::trailer::{self, DecodedArtifact, TrailerKind, TrailerV2};

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
    // schema-2 manifest, one-entry store.
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
        manifest.contains(r#""manifest_schema":2"#),
        "v2 artifacts carry manifest schema 2: {manifest}"
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
    assert_eq!(store.index.store_schema, 1);
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

    // Manifest schema 2 with embedded-file metadata mirroring the store.
    assert_eq!(manifest["manifest_schema"], 2);
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
    assert_eq!(store.index.store_schema, 1);
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
