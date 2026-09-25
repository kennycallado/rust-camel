//! Integration tests for the substitution/materialization runtime of
//! asset-bearing artifacts (openspec change `r2embed`, Tasks 3.2 and
//! 3.3). The suite compiles each distinct document ONCE with the real
//! `camel compile` into a shared immutable fixture — the artifact
//! embeds a copy of the full `camel` binary, so per-test compiles would
//! write gigabytes under parallel execution. Every test deploys the
//! fixture artifact into its own source-free directory and runs it
//! through [`run_embedded_document_code`] in a harness CHILD of this
//! test binary (the same re-exec pattern as `compiled_artifact_test.rs`).
//!
//! The watched-temp-root technique gives the tests an external view of
//! the confined materialization: the child's `TMPDIR` points at a
//! directory the parent controls, so materialized per-boot directories
//! (`camel-assets-…`) are observable while the artifact runs and their
//! absence after exit proves guard cleanup. Materialized bytes are
//! read mid-run: the "context started" marker is emitted only after
//! the pre-boot prepare phase, so everything materialized is on disk
//! by the time the marker is visible, and the guard removes the
//! directory at exit — byte-identity assertions must run before TERM.
//!
//! Fixture shapes are the bootable TLS shapes of this runtime (Task
//! 3.3 deviations, each empirically established against HEAD
//! 594f3fd6, see the per-test docs):
//! - a route-level `tls:` block is an unknown DSL field at discovery
//!   (`YAML parse error: unknown field: tls`), so the bootable TLS
//!   document-field-free shape carries TLS in `https://` URI
//!   parameters; a REAL certificate/key pair (EC P-256, self-signed,
//!   throwaway) drives a genuine rustls listener via
//!   `from: https://…` — the listener parses the PEMs at boot, so boot
//!   success proves the substituted confined files were opened;
//! - the ONLY legal document-field TLS sites (`cert`/`key`/`client_ca`
//!   under a `tls:` block) cannot boot (above), so the tls-block
//!   fixture exists for the pre-boot failure paths only: the
//!   strengthened guard case (exit 2 AFTER materialization) and the
//!   substitution path-safety clauses (which fire before discovery);
//! - asset logical paths use the block-faithful NESTED shapes: the TLS
//!   certificate/key fixtures live under one shared `certs/` directory —
//!   the exact shape the Task 3.2 missing-parent defect rejected — so
//!   every shared-subdirectory write boots, materializes, and cleans up;
//! - the job-kind fixture carries an `xslt:` stylesheet AND a bootable
//!   TLS certificate/key pair under the same shared `certs/` directory
//!   (the block's "document-field TLS cert" is unbootable, so the pair
//!   rides `https://` URI parameters — the bootable shape of the route
//!   listener); it reaches `run_embedded_job_store` through the same
//!   rewritten store and asserts confinement/substitution/cleanup
//!   identical to the route kind.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use camel_bundles::AssetRegistry;
use camel_cli::compile::runtime::{ArtifactArgs, EmbeddedRequest};
use camel_cli::compile::trailer;

use common::{KillOnDrop, spawn_drained, wait_for_marker};

/// Env var that marks a harness child: its value is the artifact path.
const CHILD_ENV: &str = "CAMEL_ASSET_ROUNDTRIP_CHILD";

/// Serializes every test that boots a fixture artifact. Each child
/// reads and boots a ~287 MiB image, and the deploy root may sit on a
/// slow filesystem: concurrent boots starve each other's observation
/// windows, so the boots run one at a time (the TLS listener also binds
/// `127.0.0.1:18443`, which the serialization covers for free).
static BOOT_SERIAL: Mutex<()> = Mutex::new(());

/// The free space a fixture root must offer: seven compiled artifacts
/// (~287 MiB each, the embedded `camel` binary) plus deploy copies.
const REQUIRED_ROOT_FREE: u64 = 3 * 1024 * 1024 * 1024;

