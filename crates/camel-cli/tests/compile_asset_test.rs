//! Integration tests for the revised compile-time asset matrix (openspec
//! change `r2embed`, Task 1.2). Every test spawns the real `camel` binary
//! against a seeded project tree (`tests/fixtures/assets`) and asserts the
//! end-to-end contract: the revised matrix classes — TLS document fields,
//! TLS/gRPC endpoint URI parameters, `xslt:`/`validator:`/`sql:file:` URI
//! operands, and `static_dir` trees — compile into typed `asset` store
//! entries with a complete substitution table, while the still-rejected
//! classes (`wasm:` operands, `Camel.toml` bean plugins and WASM security
//! permissions, escape/missing/placeholder asset paths, file-valued
//! secrets) exit 2 with named diagnostics and no usable output.

use std::path::Path;
use std::process::{Command, Output};

use camel_cli::compile::store::{
    StoreEntryKind, SubstitutionContext, SubstitutionEntry, VirtualDocumentStore,
};
use camel_cli::compile::trailer::{self, DecodedArtifact, TrailerV2};

/// Seed a fresh project directory with the committed fixture tree.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_tree(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/assets")
            .as_path(),
        dir.path(),
    );
    dir
}

/// Recursively copy the fixture tree into the project root.
fn copy_tree(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("fixture entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("fixture type").is_dir() {
            std::fs::create_dir_all(&target).expect("mkdir fixture dir");
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

/// Spawn `camel compile <doc> -o <artifact>` in `dir` with a cleared
/// environment and an optional explicit `--config`.
fn compile(dir: &Path, doc: &str, artifact: &str, config: Option<&str>) -> Output {
    compile_extra(dir, doc, artifact, config, &[])
}

/// `compile` with extra CLI flags appended (e.g. `--embed-secrets`,
/// `--max-payload-bytes`).
fn compile_extra(
    dir: &Path,
    doc: &str,
    artifact: &str,
    config: Option<&str>,
    extra: &[&str],
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear().current_dir(dir);
    cmd.arg("compile").arg(doc).arg("-o").arg(artifact);
    if let Some(config) = config {
        cmd.arg("--config").arg(config);
    }
    cmd.args(extra);
    cmd.output().expect("spawn `camel compile`")
}

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

/// `(path, asset class)` of every asset entry, in canonical order.
fn asset_entries(store: &VirtualDocumentStore) -> Vec<(String, Option<String>)> {
    store
        .index
        .entries
        .iter()
        .filter(|entry| entry.kind == StoreEntryKind::Asset)
        .map(|entry| (entry.path.clone(), entry.asset_class.clone()))
        .collect()
}

/// The substitution entry for one `(document, declared)` pair.
fn substitution<'a>(
    store: &'a VirtualDocumentStore,
    document: &str,
    declared: &str,
) -> &'a SubstitutionEntry {
    store
        .index
        .substitutions
        .iter()
        .find(|entry| entry.document == document && entry.declared == declared)
        .unwrap_or_else(|| panic!("no substitution for ({document}, {declared:?})"))
}

/// Assert every span of `entry` covers exactly the declared string inside
/// the site entry's normalized bytes.
fn assert_spans_match(store: &VirtualDocumentStore, entry: &SubstitutionEntry) {
    let site = std::str::from_utf8(
        store
            .read(&entry.document)
            .unwrap_or_else(|| panic!("site entry {} missing", entry.document)),
    )
    .expect("site entry is UTF-8");
    for span in &entry.spans {
        let slice = &site[span.start as usize..span.end as usize];
        assert_eq!(
            slice, entry.declared,
            "span {}..{} of {} in {} must cover the declared string",
            span.start, span.end, entry.declared, entry.document
        );
    }
}

/// A route whose TLS block references fixture PEMs and whose HTTPS
/// endpoint carries `tlsCert`/`tlsKey` URI parameters.
const TLS_DOC: &str = "\
routes:
  - id: tls
    from: timer:t
    steps:
      - to: 'https://localhost:8443?tlsCert=certs/svc.crt&tlsKey=certs/svc.key'
    tls:
      cert: certs/tls.crt
      key: certs/tls.key
      client_ca: certs/ca.pem
";

