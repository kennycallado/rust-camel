//! Integration test for `--manifest` on an asset-bearing artifact
//! (openspec change `r2embed`, Task 2.2). The artifact embeds public-class
//! assets only (secret-entry digest-only exposure is covered by the lib
//! manifest class/digest test); `--manifest` must print asset paths,
//! classes, lengths, digests, `artifact_kind`, and `total_embedded_bytes`
//! and exit 0 without booting any route.

use std::process::Command;

use camel_cli::compile::trailer::{self, DecodedArtifact};

/// A route whose `xslt:` URI operand references a public-class stylesheet
/// asset next to the document.
const ROUTE_DOC: &str = "\
routes:
  - id: demo
    from: timer:tick?period=1s
    steps:
      - to: 'xslt:transform.xslt'
";

/// The stylesheet fixture bytes, embedded verbatim into the store.
const XSLT_BYTES: &[u8] = b"<xsl:stylesheet version=\"1.0\"/>\n";

#[test]
fn artifact_manifest_prints_assets_without_boot() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("app.yaml"), ROUTE_DOC).expect("write document");
    std::fs::write(dir.path().join("transform.xslt"), XSLT_BYTES).expect("write asset");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear().current_dir(dir.path());
    cmd.arg("compile").arg("app.yaml").arg("-o").arg("out.bin");
    let compiled = cmd.output().expect("spawn `camel compile`");
    assert_eq!(
        compiled.status.code(),
        Some(0),
        "compile must succeed; stderr: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    // The artifact decodes as a v2 store artifact carrying a schema-3
    // manifest (library-level sanity so the run assertions below inspect
    // a known-good image).
    let bytes = std::fs::read(dir.path().join("out.bin")).expect("artifact exists");
    let decoded = trailer::decode_artifact(&bytes)
        .expect("trailer must be intact")
        .expect("terminal magic must mark the trailer present");
    let DecodedArtifact::V2(v2) = decoded else {
        panic!("compile must emit a v2 multi-document artifact");
    };
    let body: serde_json::Value =
        serde_json::from_slice(&v2.manifest).expect("manifest must be JSON");
    assert_eq!(body["manifest_schema"], 3, "manifest: {body}");

    // Run the artifact from a clean deploy directory: only the artifact
    // binary is present, no source tree.
    let deploy = tempfile::tempdir().expect("tempdir");
    std::fs::copy(dir.path().join("out.bin"), deploy.path().join("app.bin"))
        .expect("deploy the artifact");
    let mut run = Command::new(deploy.path().join("app.bin"));
    run.env_clear().current_dir(deploy.path());
    run.arg("--manifest");
    let report = run.output().expect("spawn the artifact");
    let stdout = String::from_utf8_lossy(&report.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&report.stderr).into_owned();
    assert_eq!(
        report.status.code(),
        Some(0),
        "--manifest exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let manifest: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is manifest JSON");
    assert_eq!(manifest["manifest_schema"], 3, "manifest: {manifest}");
    assert_eq!(manifest["kind"], "route", "manifest: {manifest}");
    assert_eq!(manifest["artifact_kind"], "server", "manifest: {manifest}");

    // The embedded files: the route document and the stylesheet asset,
    // each with its path, asset class, secret-material class, byte
    // length, and BLAKE3 digest.
    let files = manifest["embedded_files"]
        .as_array()
        .expect("embedded_files array");
    let asset = files
        .iter()
        .find(|f| f["kind"] == "asset")
        .expect("the stylesheet asset must be listed");
    assert_eq!(
        asset["path"], "assets/transform.xslt",
        "manifest: {manifest}"
    );
    assert_eq!(
        asset["asset_class"], "xslt stylesheet",
        "manifest: {manifest}"
    );
    assert_eq!(asset["class"], "public", "manifest: {manifest}");
    assert_eq!(
        asset["digest"],
        blake3::hash(XSLT_BYTES).to_hex().to_string(),
        "manifest: {manifest}"
    );
    assert_eq!(
        asset["length"],
        XSLT_BYTES.len() as u64,
        "manifest: {manifest}"
    );

    // total_embedded_bytes equals the sum of all content-entry lengths.
    let total: u64 = files
        .iter()
        .map(|f| f["length"].as_u64().expect("entry length"))
        .sum();
    assert_eq!(
        manifest["total_embedded_bytes"], total,
        "manifest: {manifest}"
    );

    // Zero route boot: the manifest print never starts a context.
    let all = format!("{stdout}{stderr}");
    assert!(!all.contains("context started"), "no route boot: {all}");
}