/// Free bytes at `path` per statvfs, or `None` when unavailable.
fn free_bytes(path: &Path) -> Option<u64> {
    let c_path = std::ffi::CString::new(path.as_os_str().to_str()?).ok()?;
    // SAFETY: `c_path` outlives the call; `statvfs` writes only into
    // `stat`, a plain-old-data struct owned here.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

/// The root directory for this suite's large writes: the OS temp
/// directory when it has room, otherwise the workspace target directory
/// (resolved from this file's manifest dir — cargo runs test processes
/// with the crate directory as cwd, where a bare `target` would not
/// resolve).
fn fixture_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let workspace_target = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target");
        let target = std::env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or(workspace_target);
        let candidates = [std::env::temp_dir(), target];
        for candidate in &candidates {
            if free_bytes(candidate).is_some_and(|free| free >= REQUIRED_ROOT_FREE) {
                return candidate.clone();
            }
        }
        panic!(
            "no fixture root has the required free space: need {REQUIRED_ROOT_FREE} bytes, \
             candidates [{}]",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
    .clone()
}

/// The single cache directory for this test process's compiled fixtures.
fn fixture_dir() -> PathBuf {
    fixture_root().join(format!("camel-asset-roundtrip-{}", std::process::id()))
}

/// A one-shot job whose pipeline fails deterministically AFTER boot (the
/// `direct:boom` consumer sends to a `direct:` endpoint with no
/// consumer) while an UNUSED inline route carries the `xslt:` asset
/// reference: the stylesheet materializes at boot, then the pipeline
/// fails — the guard must remove the per-boot directory at exit.
const FAILING_XSLT_JOB_DOC: &str = "\
execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:boom
routes:
  - id: job-fail
    from: direct:boom
    steps:
      - to: direct:missing-consumer
  - id: unused-xslt
    from: direct:unused
    steps:
      - to: 'xslt:transform.xslt'
";

/// A route with a TLS block and TLS endpoint URI parameters: five
/// materialized-class assets (certificate / private key / client CA).
/// TLS resolution is embedded-only — the fixtures never need to exist at
/// run time. This artifact is used only for the pre-boot failure paths
/// (unavailable temp; path-safety clauses; the exit-2-after-
/// materialization guard case): a route-level `tls:` block is an
/// unknown DSL field at discovery, so this artifact never boots.
const TLS_ROUTE_DOC: &str = "\
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

/// The bootable TLS listener route (Task 3.3): a real `https://`
/// CONSUMER (the listener parses `tlsCert`/`tlsKey` PEMs via rustls at
/// boot — boot success proves the substituted confined files were
/// opened) plus a producer route whose `tlsKey` names a SECOND logical
/// path with byte-identical content (identical-byte secret population).
/// All three assets live under one shared `certs/` directory, so the
/// listener is the end-to-end exercise of the shared-subdirectory
/// materialization (three writes into `certs/`). TLS resolution is
/// embedded-only — the source files never exist at run time.
const TLS_LISTENER_DOC: &str = "\
routes:
  - id: tls-listener
    from: 'https://127.0.0.1:18443/hook?tlsCert=certs/svc.crt&tlsKey=certs/svc.key'
    steps:
      - to: 'log:got-request'
  - id: tls-producer
    from: timer:b?period=60000
    steps:
      - to: 'https://127.0.0.1:19444/b?tlsCert=certs/svc.crt&tlsKey=certs/alt.key'
";

/// A static-tree-only route: the embedded `www` tree is memory-served
/// through the registry, so booting needs NO writable temp at all. The
/// `static_dir` key is carried inside the step's free-form `parameters`
/// map: the compile-time collection walk gathers it at any mapping
/// level, while the runtime route parser accepts the step shape (a
/// document-level `static_dir` key would be rejected as an unknown
/// route field).
const STATIC_ROUTE_DOC: &str = "\
routes:
  - id: static
    from: timer:tick?period=300
    steps:
      - to: 'log:static'
        parameters:
          static_dir: www
";

/// The disk-written-class round trip (Task 3.3): `xslt:`,
/// `validator:`, and `sql:file:` in ONE artifact. All three consumers
/// resolve their substituted confined path eagerly — the xslt endpoint
/// reads the stylesheet and the validator compiles the schema at
/// endpoint creation, the sql consumer reads the query file at
/// consumer start (before the pool init, which uses the in-memory
/// sqlite URL so no database server is needed). Boot success proves
/// all three substituted paths resolved.
const MULTI_CLASS_DOC: &str = "\
routes:
  - id: xslt-route
    from: timer:x?period=60000
    steps:
      - to: 'xslt:transform.xslt'
  - id: validator-route
    from: timer:v?period=60000
    steps:
      - to: 'validator:schema.json'
  - id: sql-route
    from: sql:file:query.sql?db_url=sqlite://:memory:&poll_delay=60000
    steps:
      - to: 'log:row'
";

/// Validator-only route for the percent-encoding round trip: the
/// `validator:` percent-decode is the ONLY consumer-side decode of a
/// substituted `uri`-site value (`camel-xslt`/`camel-sql` open the
/// substituted value literally), so the space+`%` TMPDIR case must not
/// carry classes whose readers do not decode.
const VALIDATOR_DOC: &str = "\
routes:
  - id: schema
    from: timer:v?period=60000
    steps:
      - to: 'validator:schema.json'
";

/// A COMPLETING one-shot job carrying materialized-class assets (Task
/// 3.3 job-kind round trip): the `direct:work` send holds the exchange
/// on an 8-second delay step — a completing job is otherwise
/// millisecond-fast, which would make the confined directory
/// unobservable — while the unused inline routes carry an `xslt:`
/// stylesheet AND a TLS certificate/key pair under one shared `certs/`
/// directory (two more writes into the shared subdirectory). The pair
/// rides `https://` URI parameters because the block's "document-field
/// TLS cert" is unbootable in this runtime (a route-level `tls:` block
/// is an unknown DSL field, and jobs share the route DSL) — the same
/// bootable shape as [`TLS_LISTENER_DOC`]. The job kind reaches
/// `run_embedded_job_store` through the same rewritten store at the
/// single swap point, so substitution, confinement, and cleanup must
/// behave identically to the route kind.
const JOB_ASSET_DOC: &str = "\
execute:
  mode: one-shot
  timeout: 60s
  send:
    to: 'direct:work'
routes:
  - id: work
    from: direct:work
    steps:
      - delay: 8000
      - to: 'log:done'
  - id: unused-xslt
    from: direct:unused
    steps:
      - to: 'xslt:transform.xslt'
  - id: unused-tls
    from: direct:unused-tls
    steps:
      - to: 'https://127.0.0.1:19444/b?tlsCert=certs/svc.crt&tlsKey=certs/svc.key'
";

/// The stylesheet bytes embedded by the xslt-bearing fixtures.
const XSLT_BYTES: &[u8] = b"<xsl:stylesheet version=\"1.0\"/>\n";

/// The JSON schema bytes embedded by the validator-bearing fixtures.
const SCHEMA_BYTES: &[u8] = b"{\"type\":\"object\"}\n";

/// The SQL query bytes embedded by the multi-class fixture.
const QUERY_BYTES: &[u8] = b"SELECT 1 AS one\n";

/// The TLS fixture bytes for artifacts that never boot past the
/// pre-boot failure paths (PEM-shaped; never parsed).
const PEM_BYTES: &[u8] = b"-----BEGIN CERTIFICATE-----\nfixture\n-----END CERTIFICATE-----\n";

/// A REAL throwaway certificate (EC P-256, self-signed for localhost,
/// 30 days): the https listener parses the PEMs at boot, so bogus bytes
/// would fail the listener. Generated once for this suite; carries no
/// secret value.
const CERT_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----
MIIBfjCCASOgAwIBAgIUBJbyQsqeNoChIjv2jTBjrjoj9b8wCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDkyNTEwMjI1MVoXDTI2MTAyNTEw
MjI1MVowFDESMBAGA1UEAwwJbG9jYWxob3N0MFkwEwYHKoZIzj0CAQYIKoZIzj0D
AQcDQgAErGI+cXJObGG2dYFDAst8yPMgNiViYjt5NsZZsUh6HGsCt+3uKDNnx00s
7qWT+eu9uUcUFqJ/m27sSKKQ0FIeQKNTMFEwHQYDVR0OBBYEFOYec2R7pzft1t6q
+pT5nGbOOuk5MB8GA1UdIwQYMBaAFOYec2R7pzft1t6q+pT5nGbOOuk5MA8GA1Ud
EwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDSQAwRgIhAJnwiuftApgSSZkg+S5F75F6
17EUtSHDbqZeep0O+wRRAiEApERewYxKIZSvIrZDIP7ujFFZWFSK2sleA1bbpi3t
Gv0=
-----END CERTIFICATE-----
";

/// The matching private key for [`CERT_PEM`] (same throwaway pair).
const KEY_PEM: &[u8] = b"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg+djQBzgXWDIosNc2
3ynQhOqfk2qtbso+YfsETVlCqfahRANCAASsYj5xck5sYbZ1gUMCy3zI8yA2JWJi
O3k2xlmxSHocawK37e4oM2fHTSzupZP56725RxQWon+bbuxIopDQUh5A
-----END PRIVATE KEY-----
";

/// One compiled fixture artifact.
struct Fixture {
    failing_xslt_job: PathBuf,
    tls_route: PathBuf,
    static_route: PathBuf,
    tls_listener: PathBuf,
    tls_block: PathBuf,
    multi_class: PathBuf,
    validator_only: PathBuf,
    job_asset: PathBuf,
}

/// Spawn a detached reaper that removes `dir` once this test process
/// dies (see `compiled_artifact_test.rs`).
fn spawn_reaper(dir: &Path) {
    let dir = dir.to_string_lossy().into_owned();
    let pid = std::process::id().to_string();
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "while kill -0 {pid} 2>/dev/null; do sleep 1; done; rm -rf -- '{dir}'"
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Compile every distinct document once, into the process-keyed fixture
/// directory.
fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let dir = fixture_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        spawn_reaper(&dir);

        // Failing job with an xslt asset beside the document.
        std::fs::write(dir.join("fail.job.yaml"), FAILING_XSLT_JOB_DOC).expect("write job doc");
        std::fs::write(dir.join("transform.xslt"), XSLT_BYTES).expect("write xslt asset");
        let compile = |doc: &str, artifact: &str, extra: &[&str]| -> PathBuf {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
            cmd.env_clear().current_dir(&dir);
            cmd.arg("compile").arg(doc).arg("-o").arg(artifact);
            cmd.args(extra);
            let out = cmd.output().expect("spawn `camel compile`");
            assert_eq!(
                out.status.code(),
                Some(0),
                "fixture {doc} must compile: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            dir.join(artifact)
        };
        let failing_xslt_job = compile("fail.job.yaml", "fail.bin", &[]);

        // TLS route with its five PEM fixtures under certs/. The key
        // fields are the secret family, so the compile needs the
        // `--embed-secrets` opt-in. This artifact serves the pre-boot
        // failure paths only (see the module docs).
        std::fs::create_dir_all(dir.join("certs")).expect("mkdir certs");
        std::fs::write(dir.join("app.yaml"), TLS_ROUTE_DOC).expect("write tls doc");
        for name in ["tls.crt", "tls.key", "svc.crt", "svc.key", "ca.pem"] {
            std::fs::write(dir.join("certs").join(name), PEM_BYTES).expect("write pem fixture");
        }
        let tls_route = compile("app.yaml", "tls.bin", &["--embed-secrets"]);

        // Static-tree route with a small www tree.
        std::fs::write(dir.join("static.yaml"), STATIC_ROUTE_DOC).expect("write static doc");
        std::fs::create_dir_all(dir.join("www").join("sub")).expect("mkdir www tree");
        std::fs::write(dir.join("www").join("a.txt"), "static alpha\n").expect("write a.txt");
        std::fs::write(dir.join("www").join("sub").join("b.txt"), "static beta\n")
            .expect("write b.txt");
        let static_route = compile("static.yaml", "static.bin", &[]);

        // Bootable TLS listener: real cert/key, plus a second key path
        // with byte-identical content (alt.key == svc.key), all under one
        // shared `certs/` directory. The listener route consumes both key
        // assets and the shared cert, so three writes into `certs/` are
        // the end-to-end exercise of the shared-subdirectory fix.
        std::fs::write(dir.join("listener.yaml"), TLS_LISTENER_DOC).expect("write listener doc");
        std::fs::write(dir.join("certs").join("svc.crt"), CERT_PEM).expect("write svc.crt");
        std::fs::write(dir.join("certs").join("svc.key"), KEY_PEM).expect("write svc.key");
        std::fs::write(dir.join("certs").join("alt.key"), KEY_PEM).expect("write alt.key");
        let tls_listener = compile("listener.yaml", "listener.bin", &["--embed-secrets"]);

        // TLS-block artifact for the pre-boot failure paths: bogus PEM
        // bytes suffice (never parsed) and `--embed-secrets` covers the
        // key field. It reuses the SAME nested `app.yaml` document as
        // `tls.bin` (its `certs/` files are already written), so those
        // five nested assets exercise shared-subdirectory materialization
        // on the failure path too — the pre-fix root-level strip
        // workaround is gone.
        let tls_block = compile("app.yaml", "tlsblock.bin", &["--embed-secrets"]);

        // Multi-class round trip: xslt + validator + sql:file: with
        // root-level logical paths.
        std::fs::write(dir.join("multi.yaml"), MULTI_CLASS_DOC).expect("write multi doc");
        std::fs::write(dir.join("schema.json"), SCHEMA_BYTES).expect("write schema");
        std::fs::write(dir.join("query.sql"), QUERY_BYTES).expect("write query");
        let multi_class = compile("multi.yaml", "multi.bin", &[]);

        // Validator-only route for the percent-encoding round trip
        // (reuses transform.xslt is NOT wanted here: only the
        // percent-decoding validator consumer tolerates the encoded
        // confined path, so this document carries no other class).
        std::fs::write(dir.join("valdoc.yaml"), VALIDATOR_DOC).expect("write validator doc");
        let validator_only = compile("valdoc.yaml", "valdoc.bin", &[]);

        // Completing job carrying the same xslt asset (reuses
        // transform.xslt already written above) plus the nested shared
        // `certs/` TLS pair; the key field makes `--embed-secrets`
        // mandatory.
        std::fs::write(dir.join("asset.job.yaml"), JOB_ASSET_DOC).expect("write job doc");
        let job_asset = compile("asset.job.yaml", "jobasset.bin", &["--embed-secrets"]);

        Fixture {
            failing_xslt_job,
            tls_route,
            static_route,
            tls_listener,
            tls_block,
            multi_class,
            validator_only,
            job_asset,
        }
    })
}