#[test]
fn compile_embeds_tls_document_and_uri_param_assets() {
    let dir = project();
    std::fs::write(dir.path().join("app.yaml"), TLS_DOC).expect("write document");

    let output = compile_extra(
        dir.path(),
        "app.yaml",
        "one.bin",
        None,
        &["--embed-secrets"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "revised TLS matrix must compile under the secret opt-in: {}",
        stderr_of(&output)
    );
    let bytes = std::fs::read(dir.path().join("one.bin")).expect("artifact exists");
    let (_, store, _) = decode_v2(&bytes);

    // Typed asset entries under the `assets/` namespace, one per file,
    // with the declared class.
    assert_eq!(
        asset_entries(&store),
        vec![
            ("assets/certs/ca.pem".into(), Some("client CA".into())),
            ("assets/certs/svc.crt".into(), Some("certificate".into())),
            ("assets/certs/svc.key".into(), Some("private key".into())),
            ("assets/certs/tls.crt".into(), Some("certificate".into())),
            ("assets/certs/tls.key".into(), Some("private key".into())),
        ]
    );
    // Asset bytes embed verbatim.
    let fixture = std::fs::read(dir.path().join("certs/tls.crt")).expect("fixture");
    assert_eq!(store.read("assets/certs/tls.crt"), Some(fixture.as_slice()));

    // Substitution entries: document fields are `literal` sites, URI
    // parameters are `uri` sites.
    for declared in ["certs/tls.crt", "certs/tls.key", "certs/ca.pem"] {
        let entry = substitution(&store, "app.yaml", declared);
        assert_eq!(entry.context, SubstitutionContext::Literal, "{declared}");
        assert_eq!(entry.asset, format!("assets/{declared}"));
        assert!(!entry.spans.is_empty());
        assert_spans_match(&store, entry);
    }
    for declared in ["certs/svc.crt", "certs/svc.key"] {
        let entry = substitution(&store, "app.yaml", declared);
        assert_eq!(entry.context, SubstitutionContext::Uri, "{declared}");
        assert_eq!(entry.asset, format!("assets/{declared}"));
        assert_spans_match(&store, entry);
    }

    // Deterministic bytes across runs.
    let output = compile_extra(
        dir.path(),
        "app.yaml",
        "two.bin",
        None,
        &["--embed-secrets"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    let bytes_two = std::fs::read(dir.path().join("two.bin")).expect("second artifact");
    assert_eq!(bytes, bytes_two, "asset embedding must be deterministic");
}

/// The [`TLS_DOC`] body with a leading UTF-8 BOM and `\r\n` line endings:
/// normalization strips the BOM and folds CRLF to LF, so every byte
/// offset thereafter shifts relative to the raw file text.
fn tls_doc_crlf_bom() -> Vec<u8> {
    "\u{feff}routes:\r\n  - id: tls\r\n    from: timer:t\r\n    steps:\r\n      - to: 'https://localhost:8443?tlsCert=certs/svc.crt&tlsKey=certs/svc.key'\r\n    tls:\r\n      cert: certs/tls.crt\r\n      key: certs/tls.key\r\n      client_ca: certs/ca.pem\r\n"
        .as_bytes()
        .to_vec()
}

#[test]
fn compile_pins_span_offsets_to_normalized_bytes() {
    // Substitution spans are byte offsets into the NORMALIZED document
    // (BOM stripped, CRLF/CR folded). Every LF-only, BOM-free fixture
    // leaves raw == normalized, so this fixture is the one that fails
    // the moment a raw-text search is substituted for the normalized
    // search: the declared references sit after several CRLF lines, so
    // raw offsets overshoot the decoded store's bytes.
    let dir = project();
    std::fs::write(dir.path().join("app.yaml"), tls_doc_crlf_bom()).expect("write document");

    let output = compile_extra(
        dir.path(),
        "app.yaml",
        "one.bin",
        None,
        &["--embed-secrets"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "BOM + CRLF document must compile under the secret opt-in: {}",
        stderr_of(&output)
    );
    let bytes = std::fs::read(dir.path().join("one.bin")).expect("artifact exists");
    let (_, store, _) = decode_v2(&bytes);

    // The embedded entry point is normalized: no BOM, no CR.
    let site = std::str::from_utf8(store.read("app.yaml").expect("entry point")).expect("UTF-8");
    assert!(!site.starts_with('\u{feff}'), "BOM must be stripped");
    assert!(!site.contains('\r'), "CRLF/CR must fold to LF");

    // Same typed matrix as the LF-only TLS fixture.
    assert_eq!(
        asset_entries(&store),
        vec![
            ("assets/certs/ca.pem".into(), Some("client CA".into())),
            ("assets/certs/svc.crt".into(), Some("certificate".into())),
            ("assets/certs/svc.key".into(), Some("private key".into())),
            ("assets/certs/tls.crt".into(), Some("certificate".into())),
            ("assets/certs/tls.key".into(), Some("private key".into())),
        ]
    );

    // Every span must slice exactly the declared spelling out of the
    // decoded (normalized) entry. A raw-offset span lands misaligned.
    for declared in ["certs/tls.crt", "certs/tls.key", "certs/ca.pem"] {
        let entry = substitution(&store, "app.yaml", declared);
        assert_eq!(entry.context, SubstitutionContext::Literal, "{declared}");
        assert!(!entry.spans.is_empty(), "{declared}");
        assert_spans_match(&store, entry);
    }
    for declared in ["certs/svc.crt", "certs/svc.key"] {
        let entry = substitution(&store, "app.yaml", declared);
        assert_eq!(entry.context, SubstitutionContext::Uri, "{declared}");
        assert_spans_match(&store, entry);
    }

    // Discrimination guard: raw offsets are strictly greater than the
    // normalized offsets the spans must use. If this ever stops holding
    // the fixture no longer exercises normalization shift.
    let raw = String::from_utf8(tls_doc_crlf_bom()).expect("fixture UTF-8");
    for declared in ["certs/svc.crt", "certs/tls.crt"] {
        let raw_start = raw.find(declared).expect("declared in raw text");
        let normalized_start = site.find(declared).expect("declared in normalized text");
        assert!(
            raw_start > normalized_start,
            "fixture must shift raw offsets for {declared}: raw {raw_start} vs normalized \
             {normalized_start}"
        );
    }
}

#[test]
fn compile_collects_grpc_tls_uri_params() {
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "\
routes:
  - id: grpc-server
    from: 'grpc:svc?transport=tls&serverCertPath=grpc/server.crt&serverKeyPath=grpc/server.key&clientCaPath=grpc/server-ca.pem'
    steps:
      - to: log:server
  - id: grpc-client
    from: timer:t
    steps:
      - to: 'grpc:svc?transport=tls&caCertPath=grpc/client-ca.pem&clientCertPath=grpc/client.crt&clientKeyPath=grpc/client.key'
",
    )
    .expect("write document");

    let output = compile_extra(
        dir.path(),
        "app.yaml",
        "out.bin",
        None,
        &["--embed-secrets"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "gRPC TLS URI parameters must compile under the secret opt-in: {}",
        stderr_of(&output)
    );
    let (_, store, _) = decode_v2(&std::fs::read(dir.path().join("out.bin")).expect("artifact"));

    // One asset entry per referenced file, with the correct class.
    assert_eq!(
        asset_entries(&store),
        vec![
            ("assets/grpc/client-ca.pem".into(), Some("client CA".into()),),
            ("assets/grpc/client.crt".into(), Some("certificate".into())),
            ("assets/grpc/client.key".into(), Some("private key".into())),
            ("assets/grpc/server-ca.pem".into(), Some("client CA".into()),),
            ("assets/grpc/server.crt".into(), Some("certificate".into()),),
            ("assets/grpc/server.key".into(), Some("private key".into())),
        ]
    );
    for declared in [
        "grpc/server.crt",
        "grpc/server.key",
        "grpc/server-ca.pem",
        "grpc/client-ca.pem",
        "grpc/client.crt",
        "grpc/client.key",
    ] {
        let entry = substitution(&store, "app.yaml", declared);
        assert_eq!(entry.context, SubstitutionContext::Uri, "{declared}");
        assert_spans_match(&store, entry);
    }
}

#[test]
fn compile_collects_uri_scheme_and_sql_file_assets() {
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "\
routes:
  - id: styles
    from: timer:t
    steps:
      - to: 'xslt:transform.xslt'
      - poll_enrich: 'validator:schema.xsd'
  - id: file-query
    from: 'sql:file:query.sql?db_url=postgres://db.local/app'
    steps:
      - to: log:row
  - id: inline-query
    from: timer:t
    steps:
      - to: 'sql:SELECT 1?db_url=postgres://db.local/app'
",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "xslt/validator/sql:file assets must compile: {}",
        stderr_of(&output)
    );
    let (_, store, _) = decode_v2(&std::fs::read(dir.path().join("out.bin")).expect("artifact"));

    // One asset entry per reference; the inline-query `sql:` endpoint
    // carries a runtime datasource query, not an asset, and adds none.
    assert_eq!(
        asset_entries(&store),
        vec![
            ("assets/query.sql".into(), Some("sql file".into())),
            ("assets/schema.xsd".into(), Some("xsd schema".into())),
            (
                "assets/transform.xslt".into(),
                Some("xslt stylesheet".into()),
            ),
        ]
    );
    let entry = substitution(&store, "app.yaml", "query.sql");
    assert_eq!(entry.context, SubstitutionContext::Uri);
    assert_spans_match(&store, entry);
    assert!(
        store
            .index
            .substitutions
            .iter()
            .all(|entry| entry.declared != "SELECT 1")
    );
}

#[test]
fn compile_rejects_wasm_uri_operand() {
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "\
routes:
  - id: wasm
    from: timer:t
    steps:
      - to: 'wasm:module.wasm'
",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("wasm:module.wasm"),
        "rejection must name the operand: {stderr}"
    );
    assert!(
        stderr.contains("deferral"),
        "rejection must state the recorded R2 deferral: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
}

#[test]
fn compile_rejects_beans_plugin_and_permissions_wasm_path() {
    let dir = project();
    std::fs::write(dir.path().join("app.yaml"), TLS_DOC).expect("write document");

    // A [beans.<name>] plugin entry fails closed, naming the bean.
    std::fs::write(
        dir.path().join("beans.toml"),
        "[beans.loader]\nplugin = \"file-loader\"\n",
    )
    .expect("write beans config");
    let output = compile(dir.path(), "app.yaml", "out1.bin", Some("beans.toml"));
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("loader") && stderr.contains("plugin"),
        "rejection must name the bean and its plugin: {stderr}"
    );
    assert!(!dir.path().join("out1.bin").exists(), "no output artifact");

    // A [security.permissions.<name>] WASM provider path fails closed,
    // naming the policy.
    std::fs::write(
        dir.path().join("perms.toml"),
        "[security.permissions.check]\nprovider = \"wasm\"\npath = \"check-policy.wasm\"\n",
    )
    .expect("write permissions config");
    let output = compile(dir.path(), "app.yaml", "out2.bin", Some("perms.toml"));
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("check"),
        "rejection must name the security permission: {stderr}"
    );
    assert!(!dir.path().join("out2.bin").exists(), "no output artifact");

    // A [security.policies.wasm.<name>] module declaration fails closed
    // too (review F2): the policy module reads a host file-system path
    // at boot, and security-policy WASM never reaches an artifact in R2.
    std::fs::write(
        dir.path().join("policies.toml"),
        "[security.policies.wasm.corp-auth]\npath = \"plugins/authz.wasm\"\n",
    )
    .expect("write policies config");
    let output = compile(dir.path(), "app.yaml", "out3.bin", Some("policies.toml"));
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("corp-auth"),
        "rejection must name the security policy: {stderr}"
    );
    assert!(!dir.path().join("out3.bin").exists(), "no output artifact");

    // A selected profile section merges into the effective root at
    // runtime, so its bean plugins fail closed exactly like root-level
    // ones (review F2): `[prod.beans.<name>]` with `--profile prod`.
    std::fs::write(
        dir.path().join("prod-beans.toml"),
        "[prod.beans.loader]\nplugin = \"file-loader\"\n",
    )
    .expect("write profile beans config");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear().current_dir(&dir);
    cmd.arg("compile").arg("app.yaml").arg("-o").arg("out4.bin");
    cmd.arg("--config").arg("prod-beans.toml");
    cmd.arg("--profile").arg("prod");
    let output = cmd.output().expect("spawn `camel compile`");
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("loader") && stderr.contains("plugin"),
        "rejection must name the profile-section bean and its plugin: {stderr}"
    );
    assert!(!dir.path().join("out4.bin").exists(), "no output artifact");
}