/// Deploy a fixture artifact into a fresh source-free directory under
/// the canonical `app.bin` name (hardlink, copy fallback).
fn deploy_artifact(artifact: &Path) -> (tempfile::TempDir, PathBuf) {
    let deploy_dir = tempfile::Builder::new()
        .prefix("camel-asset-deploy-")
        .tempdir_in(fixture_root())
        .expect("deploy tempdir on the fixture root");
    let target = deploy_dir.path().join("app.bin");
    if std::fs::hard_link(artifact, &target).is_err() {
        std::fs::copy(artifact, &target).expect("copy artifact");
    }
    (deploy_dir, target)
}

/// Harness-child branch: decode the artifact named by [`CHILD_ENV`], run
/// the embedded document, and exit with its code. A v2 artifact MUST
/// populate the asset registry before dispatch: the child asserts
/// `is_populated()` after the run and panics otherwise, so a runtime
/// that skips registry population is observable as a failed child.
fn run_child() -> i32 {
    // The real `camel` binary installs the rustls crypto provider in
    // `main` before any TLS operation (the dep graph enables both ring
    // and aws-lc-rs, so explicit selection is required). The harness
    // child bypasses `main`, so it mirrors that install here — a
    // booting TLS listener in a child otherwise panics inside rustls.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let artifact = std::env::var(CHILD_ENV).expect("child env names the artifact");
    let argv: Vec<String> = std::env::args().skip_while(|a| a != "--").skip(1).collect();
    let bytes = std::fs::read(&artifact).expect("child reads the artifact");
    let decoded = match trailer::decode_artifact(&bytes) {
        Ok(Some(decoded)) => decoded,
        Ok(None) => {
            eprintln!("compiled artifact integrity error: no terminal marker");
            return 2;
        }
        Err(e) => {
            eprintln!("compiled artifact integrity error: {e}");
            return 2;
        }
    };
    let args = match ArtifactArgs::parse(&argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let request = match decoded {
        trailer::DecodedArtifact::V1(v1) => EmbeddedRequest::from_trailer(v1, args),
        trailer::DecodedArtifact::V2(v2) => EmbeddedRequest::from_v2(v2, args),
    };
    let request = match request {
        Ok(request) => request,
        Err(e) => {
            eprintln!("compiled artifact integrity error: {e}");
            return 2;
        }
    };
    let code = tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(async { camel_cli::compile::runtime::run_embedded_document_code(request).await });
    // The registry is the memory-served class's FS of record: a v2
    // artifact runtime populates it exactly once, before dispatch.
    assert!(
        AssetRegistry::global().is_populated(),
        "the v2 artifact runtime must populate the asset registry"
    );
    code
}

/// Run the child branch if this process is a harness child.
fn child_guard() {
    if std::env::var(CHILD_ENV).is_ok() {
        std::process::exit(run_child());
    }
}

/// Spawn the artifact runtime as a harness child with `envs` overlaid on
/// the inherited environment (TMPDIR control for the watched temp root).
fn spawn_child(test: &str, dir: &Path, artifact: &Path, envs: &[(&str, &str)]) -> KillOnDrop {
    let mut cmd = Command::new(std::env::current_exe().expect("current test exe"));
    cmd.env(CHILD_ENV, artifact)
        .envs(envs.iter().copied())
        .current_dir(dir)
        .args(["--exact", test, "--nocapture", "--"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    KillOnDrop(cmd.spawn().expect("spawn harness child"))
}

/// Wait for the child to exit at most `timeout`; returns the exit code,
/// or `-1` after a force kill at the deadline.
fn wait_exit_code(child: &mut KillOnDrop, timeout: Duration) -> i32 {
    let start = Instant::now();
    loop {
        match child.0.try_wait() {
            Ok(Some(status)) => return status.code().unwrap_or(-1),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.0.kill();
                    let _ = child.0.wait();
                    return -1;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }
}

/// Names of entries currently in `watched` (the watched temp root).
fn entries_of(watched: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(watched) {
        names.extend(
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned()),
        );
    }
    names.sort();
    names
}

/// The per-boot materialization directory inside `watched`, if one
/// currently exists.
fn per_boot_dir(watched: &Path) -> Option<PathBuf> {
    entries_of(watched)
        .into_iter()
        .find(|name| name.starts_with("camel-assets-"))
        .map(|name| watched.join(name))
}

/// Poll `watched` for the per-boot materialization directory, at most
/// `timeout`, bailing out early when the child exits (the directory is
/// removed at exit, so an exited child means the observation window is
/// closed). When `want` names files, the poll keeps going until every
/// one of them exists under the per-boot directory — the directory
/// itself appears before its files are written, so a bare directory
/// observation races the writer. Returns the directory path plus its
/// confined file map, or `None` at the deadline or child exit.
fn wait_for_per_boot_dir(
    watched: &Path,
    child: &mut KillOnDrop,
    timeout: Duration,
    want: &[&str],
) -> Option<(PathBuf, BTreeMap<String, Vec<u8>>)> {
    let start = Instant::now();
    loop {
        if let Some(dir) = per_boot_dir(watched) {
            let confined = read_confined(&dir);
            if want.iter().all(|name| confined.contains_key(*name)) {
                return Some((dir, confined));
            }
        }
        if child.0.try_wait().expect("try_wait").is_some() || start.elapsed() >= timeout {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Every regular file under `dir`, keyed by its path relative to `dir`.
fn read_confined(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).expect("read confined directory") {
            let entry = entry.expect("confined directory entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(dir)
                    .expect("confined entry under the per-boot directory")
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read(&path).expect("read confined file"));
            }
        }
    }
    out
}

/// Run the failing-xslt job artifact against a watched temp root. The
/// per-boot materialization directory (`camel-assets-…`) must appear
/// while the artifact runs and must be gone at exit — guard cleanup on
/// failure — with the job's pipeline failure as the exit cause.
///
/// Strengthened (Task 3.3 review follow-up) with a SECOND case: an
/// exit-2 boot failure AFTER materialization (the TLS-block artifact
/// reaches discovery with `unknown field: tls`, which is strictly
/// after the confined materialization in the decided prepare order).
/// The guard must remove the directory on that path too.
#[test]
fn materialize_guard_removes_directory_on_boot_failure() {
    child_guard();
    // ---- Case 1 (Task 3.2): exit-1 pipeline failure after
    // materialization.
    let (deploy, artifact) = deploy_artifact(&fixture().failing_xslt_job);
    let watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");
    assert!(
        entries_of(watched.path()).is_empty(),
        "watched root starts empty"
    );

    let mut child = spawn_child(
        "materialize_guard_removes_directory_on_boot_failure",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);

    let observed = wait_for_per_boot_dir(watched.path(), &mut child, Duration::from_secs(180), &[]);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(
        code, 1,
        "the failing job pipeline exits 1; captured: {captured}"
    );
    assert!(
        observed.is_some(),
        "the per-boot materialization directory must have been observed in the watched root"
    );
    assert!(
        entries_of(watched.path()).is_empty(),
        "the guard must remove the per-boot directory at exit; left behind: {:?}",
        entries_of(watched.path())
    );

    // ---- Case 2 (Task 3.3 review): exit-2 discovery failure AFTER
    // materialization. The TLS-block artifact materializes its three
    // assets, then discovery rejects the route-level `tls:` block.
    // Whatever the observation window catches, the watched root must be
    // clean at exit: an early return between materialization and the
    // lifecycle handoff drops the guard.
    let (deploy2, artifact2) = deploy_artifact(&fixture().tls_block);
    let watched2 = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");
    let mut child2 = spawn_child(
        "materialize_guard_removes_directory_on_boot_failure",
        deploy2.path(),
        &artifact2,
        &[("TMPDIR", watched2.path().to_str().expect("watched utf-8"))],
    );
    let drained2 = spawn_drained(&mut child2);
    let observed =
        wait_for_per_boot_dir(watched2.path(), &mut child2, Duration::from_secs(180), &[]);
    let code2 = wait_exit_code(&mut child2, Duration::from_secs(180));
    let captured2 = drained2.finish();
    assert_eq!(
        code2, 2,
        "the discovery rejection exits 2; captured: {captured2}"
    );
    assert!(
        captured2.contains("unknown field: tls"),
        "the exit-2 diagnostic is the discovery rejection, strictly after the prepare phase; \
         captured: {captured2}"
    );
    assert!(
        entries_of(watched2.path()).is_empty(),
        "the guard must remove the per-boot directory on the exit-2 path too; observed dir: \
         {:?}, left behind: {:?}",
        observed,
        entries_of(watched2.path())
    );
}

/// A disk-written class (TLS) with an UNWRITABLE temp override exits 2
/// BEFORE boot with the class-and-temp diagnostic — never a route or
/// configuration boot.
#[test]
fn disk_written_class_without_writable_temp_fails_before_boot() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().tls_route);
    let watched = tempfile::tempdir().expect("watched temp root");
    // A regular FILE as TMPDIR: `tempdir_in` fails with ENOTDIR for
    // every user, including root — a deterministic unwritable temp.
    let blocker = watched.path().join("blocker");
    std::fs::write(&blocker, b"not a directory\n").expect("plant blocker file");

    let mut child = spawn_child(
        "disk_written_class_without_writable_temp_fails_before_boot",
        deploy.path(),
        &artifact,
        &[("TMPDIR", blocker.to_str().expect("blocker utf-8"))],
    );
    let drained = spawn_drained(&mut child);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(
        code, 2,
        "an unavailable writable temp exits 2 before boot; captured: {captured}"
    );
    assert!(
        captured.contains("client CA") || captured.contains("certificate"),
        "the diagnostic names the materialized asset class; captured: {captured}"
    );
    assert!(
        captured.to_lowercase().contains("temp"),
        "the diagnostic names the missing writable temp; captured: {captured}"
    );
    assert!(
        !captured.contains("context started"),
        "no route boot may happen; captured: {captured}"
    );
}

/// A static-tree-only artifact needs NO writable temp: booted against a
/// watched temp root, registry serving carries the embedded tree, no
/// temp files appear, and the source tree (never deployed) is not read.
#[test]
fn round_trip_memory_served_class_creates_no_temp_files() {
    child_guard();
    let _guard = BOOT_SERIAL.lock().expect("boot serial");
    let (deploy, artifact) = deploy_artifact(&fixture().static_route);
    // Source-free deploy: only the artifact is present — the embedded
    // static tree has no on-disk origin to read.
    assert!(!deploy.path().join("www").exists(), "no source tree");
    assert!(!deploy.path().join("static.yaml").exists(), "no source doc");
    let watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");

    let mut child = spawn_child(
        "round_trip_memory_served_class_creates_no_temp_files",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(
            &mut child,
            &drained.markers(),
            "context started",
            Duration::from_secs(180)
        ),
        "the static-only artifact boots from the embedded store alone: {}",
        drained.captured()
    );
    common::send_term(&child.0);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(
        code, 0,
        "SIGTERM shuts down gracefully; captured: {captured}"
    );
    assert!(
        entries_of(watched.path()).is_empty(),
        "a static-only artifact must create no temp files; found: {:?}",
        entries_of(watched.path())
    );
}

/// TLS materialization round trip with a booting listener (Task 3.3):
/// the source tree is never deployed, so the ONLY way the rustls
/// listener can parse `certs/svc.crt`/`certs/svc.key` is through the
/// confined materialization — embedded-only resolution, no host fallback
/// (bd rc-p823t). Boot success is the resolution proof; the watcher root
/// must host exactly one per-boot directory and be clean at exit.
///
/// Task 3.3 deviation: the block's cert/key/CA trio has no bootable CA
/// consumer (a route-level `tls:` block is an unknown DSL field, and
/// `https://` URI parameters carry cert/key only), so the listener
/// fixture carries cert+key.
#[test]
fn round_trip_tls_resolution_never_probes_host_fs() {
    child_guard();
    let _guard = BOOT_SERIAL.lock().expect("boot serial");
    let (deploy, artifact) = deploy_artifact(&fixture().tls_listener);
    // Source-free deploy: the originally declared host paths
    // (`certs/svc.crt`, `certs/svc.key`, `certs/alt.key`) do not exist
    // beside the artifact. A host-filesystem fallback would fail the
    // listener's PEM parse.
    assert!(!deploy.path().join("certs").exists(), "no host certs dir");
    assert!(
        !deploy.path().join("certs").join("svc.crt").exists(),
        "no host cert"
    );
    assert!(
        !deploy.path().join("certs").join("svc.key").exists(),
        "no host key"
    );
    assert!(
        !deploy.path().join("listener.yaml").exists(),
        "no source doc"
    );
    let watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");

    let mut child = spawn_child(
        "round_trip_tls_resolution_never_probes_host_fs",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(
            &mut child,
            &drained.markers(),
            "context started",
            Duration::from_secs(180)
        ),
        "the TLS listener boots from the confined materialization alone: {}",
        drained.captured()
    );
    let per_boot = per_boot_dir(watched.path()).expect("per-boot directory after boot marker");
    let confined = read_confined(&per_boot);
    assert_eq!(
        confined.get("certs/svc.crt").map(Vec::as_slice),
        Some(CERT_PEM),
        "the nested shared-directory cert is materialized byte-identical"
    );
    assert_eq!(
        confined.get("certs/svc.key").map(Vec::as_slice),
        Some(KEY_PEM),
        "the nested shared-directory key is materialized byte-identical"
    );
    assert_eq!(
        confined.get("certs/alt.key").map(Vec::as_slice),
        Some(KEY_PEM),
        "the second nested key path is materialized byte-identical"
    );
    common::send_term(&child.0);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(
        code, 0,
        "SIGTERM shuts down gracefully; captured: {captured}"
    );
    assert!(
        entries_of(watched.path()).is_empty(),
        "shutdown removes the per-boot directory; left behind: {:?}",
        entries_of(watched.path())
    );
}

/// TLS materialization confinement and cleanup (Task 3.3): the
/// listener boots against materialized files whose BLAKE3 digests match
/// the fixture bytes; every write stays inside the confined directory
/// (0700 dir, 0600 files); shutdown removes it.
#[test]
fn round_trip_tls_materializes_confined_and_boots() {
    child_guard();
    // Bootable shape (Task 3.3 deviation, Mission 243 review): a
    // route-level `tls:` block is an unknown DSL field at discovery, so
    // the listener does NOT boot from a document-field `tls:` block —
    // it boots from `https://` URI parameters (`tlsCert`/`tlsKey`) whose
    // substituted confined paths rustls parses at listener creation.
    // Boot success is the proof that the nested `certs/` files were
    // materialized and substituted.
    let _guard = BOOT_SERIAL.lock().expect("boot serial");
    let (deploy, artifact) = deploy_artifact(&fixture().tls_listener);
    let watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");

    let mut child = spawn_child(
        "round_trip_tls_materializes_confined_and_boots",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(
            &mut child,
            &drained.markers(),
            "context started",
            Duration::from_secs(180)
        ),
        "the listener spawns against the materialized files: {}",
        drained.captured()
    );

    // Exactly one per-boot directory, hosting exactly the three
    // substitution-targeted assets.
    let per_boot = per_boot_dir(watched.path()).expect("per-boot directory after boot marker");
    assert_eq!(
        entries_of(watched.path()),
        vec![
            per_boot
                .file_name()
                .expect("per-boot dir name")
                .to_string_lossy()
                .into_owned()
        ],
        "the watched root hosts exactly the per-boot directory"
    );

    // Digests: blake3(confined) == blake3(fixture bytes).
    let digest = |bytes: &[u8]| blake3::hash(bytes).to_hex().to_string();
    let confined = read_confined(&per_boot);
    let expected: BTreeMap<&str, &[u8]> = BTreeMap::from([
        ("certs/svc.crt", CERT_PEM),
        ("certs/svc.key", KEY_PEM),
        ("certs/alt.key", KEY_PEM),
    ]);
    assert_eq!(
        confined.len(),
        expected.len(),
        "exactly the targeted assets"
    );
    for (name, bytes) in &expected {
        let written = confined
            .get(*name)
            .unwrap_or_else(|| panic!("materialized file {name} exists"));
        assert_eq!(
            digest(written),
            digest(bytes),
            "materialized {name} matches the fixture digest"
        );
    }

    // Modes: per-boot directory 0700, files 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = std::fs::metadata(&per_boot)
            .expect("per-boot dir metadata")
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700, "per-boot directory is 0700");
        for name in expected.keys() {
            let file_mode = std::fs::metadata(per_boot.join(name))
                .unwrap_or_else(|e| panic!("materialized {name} metadata: {e}"))
                .permissions()
                .mode();
            assert_eq!(file_mode & 0o777, 0o600, "materialized {name} is 0600");
        }
    }

    common::send_term(&child.0);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(
        code, 0,
        "SIGTERM shuts down gracefully; captured: {captured}"
    );
    assert!(
        entries_of(watched.path()).is_empty(),
        "shutdown removes the per-boot directory; left behind: {:?}",
        entries_of(watched.path())
    );
}