#[test]
fn compile_registry_name_wasm_field_is_not_an_asset() {
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "\
routes:
  - id: wasm-api
    from: direct:start
    security_policy:
      wasm: \"registry-policy\"
    steps:
      - to: log:info
",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "the wasm registry-name field compiles as ordinary data: {}",
        stderr_of(&output)
    );
    let (_, store, _) = decode_v2(&std::fs::read(dir.path().join("out.bin")).expect("artifact"));
    assert!(
        asset_entries(&store).is_empty(),
        "no asset entry may be created for a registry-name wasm field"
    );
    assert!(store.index.substitutions.is_empty());
}

#[test]
fn compile_expands_static_dir_deterministically_and_fails_on_symlink() {
    let dir = project();
    // Varied creation order: the tree enumerates in a different order
    // than the canonical sort.
    std::fs::create_dir_all(dir.path().join("www/sub")).expect("mkdir www/sub");
    std::fs::write(dir.path().join("www/b.txt"), "static beta\n").expect("write b.txt");
    std::fs::write(dir.path().join("www/sub/c.txt"), "static gamma nested\n").expect("write c.txt");
    std::fs::write(dir.path().join("www/a.txt"), "static alpha\n").expect("write a.txt");
    // A FIFO: a non-regular file that is not a symlink — skipped.
    let fifo = Command::new("mkfifo")
        .arg(dir.path().join("www/pipe.fifo"))
        .output()
        .expect("spawn mkfifo");
    assert!(fifo.status.success(), "mkfifo must succeed for the fixture");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: static\n    from: timer:t\n    steps:\n      - to: log:x\nstatic_dir: www\n",
    )
    .expect("write document");

    // Two runs embed the same sorted regular-file entries and never the
    // FIFO; bytes are deterministic.
    let output = compile(dir.path(), "app.yaml", "one.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "static tree with a FIFO must compile: {}",
        stderr_of(&output)
    );
    let bytes = std::fs::read(dir.path().join("one.bin")).expect("artifact");
    let (_, store, _) = decode_v2(&bytes);
    assert_eq!(
        asset_entries(&store),
        vec![
            ("assets/www/a.txt".into(), Some("static directory".into())),
            ("assets/www/b.txt".into(), Some("static directory".into())),
            (
                "assets/www/sub/c.txt".into(),
                Some("static directory".into()),
            ),
        ]
    );
    // The declared directory spelling records NO substitution site in
    // R2 (review F4): dir-level substitution semantics are deferred to
    // the Phase-3 registry review, and the expanded files resolve
    // through that registry instead.
    assert!(
        store
            .index
            .substitutions
            .iter()
            .all(|entry| entry.declared != "www"),
        "a static tree declaration must not produce a substitution entry"
    );

    let output = compile(dir.path(), "app.yaml", "two.bin", None);
    assert_eq!(output.status.code(), Some(0), "{}", stderr_of(&output));
    let bytes_two = std::fs::read(dir.path().join("two.bin")).expect("second artifact");
    assert_eq!(bytes, bytes_two, "static expansion must be deterministic");

    // A symlink inside the tree fails compilation closed, naming the
    // symlink, instead of being silently skipped or followed.
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        dir.path().join("www/a.txt"),
        dir.path().join("www/link.txt"),
    )
    .expect("create symlink fixture");
    let output = compile(dir.path(), "app.yaml", "three.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("link.txt"),
        "rejection must name the symlink: {stderr}"
    );
    assert!(
        !dir.path().join("three.bin").exists(),
        "no output artifact for a symlinked static tree"
    );
}