/// TLS URI parameters materialize and substitute (Task 3.3): the
/// `tlsCert=certs/svc.crt`/`tlsKey=certs/svc.key` parameters resolve
/// through the substitution table to confined paths with byte-identical
/// content — two files under ONE shared `certs/` directory, the exact
/// shape the pre-fix missing-parent walk rejected with `File exists`.
/// With the source tree absent, boot success requires the substitution —
/// the listener's PEM parse opens the substituted confined paths.
#[test]
fn round_trip_tls_uri_params_materialize_and_substitute() {
    child_guard();
    let _guard = BOOT_SERIAL.lock().expect("boot serial");
    let (deploy, artifact) = deploy_artifact(&fixture().tls_listener);
    let watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");

    let mut child = spawn_child(
        "round_trip_tls_uri_params_materialize_and_substitute",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(
            &mut child,
            &drained.markers(),
            "context started",
            Duration::from_secs(180)
        ),
        "the URI parameters resolve to the confined materialization: {}",
        drained.captured()
    );
    let per_boot = per_boot_dir(watched.path()).expect("per-boot directory after boot marker");
    let confined = read_confined(&per_boot);
    for (name, bytes) in [
        ("certs/svc.crt", CERT_PEM),
        ("certs/svc.key", KEY_PEM),
        ("certs/alt.key", KEY_PEM),
    ] {
        assert_eq!(
            confined.get(name).map(Vec::as_slice),
            Some(bytes),
            "the substituted {name} parameter resolves to byte-identical materialized content"
        );
    }
    common::send_term(&child.0);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(
        code, 0,
        "SIGTERM shuts down gracefully; captured: {captured}"
    );
    assert!(entries_of(watched.path()).is_empty(), "shutdown cleanup");
}