#[test]
fn compile_nested_static_trees_dedupe_per_file() {
    // Nested tree declarations (`static/www` + `static/www/sub`) share
    // files: the `claimed` set is consulted per walked FILE, so the
    // store keeps exactly one asset entry per unique file — never a
    // duplicate that surfaces as an opaque DuplicatePath (review F3).
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "\
routes:
  - id: static
    from: timer:t
    steps:
      - to: log:x
static_dir:
  - static/www
  - static/www/sub
",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "overlapping static trees must compile: {}",
        stderr_of(&output)
    );
    let (_, store, _) = decode_v2(&std::fs::read(dir.path().join("out.bin")).expect("artifact"));

    // One entry per FILE, despite both trees covering www/sub/c.txt.
    assert_eq!(
        asset_entries(&store),
        vec![
            (
                "assets/static/www/a.txt".into(),
                Some("static directory".into())
            ),
            (
                "assets/static/www/b.txt".into(),
                Some("static directory".into())
            ),
            (
                "assets/static/www/sub/c.txt".into(),
                Some("static directory".into()),
            ),
        ]
    );
    // Tree declarations record no substitution sites (review F4).
    assert!(store.index.substitutions.is_empty());
}

#[test]
fn compile_rejects_asset_escape_missing_placeholder_and_secret() {
    // Traversal: `..` cannot escape the selected root.
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\n    tls:\n      cert: ../outside.pem\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("cert") && stderr.contains("certificate"),
        "rejection must name the field and class: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Symlink escape: a declared path inside the root resolving outside
    // it through a symlink is confined.
    let dir = project();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hostname", dir.path().join("certs/escape.pem"))
        .expect("create escape symlink");
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\n    tls:\n      cert: certs/escape.pem\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("escape.pem") && stderr.contains("certificate"),
        "rejection must name the field, class, and path: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Missing file: reject-missing at compile time.
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\n    tls:\n      cert: certs/absent.pem\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("certs/absent.pem") && stderr.contains("certificate"),
        "rejection must name the field, class, and path: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Dynamic placeholder: an unknowable asset path cannot be embedded.
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\n    tls:\n      cert: ${env:TLS_CERT_PATH}\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("cert") && stderr.contains("${env:"),
        "rejection must name the field and the placeholder: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Absolute TLS URI parameter: a root-relative compile-known path is
    // required.
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: 'https://localhost:8443?tlsCert=/etc/pki/cert.pem'\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("tlsCert") && stderr.contains("root-relative"),
        "rejection must name the parameter and require a root-relative path: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // File-valued secret fields stay rejected.
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "routes:\n  - id: r\n    from: timer:t\n    steps:\n      - to: log:x\nsecrets:\n  file: secret.txt\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("secrets") && stderr.contains("secret file"),
        "rejection must name the secret field and class: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");
}

#[test]
fn compile_dedupes_shared_asset_targets_with_alias_table() {
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "\
routes:
  - id: one
    from: timer:t
    steps:
      - to: log:one
    tls:
      client_ca: certs/ca.pem
  - id: two
    from: timer:t
    steps:
      - to: log:two
    tls:
      client_ca: ./certs/ca.pem
",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "two spellings of one CA must compile: {}",
        stderr_of(&output)
    );
    let (_, store, _) = decode_v2(&std::fs::read(dir.path().join("out.bin")).expect("artifact"));

    // Exactly one asset entry for the canonical target.
    let assets = asset_entries(&store);
    assert_eq!(
        assets,
        vec![(
            "assets/certs/ca.pem".to_string(),
            Some("client CA".to_string())
        )],
        "shared targets must dedupe into one entry"
    );

    // Both alias spellings map to that single entry.
    for declared in ["certs/ca.pem", "./certs/ca.pem"] {
        let entry = substitution(&store, "app.yaml", declared);
        assert_eq!(entry.asset, "assets/certs/ca.pem", "{declared}");
        assert_eq!(entry.context, SubstitutionContext::Literal);
        assert_spans_match(&store, entry);
    }
    assert_eq!(
        store
            .index
            .substitutions
            .iter()
            .filter(|entry| entry.document == "app.yaml")
            .count(),
        2,
        "both alias spellings stay recorded"
    );

    // Boundary-aware sites (review F1): each alias entry records exactly
    // ONE span — the nested `certs/ca.pem` spelling inside
    // `./certs/ca.pem` is a substring of the longer spelling, not a
    // site — and the two spans are disjoint, so a last-offset-first
    // rewrite can never double-rewrite one region.
    let ca = substitution(&store, "app.yaml", "certs/ca.pem");
    let dotted = substitution(&store, "app.yaml", "./certs/ca.pem");
    assert_eq!(ca.spans.len(), 1, "exactly one site: {:?}", ca.spans);
    assert_eq!(
        dotted.spans.len(),
        1,
        "exactly one site: {:?}",
        dotted.spans
    );
    let (a, b) = (ca.spans[0], dotted.spans[0]);
    assert!(
        a.end <= b.start || b.end <= a.start,
        "alias spans must be non-nested: {a:?} vs {b:?}"
    );

    // Zero cross-entry span overlap anywhere in the table: the runtime
    // rewrite walks spans last-offset-first assuming disjointness.
    for (i, x) in store.index.substitutions.iter().enumerate() {
        for y in &store.index.substitutions[i + 1..] {
            if x.document != y.document {
                continue;
            }
            for s in &x.spans {
                for t in &y.spans {
                    assert!(
                        s.end <= t.start || t.end <= s.start,
                        "cross-entry span overlap between {:?} and {:?} in {}: {s:?} vs {t:?}",
                        x.declared,
                        y.declared,
                        x.document
                    );
                }
            }
        }
    }
}