/// `xslt:`, `validator:`, and `sql:file:` substitution round trip
/// (Task 3.3): all three consumers resolve their substituted confined
/// path eagerly (stylesheet read and schema compile at endpoint
/// creation, query-file read at consumer start — the latter before the
/// in-memory sqlite pool init, so no database server is needed), so
/// boot success proves resolution, and the confined files hold the
/// embedded bytes.
#[test]
fn round_trip_xslt_validator_sql_materialize_and_substitute() {
    child_guard();
    let _guard = BOOT_SERIAL.lock().expect("boot serial");
    let (deploy, artifact) = deploy_artifact(&fixture().multi_class);
    assert!(
        !deploy.path().join("transform.xslt").exists(),
        "no host xslt"
    );
    assert!(
        !deploy.path().join("schema.json").exists(),
        "no host schema"
    );
    assert!(!deploy.path().join("query.sql").exists(), "no host query");
    let watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");

    let mut child = spawn_child(
        "round_trip_xslt_validator_sql_materialize_and_substitute",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(
            &mut child,
            &drained.markers(),
            "context started",
            Duration::from_secs(180)
        ),
        "all three substituted classes resolve at boot: {}",
        drained.captured()
    );
    let per_boot = per_boot_dir(watched.path()).expect("per-boot directory after boot marker");
    let confined = read_confined(&per_boot);
    assert_eq!(
        confined.get("transform.xslt").map(Vec::as_slice),
        Some(XSLT_BYTES),
        "the stylesheet materializes byte-identical"
    );
    assert_eq!(
        confined.get("schema.json").map(Vec::as_slice),
        Some(SCHEMA_BYTES),
        "the schema materializes byte-identical"
    );
    assert_eq!(
        confined.get("query.sql").map(Vec::as_slice),
        Some(QUERY_BYTES),
        "the sql query file materializes byte-identical"
    );
    common::send_term(&child.0);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(
        code, 0,
        "SIGTERM shuts down gracefully; captured: {captured}"
    );
    assert!(
        entries_of(watched.path()).is_empty(),
        "shutdown removes the per-boot directory; left behind: {:?}",
        entries_of(watched.path())
    );
}