#[test]
fn compile_mixed_context_declared_string_records_context_per_site() {
    // One declared string in both a literal field and an `xslt:` URI
    // operand of the same document: the table records one entry PER
    // CONTEXT, each with its own span — the mixed declaration never
    // collapses into a first-seen-wins entry (review F1).
    let dir = project();
    std::fs::write(
        dir.path().join("app.yaml"),
        "\
routes:
  - id: styled
    from: timer:t
    xslt: transform.xslt
    steps:
      - to: 'xslt:transform.xslt'
",
    )
    .expect("write document");

    let output = compile(dir.path(), "app.yaml", "out.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "a mixed literal/uri declaration must compile: {}",
        stderr_of(&output)
    );
    let (_, store, _) = decode_v2(&std::fs::read(dir.path().join("out.bin")).expect("artifact"));

    let entries: Vec<&SubstitutionEntry> = store
        .index
        .substitutions
        .iter()
        .filter(|entry| entry.declared == "transform.xslt")
        .collect();
    assert_eq!(entries.len(), 2, "one entry per site context");
    let literal = entries
        .iter()
        .find(|entry| entry.context == SubstitutionContext::Literal)
        .expect("literal site entry");
    let uri = entries
        .iter()
        .find(|entry| entry.context == SubstitutionContext::Uri)
        .expect("uri site entry");
    for entry in [literal, uri] {
        assert_eq!(entry.asset, "assets/transform.xslt");
        assert_eq!(entry.spans.len(), 1, "one site per context: {entry:?}");
        assert_spans_match(&store, entry);
    }
    assert_ne!(literal.spans[0], uri.spans[0], "sites are disjoint");
    assert_eq!(
        asset_entries(&store),
        vec![(
            "assets/transform.xslt".to_string(),
            Some("xslt stylesheet".to_string())
        )]
    );
}

/// A minimal valid route document (no assets).
const PLAIN_DOC: &str = "\
routes:
  - id: plain
    from: timer:t
    steps:
      - to: log:x
";

#[test]
fn compile_max_payload_bytes_flag_moves_cap() {
    let dir = project();
    // Programmatic >16 MiB <32 MiB fixture: the bulk sits in a YAML
    // comment, so the document stays schema-valid while its normalized
    // byte length clears the default cap. No multi-MB fixture is
    // committed; it is generated under TMPDIR.
    let prefix = "routes:\n  - id: big\n    from: timer:t\n    steps:\n      - to: log:x\n";
    let padding = 17 * 1024 * 1024;
    let doc = format!("{prefix}# {}\n", "a".repeat(padding));
    std::fs::write(dir.path().join("big.yaml"), &doc).expect("write oversized document");
    let total = doc.len();

    // Without the flag: the default 16 MiB cap rejects, naming the
    // aggregate total AND the default cap value.
    let output = compile(dir.path(), "big.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains(&total.to_string()),
        "rejection must name the aggregate total {total}: {stderr}"
    );
    assert!(
        stderr.contains("16777216"),
        "rejection must name the default 16 MiB cap: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // Raising the cap accepts the very same input.
    let output = compile_extra(
        dir.path(),
        "big.yaml",
        "out.bin",
        None,
        &["--max-payload-bytes", "33554432"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "the raised cap must accept the oversized set: {}",
        stderr_of(&output)
    );
    assert!(
        dir.path().join("out.bin").exists(),
        "artifact written under the raised cap"
    );
}

#[test]
fn compile_rejects_invalid_max_payload_bytes() {
    let dir = project();
    std::fs::write(dir.path().join("app.yaml"), PLAIN_DOC).expect("write document");

    // Zero and non-numeric values exit 2 with a value diagnostic before
    // any compile work, and no artifact is written.
    for value in ["0", "abc"] {
        let output = compile_extra(
            dir.path(),
            "app.yaml",
            "out.bin",
            None,
            &["--max-payload-bytes", value],
        );
        assert_eq!(output.status.code(), Some(2), "value {value}");
        let stderr = stderr_of(&output);
        assert!(
            stderr.contains(value),
            "rejection must name the invalid value '{value}': {stderr}"
        );
        assert!(
            !dir.path().join("out.bin").exists(),
            "no output artifact for value {value}"
        );
    }
}

#[test]
fn artifact_mode_is_0700_with_secret_class_and_0755_without() {
    let dir = project();

    // A private-key asset under --embed-secrets: the artifact is
    // written mode 0700.
    std::fs::write(
        dir.path().join("key.yaml"),
        "routes:\n  - id: tls\n    from: timer:t\n    steps:\n      - to: log:x\n    tls:\n      key: certs/tls.key\n",
    )
    .expect("write document");
    let output = compile_extra(
        dir.path(),
        "key.yaml",
        "secret.bin",
        None,
        &["--embed-secrets"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "secret-bearing compile must succeed under opt-in: {}",
        stderr_of(&output)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.path().join("secret.bin"))
            .expect("artifact exists")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "secret-bearing artifact must not be world-executable"
        );
    }
    // The temp sibling carries the secret material for the whole copy
    // window; it must not survive the rename.
    assert!(
        !dir.path().join("secret.bin.tmp").exists(),
        "no <output>.tmp sibling may survive a successful compile"
    );

    // A planted symlink at the temp path must be refused by the exclusive
    // create instead of being followed and truncated. RED on the pre-fix
    // code: `File::create` followed the symlink and clobbered the innocent
    // target while the compile still exited 0.
    let innocent = dir.path().join("innocent.txt");
    std::fs::write(&innocent, b"do not clobber").expect("write innocent file");
    std::os::unix::fs::symlink(&innocent, dir.path().join("clobber.bin.tmp"))
        .expect("plant symlink at the temp path");
    let output = compile_extra(
        dir.path(),
        "key.yaml",
        "clobber.bin",
        None,
        &["--embed-secrets"],
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "a planted temp symlink must fail the compile closed: {}",
        stderr_of(&output)
    );
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("clobber.bin.tmp"),
        "the rejection must name the refusing temp path: {stderr}"
    );
    assert_eq!(
        std::fs::read(&innocent).expect("innocent file survives"),
        b"do not clobber",
        "the exclusive create must not follow-and-truncate the planted symlink"
    );
    assert!(
        !dir.path().join("clobber.bin").exists(),
        "no artifact is published when the temp open is refused"
    );

    // A certificate-only route (no private-key entry): the artifact
    // keeps the default 0755.
    std::fs::write(
        dir.path().join("cert.yaml"),
        "routes:\n  - id: tls\n    from: timer:t\n    steps:\n      - to: log:x\n    tls:\n      cert: certs/tls.crt\n",
    )
    .expect("write document");
    let output = compile(dir.path(), "cert.yaml", "public.bin", None);
    assert_eq!(
        output.status.code(),
        Some(0),
        "secret-free compile must succeed: {}",
        stderr_of(&output)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.path().join("public.bin"))
            .expect("artifact exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "secret-free artifact stays 0755");
    }

    // The same certificate-only route compiled WITH `--embed-secrets`:
    // the opt-in flag alone must not tighten the mode. The artifact mode
    // follows the store class inventory (whether a private-key entry was
    // actually collected), never the flag.
    let output = compile_extra(
        dir.path(),
        "cert.yaml",
        "public_flagged.bin",
        None,
        &["--embed-secrets"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "secret-free compile with the opt-in flag must succeed: {}",
        stderr_of(&output)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.path().join("public_flagged.bin"))
            .expect("artifact exists")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o755,
            "opt-in flag with no secret entry must not tighten the mode"
        );
    }
}