/// Job-kind round trip with materialized-class assets (Task 3.3): the
/// job artifact reaches `run_embedded_job_store` through the same
/// rewritten store at the single swap point, so confinement,
/// substitution to confined paths, byte-identical materialized content,
/// and shutdown cleanup are identical to the route kind. The job
/// completes (exit 0).
///
/// Task 3.3 deviation: the block's "document-field TLS cert" job asset
/// is unbootable in this runtime (a `tls:` block is an unknown DSL
/// field, and jobs share the route DSL), so the job carries an `xslt:`
/// stylesheet plus a bootable `https://` URI-parameter certificate/key
/// pair under one shared `certs/` directory — the same nested shape and
/// the same materialized-class mechanics as the route kind.
#[test]
fn round_trip_job_artifact_materializes_and_substitutes_identically() {
    child_guard();
    let _guard = BOOT_SERIAL.lock().expect("boot serial");
    let (deploy, artifact) = deploy_artifact(&fixture().job_asset);
    assert!(
        !deploy.path().join("transform.xslt").exists(),
        "no host xslt"
    );
    assert!(!deploy.path().join("certs").exists(), "no host certs");
    let watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");

    let mut child = spawn_child(
        "round_trip_job_artifact_materializes_and_substitutes_identically",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);

    // The job exits quickly after boot: catch the per-boot directory
    // while it runs and read the materialized bytes mid-flight. The
    // poll waits for the files themselves — the bare directory appears
    // before its files are written.
    let observed = wait_for_per_boot_dir(
        watched.path(),
        &mut child,
        Duration::from_secs(180),
        &["transform.xslt", "certs/svc.crt", "certs/svc.key"],
    );
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(code, 0, "the completing job exits 0; captured: {captured}");
    let (per_boot, confined) = observed.unwrap_or_else(|| {
        panic!(
            "the per-boot directory was observable during the job run; captured: {}",
            captured
        )
    });
    assert_eq!(
        confined.get("transform.xslt").map(Vec::as_slice),
        Some(XSLT_BYTES),
        "the substituted stylesheet resolves inside the confined directory with byte-identical \
         content (observed at {})",
        per_boot.display()
    );
    assert_eq!(
        confined.get("certs/svc.crt").map(Vec::as_slice),
        Some(CERT_PEM),
        "the nested shared-directory certificate resolves inside the job kind's confined \
         directory with byte-identical content (observed at {})",
        per_boot.display()
    );
    assert_eq!(
        confined.get("certs/svc.key").map(Vec::as_slice),
        Some(KEY_PEM),
        "the nested shared-directory key resolves inside the job kind's confined directory with \
         byte-identical content (observed at {})",
        per_boot.display()
    );
    assert!(
        entries_of(watched.path()).is_empty(),
        "shutdown cleanup is identical to the route kind; left behind: {:?}",
        entries_of(watched.path())
    );
}

/// Two DIFFERENT secret logical paths with IDENTICAL bytes (Task 3.3):
/// registry population pairs manifest entries to store entries BY
/// POSITION, so a duplicate digest is legitimate (only a duplicate
/// logical path is one); both entries populate, both substitution sites
/// resolve independently, and boot completes.
#[test]
fn round_trip_identical_byte_secrets_both_populate() {
    child_guard();
    let _guard = BOOT_SERIAL.lock().expect("boot serial");
    let (deploy, artifact) = deploy_artifact(&fixture().tls_listener);
    let watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");

    let mut child = spawn_child(
        "round_trip_identical_byte_secrets_both_populate",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(
            &mut child,
            &drained.markers(),
            "context started",
            Duration::from_secs(180)
        ),
        "registry population succeeded for both identical-byte secret entries: {}",
        drained.captured()
    );
    let per_boot = per_boot_dir(watched.path()).expect("per-boot directory after boot marker");
    let confined = read_confined(&per_boot);
    assert_eq!(
        confined.get("certs/svc.key").map(Vec::as_slice),
        Some(KEY_PEM),
        "the first nested secret path resolves independently"
    );
    assert_eq!(
        confined.get("certs/alt.key").map(Vec::as_slice),
        Some(KEY_PEM),
        "the second nested secret path (identical bytes, distinct logical path) resolves \
         independently"
    );
    common::send_term(&child.0);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(code, 0, "boot completes; captured: {captured}");
    assert!(entries_of(watched.path()).is_empty(), "shutdown cleanup");
}