#[test]
fn compile_secret_embed_requires_opt_in_flag() {
    let dir = project();
    std::fs::write(
        dir.path().join("key.yaml"),
        "routes:\n  - id: tls\n    from: timer:t\n    steps:\n      - to: log:x\n    tls:\n      key: certs/tls.key\n",
    )
    .expect("write document");

    // Without --embed-secrets: fail closed BEFORE any output, naming
    // the field AND the secret class.
    let output = compile(dir.path(), "key.yaml", "out.bin", None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("'key'"),
        "rejection must name the field: {stderr}"
    );
    assert!(
        stderr.contains("private key"),
        "rejection must name the secret class: {stderr}"
    );
    assert!(!dir.path().join("out.bin").exists(), "no output artifact");

    // With --embed-secrets: the secret embeds into the store normally.
    let output = compile_extra(
        dir.path(),
        "key.yaml",
        "out.bin",
        None,
        &["--embed-secrets"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "opted-in secret embed must compile: {}",
        stderr_of(&output)
    );
    let bytes = std::fs::read(dir.path().join("out.bin")).expect("artifact");
    let (_, store, _) = decode_v2(&bytes);
    assert_eq!(
        asset_entries(&store),
        vec![("assets/certs/tls.key".into(), Some("private key".into()))],
        "the secret entry is embedded"
    );
}