/// Mutated asset bytes fail boot with exit 2 BEFORE any route or
/// configuration boot (Task 3.3): the asset bytes are mutated after
/// compile AND the v2 trailer checksum is recomputed over the mutated
/// content, so framing verification passes and the manifest/registry
/// digest check is what fails.
#[test]
fn round_trip_digest_mismatch_fails_boot() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().static_route);
    // Byte surgery: locate the a.txt asset bytes in the store content
    // section, mutate one byte (length-preserving, so kind/length
    // agreement still passes), and recompute the footer checksum with
    // the v2 domain (trailer.rs `checksum_domain_v2`: separator, 0x00,
    // le version, kind disc, le content/index/manifest lengths, then
    // the three sections).
    let mut bytes = std::fs::read(&artifact).expect("read the fixture artifact");
    let footer_len = 76usize;
    let footer_start = bytes.len() - footer_len;
    let footer = bytes[footer_start..].to_vec();
    assert_eq!(&footer[0..8], b"CAMELTR1", "v2 footer magic");
    assert_eq!(&footer[68..76], b"CAMELTR1", "v2 terminal magic");
    let le = |slice: &[u8]| -> u64 { u64::from_le_bytes(slice.try_into().expect("8-byte slice")) };
    let content_len = le(&footer[12..20]) as usize;
    let index_len = le(&footer[20..28]) as usize;
    let manifest_len = le(&footer[28..36]) as usize;
    let content_start = footer_start - content_len - index_len - manifest_len;
    let content_end = content_start + content_len;
    let needle = b"static alpha\n";
    let position = bytes[content_start..content_end]
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("the asset bytes appear exactly in the store content");
    bytes[content_start + position + needle.len() - 1] = b'X';

    let mut domain = Vec::new();
    domain.extend_from_slice(b"rust-camel-trailer-v2");
    domain.push(0x00);
    domain.extend_from_slice(&footer[8..10]); // le version
    domain.push(footer[10]); // kind disc (the flags byte is excluded)
    domain.extend_from_slice(&footer[12..36]); // le content/index/manifest lengths
    domain.extend_from_slice(&bytes[content_start..content_end]); // mutated content
    domain.extend_from_slice(&bytes[content_end..footer_start]); // index + manifest
    let checksum = blake3::hash(&domain);
    bytes[footer_start + 36..footer_start + 68].copy_from_slice(checksum.as_bytes());

    // A separate FILE (not the hardlink): mutating the hardlink would
    // corrupt the shared fixture inode.
    let mutated = deploy.path().join("mutated.bin");
    std::fs::write(&mutated, &bytes).expect("write the resealed artifact");

    let watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");
    let mut child = spawn_child(
        "round_trip_digest_mismatch_fails_boot",
        deploy.path(),
        &mutated,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(
        code, 2,
        "a digest mismatch exits 2 before any boot; captured: {captured}"
    );
    assert!(
        captured.contains("compiled artifact integrity error"),
        "the diagnostic is the decode-time integrity error; captured: {captured}"
    );
    assert!(
        captured.contains("does not match the store content"),
        "the diagnostic is the digest mismatch, not the trailer checksum; captured: {captured}"
    );
    assert!(
        !captured.contains("trailer checksum"),
        "the trailer checksum must pass (it was recomputed); captured: {captured}"
    );
    assert!(
        !captured.contains("context started"),
        "no configuration or route boot may happen; captured: {captured}"
    );
    assert!(
        entries_of(watched.path()).is_empty(),
        "nothing materialized"
    );
}

/// A post-compile decoy placed at an originally referenced asset path
/// is never read (Task 3.3): the substitution table is the only
/// resolution path, so only embedded bytes are served and the decoy
/// stays untouched.
#[test]
fn round_trip_ignores_post_compile_asset_decoy() {
    child_guard();
    let _guard = BOOT_SERIAL.lock().expect("boot serial");
    let (deploy, artifact) = deploy_artifact(&fixture().tls_listener);
    // The decoy: garbage at the originally declared nested cert path. If
    // the runtime probed the host filesystem, the listener's PEM parse
    // would read it and boot would fail.
    let decoy = deploy.path().join("certs").join("svc.crt");
    std::fs::create_dir_all(deploy.path().join("certs")).expect("plant decoy dir");
    std::fs::write(&decoy, b"decoy bytes, not a certificate\n").expect("plant decoy");

    let watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");
    let mut child = spawn_child(
        "round_trip_ignores_post_compile_asset_decoy",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(
            &mut child,
            &drained.markers(),
            "context started",
            Duration::from_secs(180)
        ),
        "the listener boots past the decoy: {}",
        drained.captured()
    );
    let per_boot = per_boot_dir(watched.path()).expect("per-boot directory after boot marker");
    let confined = read_confined(&per_boot);
    assert_eq!(
        confined.get("certs/svc.crt").map(Vec::as_slice),
        Some(CERT_PEM),
        "only embedded bytes are served — the confined cert is the fixture, not the decoy"
    );
    assert_eq!(
        std::fs::read(&decoy).expect("decoy unchanged"),
        b"decoy bytes, not a certificate\n",
        "the decoy file was never written through"
    );
    common::send_term(&child.0);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(
        code, 0,
        "SIGTERM shuts down gracefully; captured: {captured}"
    );
    assert!(entries_of(watched.path()).is_empty(), "shutdown cleanup");
}

/// The read-only contract, per class (Task 3.3): a static-only
/// artifact needs NO writable temp (it boots — and writes nothing —
/// with a TMPDIR that cannot hold a directory), while a TLS-bearing
/// artifact boots against a writable TMPDIR with all materialization
/// confined inside it and nothing written to the (source-free) root.
#[test]
fn round_trip_read_only_contract_by_class() {
    child_guard();
    let _guard = BOOT_SERIAL.lock().expect("boot serial");
    // ---- Static-only side: an unusable TMPDIR (a regular FILE —
    // ENOTDIR for every user, including root) does not stop the boot.
    let (static_deploy, static_artifact) = deploy_artifact(&fixture().static_route);
    let static_watched = tempfile::tempdir().expect("watched temp root");
    let blocker = static_watched.path().join("blocker");
    std::fs::write(&blocker, b"not a directory\n").expect("plant blocker file");
    let mut static_child = spawn_child(
        "round_trip_read_only_contract_by_class",
        static_deploy.path(),
        &static_artifact,
        &[("TMPDIR", blocker.to_str().expect("blocker utf-8"))],
    );
    let static_drained = spawn_drained(&mut static_child);
    assert!(
        wait_for_marker(
            &mut static_child,
            &static_drained.markers(),
            "context started",
            Duration::from_secs(180)
        ),
        "the static-only artifact boots with no writable temp: {}",
        static_drained.captured()
    );
    common::send_term(&static_child.0);
    let static_code = wait_exit_code(&mut static_child, Duration::from_secs(180));
    let static_captured = static_drained.finish();
    assert_eq!(
        static_code, 0,
        "SIGTERM shuts the static-only artifact down gracefully; captured: {static_captured}"
    );
    assert_eq!(
        entries_of(static_watched.path()),
        vec!["blocker".to_string()],
        "the static-only artifact writes nothing (the watched root holds only the planted \
         blocker): {:?}",
        entries_of(static_watched.path())
    );

    // ---- TLS-bearing side: writable TMPDIR, read-only-contract root.
    // The deploy directory holds ONLY the artifact, and it must still
    // hold only the artifact after the run: materialization stays
    // inside TMPDIR.
    let (tls_deploy, tls_artifact) = deploy_artifact(&fixture().tls_listener);
    let tls_watched = tempfile::Builder::new()
        .prefix("camel-asset-watched-")
        .tempdir()
        .expect("watched temp root");
    let mut tls_child = spawn_child(
        "round_trip_read_only_contract_by_class",
        tls_deploy.path(),
        &tls_artifact,
        &[(
            "TMPDIR",
            tls_watched.path().to_str().expect("watched utf-8"),
        )],
    );
    let tls_drained = spawn_drained(&mut tls_child);
    assert!(
        wait_for_marker(
            &mut tls_child,
            &tls_drained.markers(),
            "context started",
            Duration::from_secs(180)
        ),
        "the TLS artifact boots with materialization confined inside TMPDIR: {}",
        tls_drained.captured()
    );
    let per_boot =
        per_boot_dir(tls_watched.path()).expect("per-boot directory confined inside TMPDIR");
    assert!(
        !read_confined(&per_boot).is_empty(),
        "the materialization lives inside the watched TMPDIR"
    );
    assert_eq!(
        entries_of(tls_deploy.path()),
        vec!["app.bin".to_string()],
        "nothing was written outside TMPDIR"
    );
    common::send_term(&tls_child.0);
    let tls_code = wait_exit_code(&mut tls_child, Duration::from_secs(180));
    let tls_captured = tls_drained.finish();
    assert_eq!(
        tls_code, 0,
        "SIGTERM shuts the TLS artifact down gracefully; captured: {tls_captured}"
    );
    assert!(
        entries_of(tls_watched.path()).is_empty(),
        "shutdown cleanup"
    );
}

/// Percent-encoded substituted `uri`-site values round-trip through the
/// `validator:` percent-decode (Task 3.3): the per-boot path contains a
/// space AND a literal `%`, so the substituted URI value is
/// percent-encoded; the validator's decode recovers the exact confined
/// path and the schema loads byte-identical (boot success is the decode
/// proof — `CompiledValidator::compile` opens the decoded path at
/// endpoint creation).
#[test]
fn substitution_percent_encoding_round_trips_validator_decode() {
    child_guard();
    let _guard = BOOT_SERIAL.lock().expect("boot serial");
    let (deploy, artifact) = deploy_artifact(&fixture().validator_only);
    assert!(
        !deploy.path().join("schema.json").exists(),
        "no host schema"
    );
    // The watched root name carries a space and a literal %: the
    // per-boot path inherits both.
    let watched = tempfile::Builder::new()
        .prefix("watched dir%")
        .tempdir()
        .expect("watched temp root");
    assert!(
        watched
            .path()
            .to_str()
            .expect("watched utf-8")
            .contains(' '),
        "the watched root path contains a space"
    );
    assert!(
        watched
            .path()
            .to_str()
            .expect("watched utf-8")
            .contains('%'),
        "the watched root path contains a literal percent"
    );

    let mut child = spawn_child(
        "substitution_percent_encoding_round_trips_validator_decode",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched.path().to_str().expect("watched utf-8"))],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(
            &mut child,
            &drained.markers(),
            "context started",
            Duration::from_secs(180)
        ),
        "the validator percent-decoded the substituted value to the exact confined path: {}",
        drained.captured()
    );
    let per_boot = per_boot_dir(watched.path()).expect("per-boot directory after boot marker");
    assert_eq!(
        read_confined(&per_boot)
            .get("schema.json")
            .map(Vec::as_slice),
        Some(SCHEMA_BYTES),
        "the schema loads from the confined path with byte-identical content"
    );
    common::send_term(&child.0);
    let code = wait_exit_code(&mut child, Duration::from_secs(180));
    let captured = drained.finish();
    assert_eq!(
        code, 0,
        "SIGTERM shuts down gracefully; captured: {captured}"
    );
    assert!(entries_of(watched.path()).is_empty(), "shutdown cleanup");
}

/// The substitution path-safety rule fails closed BEFORE boot (Task
/// 3.3), naming the violated clause, and leaves no materialized files:
/// - a TMPDIR containing a newline (an ASCII control character) is
///   rejected at EVERY site (all-sites clause);
/// - a TMPDIR containing a space pushes a `literal`-site confined path
///   outside `[A-Za-z0-9._/+-]` (literal clause).
///
/// The TLS-block artifact provides the literal sites. Both clauses fire
/// during the pre-boot prepare phase — strictly before discovery, so
/// this artifact's non-bootable `tls:` block never matters here.
#[test]
fn unencodable_materialization_path_fails_before_boot() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().tls_block);

    // ---- Case A: newline in TMPDIR (all-sites control clause). Some
    // filesystems reject control characters in names (e.g. NTFS); fall
    // back to /tmp, which every supported Linux deployment provides as
    // a control-character-tolerant filesystem.
    let watched_a = tempfile::tempdir().expect("watched temp root");
    let newline_dir = watched_a.path().join("nl\ndir");
    let newline_dir = if std::fs::create_dir(&newline_dir).is_ok() {
        newline_dir
    } else {
        let fallback =
            Path::new("/tmp").join(format!("camel-roundtrip-nl\n{}", std::process::id()));
        std::fs::create_dir(&fallback).expect("newline directory on a control-char-tolerant fs");
        fallback
    };
    let mut child_a = spawn_child(
        "unencodable_materialization_path_fails_before_boot",
        deploy.path(),
        &artifact,
        &[("TMPDIR", newline_dir.to_str().expect("newline dir utf-8"))],
    );
    let drained_a = spawn_drained(&mut child_a);
    let code_a = wait_exit_code(&mut child_a, Duration::from_secs(180));
    let captured_a = drained_a.finish();
    assert_eq!(
        code_a, 2,
        "a control character in the confined path exits 2 before boot; captured: {captured_a}"
    );
    assert!(
        captured_a.contains("substitution path-safety rule violation"),
        "the diagnostic names the rule; captured: {captured_a}"
    );
    assert!(
        captured_a.contains("ASCII control character") && captured_a.contains("every site"),
        "the diagnostic names the all-sites clause; captured: {captured_a}"
    );
    assert!(
        !captured_a.contains("context started"),
        "no route or configuration boot may happen; captured: {captured_a}"
    );
    assert!(
        per_boot_dir(&newline_dir).is_none(),
        "no materialized files are left behind in the newline TMPDIR"
    );
    let _ = std::fs::remove_dir(&newline_dir);

    // ---- Case B: space in TMPDIR (literal clause).
    let watched_b = tempfile::Builder::new()
        .prefix("spaced dir ")
        .tempdir()
        .expect("watched temp root");
    let mut child_b = spawn_child(
        "unencodable_materialization_path_fails_before_boot",
        deploy.path(),
        &artifact,
        &[("TMPDIR", watched_b.path().to_str().expect("watched utf-8"))],
    );
    let drained_b = spawn_drained(&mut child_b);
    let code_b = wait_exit_code(&mut child_b, Duration::from_secs(180));
    let captured_b = drained_b.finish();
    assert_eq!(
        code_b, 2,
        "a literal-site space in the confined path exits 2 before boot; captured: {captured_b}"
    );
    assert!(
        captured_b.contains("substitution path-safety rule violation"),
        "the diagnostic names the rule; captured: {captured_b}"
    );
    assert!(
        captured_b.contains("literal-site safe set"),
        "the diagnostic names the literal clause; captured: {captured_b}"
    );
    assert!(
        !captured_b.contains("context started"),
        "no route or configuration boot may happen; captured: {captured_b}"
    );
    assert!(
        entries_of(watched_b.path()).is_empty(),
        "no materialized files are left behind; found: {:?}",
        entries_of(watched_b.path())
    );
}
