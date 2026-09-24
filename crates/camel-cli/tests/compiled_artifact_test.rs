//! Integration tests for compiled-artifact runtime (openspec change
//! `cli-compile`, Tasks 2.2 and 2.3). The suite compiles each distinct
//! document ONCE with the real `camel compile` into a shared immutable
//! fixture — the compile step copies the full ~287 MB `camel` binary into
//! every artifact, so per-test compiles would write gigabytes under
//! parallel execution and exhaust the disk (ENOSPC). The fixture and the
//! per-test deploy directories live on a space-probed [`fixture_root`]:
//! the OS temp directory when it holds the ~3.5 GiB the suite writes,
//! otherwise the workspace target directory (bd rc-fdkta). Every test
//! then deploys the fixture artifact into its own source-free directory
//! and runs it through `run_embedded_document` — the same entry the
//! binary self-detect path (Task 2.3) calls.
//!
//! The artifact runtime executes in a harness CHILD of this test binary:
//! the parent re-spawns `current_exe()` with `--exact <test>` and the
//! artifact argv after `--`, plus [`CHILD_ENV`] naming the artifact. The
//! child branch decodes the trailer, parses `ArtifactArgs`, runs the
//! embedded document, and exits with its code — exercising the library
//! seam end to end (boot, signals, report) without a self-detecting main.
//!
//! The Task 2.3 tests below exercise the REAL self-detect entry instead:
//! they spawn the artifact binary itself (a trailer-bearing copy of
//! `camel`), whose `main` probes the trailer before Clap and consumes
//! the artifact argv surface on its own.

mod common;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use camel_cli::compile::runtime::{ArtifactArgs, EmbeddedRequest};
use camel_cli::compile::trailer;

use common::{KillOnDrop, drain_to_buffer, send_signal};

/// Env var that marks a harness child: its value is the artifact path.
const CHILD_ENV: &str = "CAMEL_COMPILED_ARTIFACT_CHILD";

/// A minimal timer→log route document (long-running; signal shutdown).
/// `period` is plain milliseconds (the timer component parses a number).
const ROUTE_DOC: &str = "\
routes:
  - id: demo
    from: timer:tick?period=300
    steps:
      - to: log:demo
";

/// A one-shot job document with inline routes (self-contained).
const JOB_DOC: &str = "\
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: ping
routes:
  - id: job-transform
    from: direct:transform
    steps:
      - set_body:
          value: job-done
";

/// A one-shot job document whose route pipeline fails (send to a
/// `direct:` endpoint with no consumer).
const FAILING_JOB_DOC: &str = "\
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
";

/// A one-shot job document whose route resolves `${env:NAME}` from the
/// deployment environment at run time. The fixture compiles it WITH a
/// compile-time value present: that value must never enter the artifact.
const ENV_DOC: &str = "\
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: ping
routes:
  - id: job-transform
    from: direct:transform
    steps:
      - set_body:
          value: ${env:DEPLOY_GREETING}
";
/// The configuration document of the multi-route fixture: one include
/// fragment plus the `routes` pattern that pulls the second embedded
/// route file into the source plan.
const MULTI_CONFIG: &str = "\
include = [\"conf/base.toml\"]
routes = [\"routes/*.yaml\"]
[default]
log_level = \"info\"
";

/// The configuration document of the multi-document job fixtures: the
/// include fragment without a `routes` pattern — the job document's
/// `routeFiles` names the indexed route source explicitly, and a
/// pattern here would duplicate it.
const MULTI_JOB_CONFIG: &str = "\
include = [\"conf/base.toml\"]
[default]
log_level = \"info\"
";

/// The include fragment of the multi-document fixtures.
const MULTI_INCLUDE: &str = "[default]\ndrain_timeout_ms = 5000\n";

/// The entry route document of the multi-route fixture: the `alpha`
/// route, observable through its logged body marker.
const MULTI_ENTRY_ROUTE: &str = "\
routes:
  - id: alpha
    from: timer:tick?period=200
    steps:
      - set_body:
          value: alpha-marker
      - to: log:alpha
";

/// The indexed route file of the multi-route fixture: the `beta` route.
const MULTI_INDEXED_ROUTE: &str = "\
routes:
  - id: beta
    from: timer:tick?period=200
    steps:
      - set_body:
          value: beta-marker
      - to: log:beta
";

/// The multi-document job document: one-shot send against a route that
/// lives in an indexed route file (resolved through `--config`).
const MULTI_JOB_DOC: &str = "\
routeFiles:
  - routes/transform.yaml
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: ping
";

/// The environment fixture's job document: same send shape, but its
/// indexed route file is the `${env:}`-resolving one.
const MULTI_ENV_JOB_DOC: &str = "\
routeFiles:
  - routes/greet.yaml
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: ping
";

/// The indexed route file of the multi-document job fixture.
const MULTI_JOB_ROUTE: &str = "\
routes:
  - id: job-transform
    from: direct:transform
    steps:
      - set_body:
          value: multi-job-done
";

/// The indexed route file of the multi-document environment fixture:
/// the `${env:}` expression stays raw in the artifact and resolves from
/// the deployment environment.
const MULTI_ENV_ROUTE: &str = "\
routes:
  - id: greet-transform
    from: direct:transform
    steps:
      - set_body:
          value: ${env:DEPLOY_GREETING}
";

/// A one-shot job document with a DECLARED argument whose default
/// (`hello`) the artifact must apply at startup (jobargs Task 3.2): the
/// send body carries `${arg:value}` and the step-free route echoes the
/// body back as the reply, so the reply value proves the resolution.
const ARG_DOC: &str = "\
args:
  value:
    default: hello
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: \"${arg:value}\"
routes:
  - id: job-arg
    from: direct:transform
";

/// A one-shot job document declaring a REQUIRED argument without a
/// default: an embedded run has no CLI flags to fill it, so the
/// artifact must reject it at startup (exit 2, naming the argument).
const REQUIRED_ARG_DOC: &str = "\
args:
  value:
    required: true
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: \"${arg:value}\"
routes:
  - id: job-arg
    from: direct:transform
";

/// A one-shot job document declaring a TYPED argument whose default
/// (`"007"`) must coerce to the canonical `7` at resolution (jobtyped
/// Task 5): the send target `direct:${arg:count}` interpolates to
/// `direct:7` and the route consumer is declared ONLY at `direct:7`,
/// so an uncoerced `direct:007` target would find no consumer and fail
/// the send — exit 0 on both run paths proves the canonical form.
const TYPED_DEFAULT_ARG_DOC: &str = "\
args:
  count:
    type: int
    default: \"007\"
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: \"direct:${arg:count}\"
    body: ping
routes:
  - id: job-count
    from: direct:7
";

/// A one-shot job document whose typed default FAILS coercion
/// (`default: "abc"` under `type: int`): `camel compile` must reject it
/// at compile time with no artifact (jobtyped Task 5).
const BAD_TYPED_DEFAULT_DOC: &str = "\
args:
  count:
    type: int
    default: \"abc\"
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: \"direct:${arg:count}\"
    body: ping
routes:
  - id: job-count
    from: direct:7
";

/// A one-shot job document with the `requried:` typo in its `args:`
/// declaration (the A2-era malformed-declaration class): `camel compile`
/// must reject it with the unknown-field diagnostic and no
/// artifact (jobtyped Task 5).
const MALFORMED_DECLARATION_DOC: &str = "\
args:
  count:
    requried: true
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: \"direct:${arg:count}\"
    body: ping
routes:
  - id: job-count
    from: direct:7
";

/// A STRUCTURE-invalid job document whose `args:` declarations are
/// perfectly valid: the unknown top-level field `wat:` fails the full
/// parser (`deny_unknown_fields`) at document load — artifact startup or
/// a normal run — but the compile seam runs the argument-declaration
/// checks ONLY, so `camel compile` must accept it (jobtyped Task 5).
const STRUCTURE_INVALID_WELL_DECLARED_DOC: &str = "\
wat: oops
args:
  count:
    type: int
    default: \"007\"
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: \"direct:${arg:count}\"
    body: ping
routes:
  - id: job-count
    from: direct:7
";

/// Compile `doc` into `artifact` inside `dir` with a clean environment.
fn compile(dir: &Path, doc: &str, artifact: &str, envs: &[(&str, &str)]) -> Output {
    compile_full(dir, doc, artifact, envs, None, &[])
}

/// Full compile invocation with the multidoc source-selection flags: an
/// explicit `--config <Camel.toml>` and repeatable `--profile <name>`.
fn compile_full(
    dir: &Path,
    doc: &str,
    artifact: &str,
    envs: &[(&str, &str)],
    config: Option<&str>,
    profiles: &[&str],
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear()
        .envs(envs.iter().copied())
        .current_dir(dir)
        .args(["compile", doc, "-o", artifact]);
    if let Some(config) = config {
        cmd.arg("--config").arg(config);
    }
    for profile in profiles {
        cmd.arg("--profile").arg(profile);
    }
    cmd.output().expect("spawn `camel compile`")
}

/// Distinct documents compiled once per test process. `camel compile`
/// copies the full ~287 MB `camel` binary into every artifact, so
/// compiling per test would write gigabytes under parallel execution and
/// exhaust the disk (ENOSPC). The fixture compiles each document exactly
/// once — serialized by the `OnceLock` — and tests share the immutable
/// artifacts; only mutation tests copy (see [`deploy_artifact`]).
///
/// The artifacts live in a single cache directory keyed by this test
/// process (`camel-compiled-fixture-<pid>` under the space-probed
/// [`fixture_root`], the repo's `camel-test-*` convention): twelve
/// artifacts at the current binary size total ~3.5 GiB, so the root
/// needs [`REQUIRED_ROOT_FREE`] free before the suite starts — the OS
/// temp directory when it has room (CI, unchanged), the workspace
/// target directory as fallback on small-`/tmp` dev machines. A
/// detached reaper child removes that directory once this process dies
/// — normal exit or crash — and the next run sweeps any leftover, so
/// repeated runs never accumulate the ~3.5 GiB of compiled artifacts.
struct Fixture {
    /// `ROUTE_DOC` artifact (timer→log route).
    route: PathBuf,
    /// `JOB_DOC` artifact (one-shot job, happy path).
    job: PathBuf,
    /// `FAILING_JOB_DOC` artifact (one-shot job, failing pipeline).
    failing_job: PathBuf,
    /// `ENV_DOC` artifact, compiled with a compile-time env value.
    env: PathBuf,
    /// `ARG_DOC` artifact (declared default applies at startup).
    arg: PathBuf,
    /// `REQUIRED_ARG_DOC` artifact (required without a default).
    required_arg: PathBuf,
    /// Multi-document route artifact (config, include, entry route,
    /// indexed route file).
    multi_route: PathBuf,
    /// Multi-document job artifact (config, job document, indexed
    /// route file).
    multi_job: PathBuf,
    /// Multi-document job artifact whose indexed route resolves
    /// `${env:DEPLOY_GREETING}`, compiled with a compile-time value.
    multi_env: PathBuf,
    /// `TYPED_DEFAULT_ARG_DOC` artifact (typed default coerces at
    /// startup, jobtyped Task 5).
    typed_arg: PathBuf,
}

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

/// Free space the fixture root must have before the suite starts
/// compiling: twelve artifacts at the current ~287 MB binary (~3.4 GiB)
/// plus the transient whole-artifact copies (the three mutation tests
/// hold up to one copy each in parallel, and the accepted loose compile
/// writes one more). Bump this when the suite gains artifacts or the
/// binary grows past what the headroom covers.
#[cfg_attr(not(unix), allow(dead_code))]
const REQUIRED_ROOT_FREE: u64 = 5 << 30;

/// The cargo target directory of this workspace: the fallback fixture
/// root when the OS temp directory does not have [`REQUIRED_ROOT_FREE`]
/// bytes free (a small `/tmp` on a shared root partition is the norm on
/// dev machines). `CARGO_TARGET_DIR` wins when cargo set it; otherwise
/// the workspace target directory next to this crate's manifest.
fn cargo_target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
    }
}

/// Free bytes available to an unprivileged process on the filesystem
/// holding `path` (`statvfs(3)`), or `None` when the query is
/// unavailable. Non-unix hosts have no probe: the root falls back to the
/// OS temp directory unprobed there (this suite's CI is Linux).
#[cfg(unix)]
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

#[cfg(not(unix))]
// Unused off unix (`fixture_root` keeps the historic temp-dir path
// there), but kept so the probe's shape documents the fallback.
#[allow(dead_code)]
fn free_bytes(_path: &Path) -> Option<u64> {
    None
}

/// Pick the first candidate root whose free space (per `free_of`)
/// reaches `required`; `None` (probe unavailable) counts as
/// insufficient. When no candidate qualifies, return `Err` naming the
/// requirement and every candidate, so a misconfigured environment
/// fails loudly at suite start instead of ENOSPC-ing mid-suite.
fn pick_root(
    candidates: &[PathBuf],
    required: u64,
    free_of: impl Fn(&Path) -> Option<u64>,
) -> Result<PathBuf, String> {
    for candidate in candidates {
        match free_of(candidate) {
            Some(free) if free >= required => return Ok(candidate.clone()),
            _ => continue,
        }
    }
    let listed = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "no temp root has the required free space: need {required} bytes, candidates [{listed}]"
    ))
}

/// `pick_root` prefers the first roomy candidate in order, treats an
/// unprobeable filesystem as insufficient, and names the requirement
/// plus every candidate in the failure when nothing qualifies.
#[test]
fn fixture_root_picking_prefers_roomy_candidates() {
    let big = PathBuf::from("/big");
    let small = PathBuf::from("/small");
    let unknown = PathBuf::from("/unknown");
    let free_of = |path: &Path| match path {
        p if p == big => Some(10),
        p if p == small => Some(2),
        _ => None,
    };
    // A roomy later candidate is reached past a tight first one.
    assert_eq!(
        pick_root(&[small.clone(), big.clone()], 5, free_of),
        Ok(big.clone())
    );
    // Order is respected: the earlier roomy candidate wins.
    assert_eq!(
        pick_root(&[big.clone(), small.clone()], 5, free_of),
        Ok(big.clone())
    );
    // Unknown free space never qualifies.
    assert!(pick_root(std::slice::from_ref(&unknown), 5, free_of).is_err());
    // The failure names the requirement and every candidate.
    let err = match pick_root(&[small.clone(), unknown], 5, free_of) {
        Err(err) => err,
        Ok(root) => panic!("tight and unprobeable roots must not qualify: {root:?}"),
    };
    assert!(err.contains("5 bytes"), "names the requirement: {err}");
    assert!(
        err.contains("/small") && err.contains("/unknown"),
        "names every candidate: {err}"
    );
}

/// The root directory for this suite's large writes: the compiled
/// fixture and the per-test deploy directories (both must sit on the
/// same filesystem so deploys can hardlink the immutable fixture
/// artifacts). Prefers the OS temp directory when it has room — the
/// CI behavior is unchanged — and falls back to the workspace target
/// directory on machines whose `/tmp` is too small for the ~3.5 GiB
/// suite footprint (bd rc-fdkta: the fixture alone exhausted a 4 GiB
/// `/tmp`, ENOSPC-ing the artifact-mutation tests).
fn fixture_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        #[cfg(not(unix))]
        {
            // No free-space probe off unix: keep the historic temp-dir
            // behavior rather than refusing to run.
            return std::env::temp_dir();
        }
        #[cfg(unix)]
        {
            let candidates = [std::env::temp_dir(), cargo_target_dir()];
            pick_root(&candidates, REQUIRED_ROOT_FREE, free_bytes)
                .expect("fixture root with enough free space")
        }
    })
    .clone()
}

/// The single cache directory for this test process's compiled fixture
/// (repo convention: `camel-test-*` under the chosen fixture root,
/// keyed by the current test process).
fn fixture_dir() -> PathBuf {
    fixture_root().join(format!("camel-compiled-fixture-{}", std::process::id()))
}

/// Spawn a detached reaper that removes `dir` once this test process
/// dies — normal exit or crash. `kill -0` probes the parent; when it
/// fails the parent is gone, so the reaper deletes the fixture. The
/// reaper is reparented to init and reaped there; it never blocks the
/// test.
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

/// Remove fixture directories left by previous test runs whose process
/// is no longer alive (crashed runs, or reapers that have not fired
/// yet). Concurrent runs keep their own PID-keyed directory. Every
/// candidate root is swept, not just the chosen one: runs from before
/// the root fallback may have left their fixture on either filesystem.
fn sweep_stale_fixtures() {
    for root in [std::env::temp_dir(), cargo_target_dir()] {
        sweep_stale_fixtures_in(&root);
    }
}

fn sweep_stale_fixtures_in(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(pid_str) = name.strip_prefix("camel-compiled-fixture-") else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let alive = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !alive {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Compile every distinct document once, into the process-keyed fixture
/// directory. A stale directory from a previous run with the same PID
/// (PID reuse after a crash) is removed first.
fn fixture() -> &'static Fixture {
    FIXTURE.get_or_init(|| {
        sweep_stale_fixtures();
        let dir = fixture_dir();
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("remove stale fixture dir");
        }
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        spawn_reaper(&dir);
        let compile_one =
            |doc_name: &str, doc: &str, artifact: &str, envs: &[(&str, &str)]| -> PathBuf {
                std::fs::write(dir.join(doc_name), doc).expect("write document");
                let output = compile(&dir, doc_name, artifact, envs);
                assert_eq!(
                    output.status.code(),
                    Some(0),
                    "document must compile: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                dir.join(artifact)
            };
        let route = compile_one("app.yaml", ROUTE_DOC, "route.bin", &[]);
        let job = compile_one("ingest.job.yaml", JOB_DOC, "job.bin", &[]);
        let failing_job = compile_one("fail.job.yaml", FAILING_JOB_DOC, "fail.bin", &[]);
        let env = compile_one(
            "greet.job.yaml",
            ENV_DOC,
            "env.bin",
            &[("DEPLOY_GREETING", "compile-secret-value")],
        );
        let arg = compile_one("args.job.yaml", ARG_DOC, "arg.bin", &[]);
        let required_arg = compile_one("reqargs.job.yaml", REQUIRED_ARG_DOC, "req.bin", &[]);
        // Multi-document fixtures: each compile gets its own source
        // subtree so one compile's route sources never capture
        // another's.
        let compile_multi = |subdir: &str,
                             config: &str,
                             entry: &str,
                             entry_doc: &str,
                             route_path: &str,
                             route_doc: &str,
                             artifact: &str,
                             envs: &[(&str, &str)]|
         -> PathBuf {
            let root = dir.join(subdir);
            std::fs::create_dir_all(root.join("conf")).expect("mkdir conf");
            std::fs::create_dir_all(root.join("routes")).expect("mkdir routes");
            std::fs::write(root.join("Camel.toml"), config).expect("write config");
            std::fs::write(root.join("conf").join("base.toml"), MULTI_INCLUDE)
                .expect("write include");
            std::fs::write(root.join(route_path), route_doc).expect("write indexed route");
            std::fs::write(root.join(entry), entry_doc).expect("write entry document");
            // `-o` is relative to the compile working directory (the
            // subtree root), so the artifact lands beside its sources.
            let output = compile_full(&root, entry, artifact, envs, Some("Camel.toml"), &[]);
            assert_eq!(
                output.status.code(),
                Some(0),
                "multi-document compile must succeed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            root.join(artifact)
        };
        let multi_route = compile_multi(
            "multi-route",
            MULTI_CONFIG,
            "multi-app.yaml",
            MULTI_ENTRY_ROUTE,
            "routes/beta.yaml",
            MULTI_INDEXED_ROUTE,
            "multi-route.bin",
            &[],
        );
        let multi_job = compile_multi(
            "multi-job",
            MULTI_JOB_CONFIG,
            "ingest-m.job.yaml",
            MULTI_JOB_DOC,
            "routes/transform.yaml",
            MULTI_JOB_ROUTE,
            "multi-job.bin",
            &[],
        );
        let multi_env = compile_multi(
            "multi-env",
            MULTI_JOB_CONFIG,
            "greet-m.job.yaml",
            MULTI_ENV_JOB_DOC,
            "routes/greet.yaml",
            MULTI_ENV_ROUTE,
            "multi-env.bin",
            &[("DEPLOY_GREETING", "compile-secret-value")],
        );
        let typed_arg = compile_one("typed.job.yaml", TYPED_DEFAULT_ARG_DOC, "typed.bin", &[]);
        Fixture {
            route,
            job,
            failing_job,
            env,
            arg,
            required_arg,
            multi_route,
            multi_job,
            multi_env,
            typed_arg,
        }
    })
}

/// Deploy a shared fixture artifact into a fresh source-free directory
/// (no source document, no Camel.toml, no routes tree) under the
/// canonical `app.bin` name. The deploy directory sits on the fixture
/// root — the same filesystem — so the artifact hardlinks zero-copy
/// into it; the copy fallback stays for filesystems that refuse hard
/// links. The deployed artifact is shared with the fixture: never
/// mutate it in place. Tests that need to alter artifact bytes must
/// copy first (see `artifact_rejects_marked_corruption`).
fn deploy_artifact(artifact: &Path) -> (tempfile::TempDir, PathBuf) {
    let deploy_dir = tempfile::Builder::new()
        .prefix("camel-compiled-deploy-")
        .tempdir_in(fixture_root())
        .expect("deploy tempdir on the fixture root");
    let target = deploy_dir.path().join("app.bin");
    if std::fs::hard_link(artifact, &target).is_err() {
        std::fs::copy(artifact, &target).expect("copy artifact");
    }
    (deploy_dir, target)
}

/// Harness-child branch: decode the artifact named by [`CHILD_ENV`], parse
/// the artifact argv (after `--`), run the embedded document, and exit
/// with its code. Decode and request-validation failures fail closed
/// exactly like the binary self-detect path: the integrity diagnostic
/// prints to stderr and the child exits 2 — never a panic, so the
/// rejection is observable as an exit code with a named diagnostic.
fn run_child() -> i32 {
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
    // Version dispatch mirrors the binary self-detect path: a v1
    // trailer builds the single-document request, a v2 multi-document
    // trailer builds the virtual-store request (store decode plus
    // typed-reference re-validation inside the constructor).
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
    tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(async { camel_cli::compile::runtime::run_embedded_document_code(request).await })
}

/// Run the child branch if this process is a harness child; returns when
/// the child has exited.
fn child_guard() {
    if std::env::var(CHILD_ENV).is_ok() {
        std::process::exit(run_child());
    }
}

/// Spawn the artifact runtime as a harness child that runs to
/// completion (job sends are self-terminating): `output()` waits and
/// drains both pipes, so no pipe buffer can deadlock the child. Returns
/// the `(exit_code, stdout, stderr)` triple.
fn spawn_child_output(
    test: &str,
    dir: &Path,
    artifact: &Path,
    argv: &[&str],
    envs: &[(&str, &str)],
) -> (i32, String, String) {
    let mut cmd = Command::new(std::env::current_exe().expect("current test exe"));
    cmd.env(CHILD_ENV, artifact)
        .envs(envs.iter().copied())
        .current_dir(dir)
        .args(["--exact", test, "--nocapture", "--"])
        .args(argv)
        .stdin(Stdio::null())
        .output()
        .expect("spawn harness child (to completion)")
        .into_code_and_strings()
}

/// Exit code plus both captured streams of a finished child.
trait CodeAndStrings {
    fn into_code_and_strings(self) -> (i32, String, String);
}

impl CodeAndStrings for std::process::Output {
    fn into_code_and_strings(self) -> (i32, String, String) {
        (
            self.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&self.stdout).into_owned(),
            String::from_utf8_lossy(&self.stderr).into_owned(),
        )
    }
}

/// Spawn the artifact runtime as a harness child: `current_exe()` with
/// `--exact <test> --nocapture -- <argv>`, [`CHILD_ENV`] pointing at the
/// artifact, working directory `dir`, and extra environment entries.
fn spawn_child(
    test: &str,
    dir: &Path,
    artifact: &Path,
    argv: &[&str],
    envs: &[(&str, &str)],
) -> KillOnDrop {
    let mut cmd = Command::new(std::env::current_exe().expect("current test exe"));
    cmd.env(CHILD_ENV, artifact)
        .envs(envs.iter().copied())
        .current_dir(dir)
        .args(["--exact", test, "--nocapture", "--"])
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    KillOnDrop(cmd.spawn().expect("spawn harness child"))
}

/// Pipe-drained capture of both child streams (see `tests/common`).
struct Drained {
    out: Arc<Mutex<String>>,
    err: Arc<Mutex<String>>,
}

impl Drained {
    fn captured(&self) -> String {
        format!(
            "stdout:\n{}\nstderr:\n{}",
            self.out.lock().expect("stdout lock").clone(),
            self.err.lock().expect("stderr lock").clone()
        )
    }
}

fn spawn_drained(child: &mut Child) -> Drained {
    let out = Arc::new(Mutex::new(String::new()));
    let err = Arc::new(Mutex::new(String::new()));
    let stdout = child.stdout.take().expect("child stdout piped");
    let stderr = child.stderr.take().expect("child stderr piped");
    thread::spawn({
        let buf = Arc::clone(&out);
        move || drain_to_buffer(stdout, buf)
    });
    thread::spawn({
        let buf = Arc::clone(&err);
        move || drain_to_buffer(stderr, buf)
    });
    Drained { out, err }
}

/// Poll the captured buffers for `marker` until it appears, the child
/// dies, or `timeout` elapses (generous deadlines: see `tests/common`).
fn wait_for_marker(drained: &Drained, marker: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if drained.out.lock().expect("stdout lock").contains(marker)
            || drained.err.lock().expect("stderr lock").contains(marker)
        {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
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

/// SIGTERM after boot, then a graceful exit 0.
fn graceful_shutdown(child: &mut KillOnDrop, drained: &Drained, test: &str) -> i32 {
    assert!(
        wait_for_marker(drained, "context started", Duration::from_secs(60)),
        "artifact must boot through the embedded document: {}",
        drained.captured()
    );
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(child, Duration::from_secs(30));
    assert_eq!(code, 0, "SIGTERM must shut down gracefully: {}", test);
    code
}

/// A compiled route boots and serves from the embedded text alone: the
/// deploy directory holds only the artifact — no source document, no
/// Camel.toml, no routes tree.
#[test]
fn compiled_route_runs_without_source_tree() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().route);
    assert!(!deploy.path().join("app.yaml").exists(), "no source doc");
    assert!(!deploy.path().join("Camel.toml").exists(), "no config");

    let mut child = spawn_child(
        "compiled_route_runs_without_source_tree",
        deploy.path(),
        &artifact,
        &[],
        &[],
    );
    let drained = spawn_drained(&mut child);
    graceful_shutdown(
        &mut child,
        &drained,
        "compiled_route_runs_without_source_tree",
    );
}

/// A compiled one-shot job runs the existing job outcome/report
/// lifecycle: Completed exits 0 with the existing report schema; a
/// failing pipeline exits 1 with a Failed report (exit precedence).
#[test]
fn compiled_job_uses_existing_outcome_report() {
    child_guard();

    // Happy path: exit 0, Completed, existing report schema, virtual
    // document identity, captured reply.
    let (deploy, artifact) = deploy_artifact(&fixture().job);
    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_uses_existing_outcome_report",
        deploy.path(),
        &artifact,
        &["--report", "report.json"],
        &[],
    );
    assert_eq!(
        code, 0,
        "completed job must exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("report.json"))
            .expect("job report must be written"),
    )
    .expect("job report is JSON");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(
        report["document"], "compiled://ingest.job.yaml",
        "report: {report}"
    );
    assert_eq!(report["mode"], "one-shot", "report: {report}");
    assert_eq!(report["terminated_early"], false, "report: {report}");
    assert_eq!(report["reply"]["body"], "job-done", "report: {report}");

    // Failure precedence: a failing route pipeline exits 1 with Failed.
    let (deploy, artifact) = deploy_artifact(&fixture().failing_job);
    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_uses_existing_outcome_report",
        deploy.path(),
        &artifact,
        &["--report", "fail-report.json"],
        &[],
    );
    assert_eq!(
        code, 1,
        "pipeline failure must exit 1;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("fail-report.json"))
            .expect("failed job report must be written"),
    )
    .expect("job report is JSON");
    assert_eq!(report["outcome"], "Failed", "report: {report}");
    assert!(
        report["error"].as_str().is_some_and(|e| !e.is_empty()),
        "report: {report}"
    );
}

/// `${env:NAME}` survives compilation as an expression and resolves from
/// the deployment environment at run time.
#[test]
fn compiled_artifact_resolves_deploy_environment() {
    child_guard();
    // The fixture compiled ENV_DOC WITH a compile-time value present: it
    // must never enter the artifact.
    let (deploy, artifact) = deploy_artifact(&fixture().env);
    let artifact_bytes = std::fs::read(&artifact).expect("artifact exists");
    assert!(
        artifact_bytes
            .windows(b"${env:DEPLOY_GREETING}".len())
            .any(|w| w == b"${env:DEPLOY_GREETING}"),
        "artifact must keep the env expression"
    );
    assert!(
        !artifact_bytes
            .windows(b"compile-secret-value".len())
            .any(|w| w == b"compile-secret-value"),
        "artifact must not embed the compile-time value"
    );

    let (code, stdout, stderr) = spawn_child_output(
        "compiled_artifact_resolves_deploy_environment",
        deploy.path(),
        &artifact,
        &["--report", "env-report.json"],
        &[("DEPLOY_GREETING", "deploy-value")],
    );
    assert_eq!(
        code, 0,
        "job must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("env-report.json")).expect("report written"),
    )
    .expect("report is JSON");
    assert_eq!(
        report["reply"]["body"], "deploy-value",
        "route must observe the deployment value: {report}"
    );
}

// ---------------------------------------------------------------------------
// jobargs Task 3.2: embedded declared arguments. The artifact payload is
// pre-interpolation authoring text; declared `args:` resolve at artifact
// startup through the same parse path normal jobs use, with NO dynamic
// flags — embedded defaults only. Unknown flags (`--arg` included) are
// rejected outside the static artifact surface (`--report`, `--help`,
// `--version`, `--manifest`).
// ---------------------------------------------------------------------------

/// A compiled job applies its embedded declaration defaults exactly like
/// a normal `camel job` run: the same document run both ways produces the
/// same reply message value (`hello`) and exit 0.
#[test]
fn compiled_job_uses_declared_default() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().arg);

    // Artifact run: the embedded default fills `${arg:value}`.
    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_uses_declared_default",
        deploy.path(),
        &artifact,
        &["--report", "arg-report.json"],
        &[],
    );
    assert_eq!(
        code, 0,
        "default resolution must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let artifact_report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("arg-report.json"))
            .expect("artifact report written"),
    )
    .expect("artifact report is JSON");
    assert_eq!(
        artifact_report["outcome"], "Completed",
        "report: {artifact_report}"
    );
    assert_eq!(
        artifact_report["reply"]["body"], "hello",
        "the embedded default must fill ${{arg:value}}: {artifact_report}"
    );

    // Parity: the same document through the normal `camel job` path (no
    // dynamic flags there either) resolves the same default.
    std::fs::write(deploy.path().join("args.job.yaml"), ARG_DOC).expect("write source doc");
    let (code, stdout, stderr) = common::run_binary(
        deploy.path(),
        Path::new(env!("CARGO_BIN_EXE_camel")),
        &["job", "args.job.yaml", "--report", "job-report.json"],
        &[],
    );
    assert_eq!(
        code, 0,
        "normal job run must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let job_report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("job-report.json"))
            .expect("job report written"),
    )
    .expect("job report is JSON");
    assert_eq!(
        artifact_report["reply"]["body"], job_report["reply"]["body"],
        "default resolution parity: artifact vs normal job; {job_report}"
    );
}

/// A compiled job with a required declaration and no default has no
/// CLI flag to fill it: artifact startup rejects it with exit 2,
/// naming the argument, before any boot and without a report.
#[test]
fn compiled_job_rejects_required_without_default() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().required_arg);
    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_rejects_required_without_default",
        deploy.path(),
        &artifact,
        &["--report", "report.json"],
        &[],
    );
    assert_eq!(
        code, 2,
        "required without default must exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("value") && combined.contains("required"),
        "diagnostic must name the argument: {combined}"
    );
    assert!(
        combined.contains("default"),
        "diagnostic must point at declaring a default: {combined}"
    );
    assert!(
        !combined.contains("pass --arg"),
        "artifact diagnostic must not suggest the unavailable --arg surface: {combined}"
    );
    assert!(!combined.contains("context started"), "no boot: {combined}");
    assert!(
        !deploy.path().join("report.json").exists(),
        "a rejected startup writes no report"
    );
}

/// An unknown flag (`--arg` spelling) stays rejected on the artifact
/// surface: the unknown-argument rejection applies (exit 2, argument
/// named, no boot).
#[test]
fn compiled_job_rejects_arg_flag() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().arg);
    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_rejects_arg_flag",
        deploy.path(),
        &artifact,
        &["--arg", "value=other"],
        &[],
    );
    assert_eq!(
        code, 2,
        "--arg must be rejected as unknown;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("--arg"),
        "must name the rejected argument: {combined}"
    );
    assert!(!combined.contains("context started"), "no boot: {combined}");
}

// ---------------------------------------------------------------------------
// jobtyped Task 5: compile-time declaration validation and typed-default
// artifact parity. `camel compile` runs the argument-declaration checks
// (`type` grammar, typed-default coercion) on job documents — exit 2, no
// artifact on failure — and compiled artifacts coerce embedded typed
// defaults at startup through the same rules as a normal job.
// ---------------------------------------------------------------------------

/// A compiled job coerces its embedded TYPED default exactly like a
/// normal `camel job` run: `count: {type: int, default: "007"}`
/// resolves to the canonical `7`, so both runs send to the identical
/// interpolated target `direct:7` — the route consumer is declared only
/// there, so an uncoerced `007` target would find no consumer and fail —
/// and both runs exit 0.
#[test]
fn compiled_job_coerces_typed_default() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().typed_arg);

    // Artifact run: the embedded typed default coerces `007` -> `7`.
    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_coerces_typed_default",
        deploy.path(),
        &artifact,
        &["--report", "typed-report.json"],
        &[],
    );
    assert_eq!(
        code, 0,
        "typed default must coerce and complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let artifact_report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("typed-report.json"))
            .expect("artifact report written"),
    )
    .expect("artifact report is JSON");
    assert_eq!(
        artifact_report["outcome"], "Completed",
        "report: {artifact_report}"
    );
    assert_eq!(
        artifact_report["reply"]["body"], "ping",
        "the coerced target must route to the `direct:7` consumer: {artifact_report}"
    );

    // Parity: the same document through the normal `camel job` path
    // (no dynamic flags there either) coerces to the identical send target.
    std::fs::write(deploy.path().join("typed.job.yaml"), TYPED_DEFAULT_ARG_DOC)
        .expect("write source doc");
    let (code, stdout, stderr) = common::run_binary(
        deploy.path(),
        Path::new(env!("CARGO_BIN_EXE_camel")),
        &["job", "typed.job.yaml", "--report", "typed-job-report.json"],
        &[],
    );
    assert_eq!(
        code, 0,
        "normal job run must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let job_report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("typed-job-report.json"))
            .expect("job report written"),
    )
    .expect("job report is JSON");
    assert_eq!(job_report["outcome"], "Completed", "report: {job_report}");
    assert_eq!(
        artifact_report["reply"]["body"], job_report["reply"]["body"],
        "identical send target: artifact vs normal job; {job_report}"
    );
}

/// Compiling a job document whose typed default fails coercion exits 2
/// with the `ArgumentCoercion` diagnostic naming the argument and
/// produces NO artifact file (jobtyped Task 5).
#[test]
fn compile_rejects_bad_typed_default() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("bad.job.yaml"), BAD_TYPED_DEFAULT_DOC).expect("write document");
    let output = compile(dir.path(), "bad.job.yaml", "bad.bin", &[]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "compile must reject the bad typed default: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("count") && stderr.contains("int") && stderr.contains("abc"),
        "diagnostic must name the argument, the expected type, and the raw value: {stderr}"
    );
    assert!(
        !dir.path().join("bad.bin").exists(),
        "a rejected compile must produce no artifact"
    );
    assert!(
        !dir.path().join("bad.bin.tmp").exists(),
        "a rejected compile must leave no partial artifact"
    );
}

/// Compiling a job document with a malformed declaration (the
/// `requried:` typo) exits 2 with the unknown-field diagnostic and
/// produces no artifact — the same declaration class the load path
/// rejects (jobtyped Task 5).
#[test]
fn compile_rejects_malformed_declaration() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("typo.job.yaml"), MALFORMED_DECLARATION_DOC)
        .expect("write document");
    let output = compile(dir.path(), "typo.job.yaml", "typo.bin", &[]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "compile must reject the malformed declaration: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requried") && stderr.contains("count"),
        "unknown-field diagnostic must name the field and the argument: {stderr}"
    );
    assert!(
        !dir.path().join("typo.bin").exists(),
        "a rejected compile must produce no artifact"
    );
    assert!(
        !dir.path().join("typo.bin.tmp").exists(),
        "a rejected compile must leave no partial artifact"
    );
}

/// The compile seam is declaration-ONLY: a job document that is
/// structure-invalid for the full parser (unknown top-level field under
/// `deny_unknown_fields`) but whose `args:` declarations are perfectly
/// valid compiles with exit 0 and a written artifact. Structure
/// rejection belongs to artifact startup / normal runs, not to `camel
/// compile` — this pins the spec sentence "no other execution-value
/// validation SHALL run at compile time" against future refactors that
/// would swap the seam to the full parser (jobtyped Task 5).
#[test]
fn compile_allows_structure_invalid_but_well_declared_job() {
    // The accepted compile writes a full ~binary-size artifact, so the
    // source directory sits on the fixture root with the suite's other
    // large writes.
    let dir = tempfile::Builder::new()
        .prefix("camel-compile-loose-")
        .tempdir_in(fixture_root())
        .expect("tempdir on the fixture root");
    std::fs::write(
        dir.path().join("loose.job.yaml"),
        STRUCTURE_INVALID_WELL_DECLARED_DOC,
    )
    .expect("write document");
    let output = compile(dir.path(), "loose.job.yaml", "loose.bin", &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "compile must run declaration checks ONLY;\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dir.path().join("loose.bin").is_file(),
        "the accepted compile must write the artifact"
    );
}

/// A compiled artifact runs on a read-only root: no temporary
/// extraction, no watcher activation, and the only directory content
/// stays the artifact itself.
#[test]
fn compiled_artifact_does_not_extract_or_watch() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().route);

    // Read-only deploy root (owner r-x): any extraction would fail here.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(deploy.path(), std::fs::Permissions::from_mode(0o555))
            .expect("chmod read-only");
    }

    let mut child = spawn_child(
        "compiled_artifact_does_not_extract_or_watch",
        deploy.path(),
        &artifact,
        &[],
        &[],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(&drained, "context started", Duration::from_secs(60)),
        "artifact must boot on a read-only root: {}",
        drained.captured()
    );
    let all_output = format!(
        "{}{}",
        drained.out.lock().expect("stdout lock"),
        drained.err.lock().expect("stderr lock")
    );
    assert!(
        !all_output.contains("hot-reload watching"),
        "the watcher must never activate: {all_output}"
    );
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(&mut child, Duration::from_secs(30));
    assert_eq!(code, 0, "graceful shutdown on read-only root");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(deploy.path(), std::fs::Permissions::from_mode(0o755))
            .expect("restore writable for cleanup");
    }
    let mut entries: Vec<String> = std::fs::read_dir(deploy.path())
        .expect("read deploy dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec!["app.bin".to_string()],
        "no extraction or other writes: {entries:?}"
    );
}

/// A route artifact writes the exact RouteReport status JSON on graceful
/// shutdown and exits 0.
#[test]
fn compiled_route_report_writes_status_json() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().route);

    let mut child = spawn_child(
        "compiled_route_report_writes_status_json",
        deploy.path(),
        &artifact,
        &["--report", "status.json"],
        &[],
    );
    let drained = spawn_drained(&mut child);
    graceful_shutdown(
        &mut child,
        &drained,
        "compiled_route_report_writes_status_json",
    );

    let report = std::fs::read_to_string(deploy.path().join("status.json"))
        .expect("route status report must be written");
    assert_eq!(
        report.trim(),
        r#"{"kind":"route","status":"completed","error":null}"#,
        "exact RouteReport JSON"
    );
}

// ---------------------------------------------------------------------------
// Task 2.3 (cli-compile and multidoc): self-detection before CLI parsing.
// These tests spawn the artifact binary itself, so `main` runs the trailer
// probe before Clap; the multidoc cases drive the v2 virtual-store path.
// ---------------------------------------------------------------------------

/// Make `path` executable (artifacts written by hand in the tests below).
#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod executable");
}

/// A trailer-free binary keeps the normal Clap CLI: the probe returns
/// `None` and standard commands behave exactly as before. Clap
/// fingerprints: `--version` exits 0 printing `camel <version>`, and an
/// unknown flag is a Clap `error:` with exit 2.
#[test]
fn trailer_free_binary_keeps_normal_cli() {
    let camel = PathBuf::from(env!("CARGO_BIN_EXE_camel"));
    let dir = tempfile::tempdir().expect("tempdir");

    let (code, stdout, stderr) = common::run_binary(dir.path(), &camel, &["--version"], &[]);
    assert_eq!(
        code, 0,
        "plain `--version` exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.trim().starts_with("camel "),
        "Clap version output: {stdout}"
    );

    let (code, stdout, stderr) = common::run_binary(dir.path(), &camel, &["--watch"], &[]);
    assert_eq!(
        code, 2,
        "unknown flag is Clap misuse;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.starts_with("error:"),
        "Clap error fingerprint: {stderr}"
    );
}

/// `--manifest` prints the operational manifest and exits 0 without
/// booting the embedded route.
#[test]
fn artifact_manifest_exits_without_boot() {
    let (deploy, artifact) = deploy_artifact(&fixture().route);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    assert_eq!(
        code, 0,
        "--manifest exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is manifest JSON");
    assert_eq!(manifest["kind"], "route", "manifest: {manifest}");
    assert_eq!(manifest["source_name"], "app.yaml", "manifest: {manifest}");
    assert_eq!(
        manifest["runtime_version"],
        camel_cli::compile::manifest::RUNTIME_VERSION,
        "manifest: {manifest}"
    );
    assert!(
        manifest["components"]
            .as_array()
            .is_some_and(|c| c.iter().any(|s| s.as_str() == Some("timer"))),
        "embedded components listed: {manifest}"
    );
    assert!(
        manifest["env_names"].as_array().is_some(),
        "required env names listed: {manifest}"
    );
    assert!(
        manifest["listeners"].as_array().is_some(),
        "listener declarations listed: {manifest}"
    );
    let all = format!("{stdout}{stderr}");
    assert!(!all.contains("context started"), "no route boot: {all}");
}

/// `--manifest` on a v2 virtual-store artifact prints the schema-2
/// canonical manifest — `manifest_schema` 2, the runtime version, and
/// EVERY embedded logical path with its document kind — and exits 0
/// without booting (multidoc Task 2.3). The v1 six-field form above is
/// untouched; a v2 artifact carries the independent store metadata.
#[test]
fn artifact_manifest_lists_virtual_store_without_boot() {
    let (deploy, artifact) = deploy_artifact(&fixture().multi_route);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    assert_eq!(
        code, 0,
        "--manifest exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is manifest JSON");
    assert_eq!(manifest["manifest_schema"], 2, "manifest: {manifest}");
    assert_eq!(manifest["kind"], "route", "manifest: {manifest}");
    assert_eq!(
        manifest["source_name"], "multi-app.yaml",
        "manifest: {manifest}"
    );
    assert_eq!(
        manifest["runtime_version"],
        camel_cli::compile::manifest::RUNTIME_VERSION,
        "manifest: {manifest}"
    );

    // Every embedded logical path of the virtual store, in canonical
    // path order, with its document kind.
    let files = manifest["embedded_files"]
        .as_array()
        .expect("embedded_files array");
    let listed: Vec<(String, String)> = files
        .iter()
        .map(|f| {
            (
                f["path"].as_str().expect("path").to_string(),
                f["kind"].as_str().expect("kind").to_string(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            ("Camel.toml".to_string(), "config".to_string()),
            ("conf/base.toml".to_string(), "include".to_string()),
            ("multi-app.yaml".to_string(), "route".to_string()),
            ("routes/beta.yaml".to_string(), "route".to_string()),
        ],
        "every embedded logical path is listed: {manifest}"
    );

    let all = format!("{stdout}{stderr}");
    assert!(!all.contains("context started"), "no route boot: {all}");
}

// ---------------------------------------------------------------------------
// jobcoexist Task 5: job artifacts project the ambient runtime journal
// and observability stack away (boot projection), while route artifacts
// keep the config-declared diagnostic listeners. The manifest reports
// the effective runtime; the runtime proof holds both diagnostic ports
// for the artifact's whole life, so exit 0 with a Completed report
// proves the artifact never even tried to bind (ADR-0070: no
// release/re-bind window).
// ---------------------------------------------------------------------------

/// A one-shot job document for the observability-enabled fixture
/// (self-contained inline routes, placed beside its config at the
/// subtree root).
const OBS_JOB_DOC: &str = "\
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: ping
routes:
  - id: obs-job-transform
    from: direct:transform
    steps:
      - set_body:
          value: obs-job-done
";

/// A route document compiled with the SAME observability-enabled config.
/// Manifest only — never booted.
const OBS_ROUTE_DOC: &str = "\
routes:
  - id: obs-route
    from: direct:start
    steps:
      - to: log:obs-route
";

/// The compiled observability fixtures: a job artifact and a route
/// artifact, both built from one `Camel.toml` that enables the
/// Prometheus and health listeners on two ephemeral ports and points
/// the runtime journal at the fixture directory. Both listeners are
/// bound BEFORE the config is written and the artifacts are compiled,
/// and they are held for this test process's whole life — the
/// runtime-proof test then executes the job artifact with zero
/// release/re-bind window.
struct ObsFixture {
    job: PathBuf,
    route: PathBuf,
    prom_port: u16,
    health_port: u16,
    /// The `[default.runtime_journal]` path written into the embedded
    /// config: absolute into the fixture dir, never created by compile —
    /// its post-run absence witnesses that the job opened no journal.
    journal: PathBuf,
    /// The pre-bound diagnostic listeners, held until process exit.
    _held: (TcpListener, TcpListener),
}

static OBS_FIXTURE: OnceLock<ObsFixture> = OnceLock::new();

fn obs_fixture() -> &'static ObsFixture {
    OBS_FIXTURE.get_or_init(|| {
        // Same process-keyed fixture directory as `fixture()`: the
        // shared sweep/reaper then clean the extra compiled artifacts.
        sweep_stale_fixtures();
        let dir = fixture_dir();
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        spawn_reaper(&dir);

        // Bind both diagnostic listeners first and never release them;
        // writing their ports into the config afterwards leaves no
        // window in which another process could take the endpoints.
        let prom = TcpListener::bind("127.0.0.1:0").expect("bind prometheus listener");
        let health = TcpListener::bind("127.0.0.1:0").expect("bind health listener");
        let prom_port = prom.local_addr().expect("prometheus addr").port();
        let health_port = health.local_addr().expect("health addr").port();
        let journal = dir.join("obs-job").join("journal.db");
        let config = format!(
            r#"[default]
log_level = "off"

[default.runtime_journal]
path = "{}"
durability = "immediate"

[default.observability.prometheus]
enabled = true
host = "127.0.0.1"
port = {prom_port}

[default.observability.health]
enabled = true
host = "127.0.0.1"
port = {health_port}
"#,
            journal.display(),
        );

        // Each compile gets its own subtree so the two embedded
        // configs never capture each other's sources.
        let compile_obs = |subdir: &str, entry: &str, entry_doc: &str, artifact: &str| -> PathBuf {
            let root = dir.join(subdir);
            std::fs::create_dir_all(&root).expect("mkdir obs subtree");
            std::fs::write(root.join("Camel.toml"), &config).expect("write obs config");
            std::fs::write(root.join(entry), entry_doc).expect("write obs entry document");
            let output = compile_full(&root, entry, artifact, &[], Some("Camel.toml"), &[]);
            assert_eq!(
                output.status.code(),
                Some(0),
                "observability fixture must compile: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            root.join(artifact)
        };
        let job = compile_obs("obs-job", "ingest-obs.job.yaml", OBS_JOB_DOC, "obs-job.bin");
        let route = compile_obs("obs-route", "app-obs.yaml", OBS_ROUTE_DOC, "obs-route.bin");
        ObsFixture {
            job,
            route,
            prom_port,
            health_port,
            journal,
            _held: (prom, health),
        }
    })
}

/// The job artifact's manifest omits the config-declared Prometheus and
/// health listeners: the boot projection suppresses both at runtime, so
/// the manifest reports the effective runtime — neither reserved
/// endpoint may appear.
#[test]
fn job_artifact_manifest_omits_suppressed_listeners() {
    let obs = obs_fixture();
    let prom = format!("127.0.0.1:{}", obs.prom_port);
    let health = format!("127.0.0.1:{}", obs.health_port);
    let (deploy, artifact) = deploy_artifact(&obs.job);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    assert_eq!(
        code, 0,
        "--manifest exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is manifest JSON");
    assert_eq!(manifest["kind"], "job", "manifest: {manifest}");
    let listeners: Vec<String> = manifest["listeners"]
        .as_array()
        .expect("listeners array")
        .iter()
        .map(|v| v.as_str().expect("listener string").to_string())
        .collect();
    assert!(
        !listeners.contains(&prom),
        "job manifest must omit the suppressed Prometheus listener {prom}: {listeners:?}"
    );
    assert!(
        !listeners.contains(&health),
        "job manifest must omit the suppressed health listener {health}: {listeners:?}"
    );
    let all = format!("{stdout}{stderr}");
    assert!(!all.contains("context started"), "no job boot: {all}");
}

/// The runtime proof, with no release/re-bind window (ADR-0070): both
/// diagnostic ports were bound before the config was written and stay
/// held by this test process through the entire artifact execution — so
/// exit 0 with a Completed report proves the embedded job never tried
/// to bind a listener (any bind attempt would fail the boot with exit
/// 2), and the post-run absence of the journal file is the direct
/// witness that the projected-away runtime journal was never opened.
#[test]
fn job_artifact_binds_no_listeners_with_ports_prebound() {
    let obs = obs_fixture();
    let (deploy, artifact) = deploy_artifact(&obs.job);
    let (code, stdout, stderr) =
        common::run_binary(deploy.path(), &artifact, &["--report", "report.json"], &[]);
    assert_eq!(
        code, 0,
        "job artifact must complete with both diagnostic ports held;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("report.json"))
            .expect("job report must be written"),
    )
    .expect("job report is JSON");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert!(
        !obs.journal.exists(),
        "job artifact must not open the runtime journal: {}",
        obs.journal.display()
    );
}

/// The route artifact compiled from the SAME config keeps both
/// config-declared listeners in its manifest — existing behavior pinned
/// (route boots really do bind them).
#[test]
fn route_artifact_manifest_keeps_config_declared_listeners() {
    let obs = obs_fixture();
    let prom = format!("127.0.0.1:{}", obs.prom_port);
    let health = format!("127.0.0.1:{}", obs.health_port);
    let (deploy, artifact) = deploy_artifact(&obs.route);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    assert_eq!(
        code, 0,
        "--manifest exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is manifest JSON");
    assert_eq!(manifest["kind"], "route", "manifest: {manifest}");
    let listeners: Vec<String> = manifest["listeners"]
        .as_array()
        .expect("listeners array")
        .iter()
        .map(|v| v.as_str().expect("listener string").to_string())
        .collect();
    assert!(
        listeners.contains(&prom),
        "route manifest must keep the Prometheus listener {prom}: {listeners:?}"
    );
    assert!(
        listeners.contains(&health),
        "route manifest must keep the health listener {health}: {listeners:?}"
    );
}

/// Duplicate/exclusive flags, a missing `--report` value, an unknown
/// flag, and a positional argument each exit 2 and name the rejected
/// argument, without booting (multidoc Task 2.3: exercised on a v2
/// virtual-store artifact — argument parsing rejects misuse before any
/// version dispatch or boot).
#[test]
fn artifact_rejects_unknown_positional_and_duplicate_args() {
    let (deploy, artifact) = deploy_artifact(&fixture().multi_route);
    let cases: &[(&[&str], &str)] = &[
        (&["--help", "--version"], "--version"),
        (&["--report", "a.json", "--report", "b.json"], "--report"),
        (&["--report"], "--report"),
        (&["--watch"], "--watch"),
        (&["routes.yaml"], "routes.yaml"),
    ];
    for (argv, named) in cases {
        let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, argv, &[]);
        assert_eq!(
            code, 2,
            "argv {argv:?} must exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let combined = format!("{stdout}{stderr}");
        assert!(
            combined.contains(named),
            "argv {argv:?} must name the rejected argument: {combined}"
        );
        assert!(!combined.contains("context started"), "no boot: {combined}");
    }
}

/// Marked corruption (terminal magic retained) fails closed: nonzero
/// integrity diagnostic and no boot. One case mutates the last
/// embedded-data byte (payload/manifest region, past the executable
/// image); the other mutates a footer checksum byte. Both break the
/// BLAKE3 checksum while the terminal magic stays intact. Each variant
/// writes its whole-artifact copy, runs it, and removes it before the
/// next variant builds — only one ~binary-size copy is on disk at a
/// time (bd rc-fdkta: holding every copy live ENOSPC'd the suite).
#[test]
fn artifact_rejects_marked_corruption() {
    let (deploy, artifact) = deploy_artifact(&fixture().route);
    let valid = std::fs::read(&artifact).expect("artifact bytes");

    let data_end = valid.len() - trailer::FOOTER_LEN;
    for (name, offset) in [("data", data_end - 1), ("footer", data_end + 28)] {
        let mut bytes = valid.clone();
        bytes[offset] ^= 0xFF;
        let path = deploy.path().join(format!("corrupt-{name}.bin"));
        std::fs::write(&path, bytes).expect("write corrupt artifact");
        #[cfg(unix)]
        make_executable(&path);
        let (code, stdout, stderr) = common::run_binary(deploy.path(), &path, &[], &[]);
        assert_eq!(
            code, 2,
            "corrupt {name} must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let combined = format!("{stdout}{stderr}");
        assert!(
            combined.contains("integrity error"),
            "corrupt {name} must carry an integrity diagnostic: {combined}"
        );
        assert!(!combined.contains("context started"), "no boot: {combined}");
        drop(std::fs::remove_file(&path));
    }
}

/// v2 marked corruption and unknown schemas fail closed through the REAL
/// self-detect entry (multidoc Task 2.3): the artifact binary itself
/// probes its trailer before Clap, and every rejected form retains the
/// terminal marker while failing with an integrity/format diagnostic,
/// exit 2, and zero route boot.
///
/// Two byte-level corruptions of a real compiled artifact — the last
/// embedded content byte and a v2 footer checksum byte — break the
/// BLAKE3 checksum. Two checksum-consistent rejections carry exactly one
/// schema mutation re-sealed through `encode_v2`, so the named failure
/// is the schema rule, never a checksum mismatch: an index declaring
/// `store_schema` 99, and a manifest declaring `manifest_schema` 99.
#[test]
fn artifact_rejects_v2_corruption_and_unknown_schemas() {
    use camel_cli::compile::store::{
        StoreDocument, StoreEntryKind, StoreIndex, VirtualDocumentStore,
    };
    use camel_cli::compile::trailer::{TrailerKind, TrailerV2};

    let (deploy, artifact) = deploy_artifact(&fixture().multi_route);
    let valid = std::fs::read(&artifact).expect("artifact bytes");
    let data_end = valid.len() - trailer::FOOTER_LEN_V2;
    let footer = &valid[data_end..];
    let le = |range: std::ops::Range<usize>| {
        u64::from_le_bytes(footer[range].try_into().expect("length field"))
    };
    let total = (le(12..20) + le(20..28) + le(28..36)) as usize;
    let content_start = data_end - total;
    // The executable image ends right before the leading family magic.
    let image = valid[..content_start - trailer::MAGIC.len()].to_vec();

    // Run the rejected image through the real binary and assert the
    // closed failure: exit 2, integrity diagnostic naming the defect,
    // and no route boot. Each ~283 MB file is removed before the next
    // variant to keep the transient disk use bounded.
    let run_rejected = |name: &str, bytes: &[u8], diagnostic: &str| {
        let path = deploy.path().join(name);
        std::fs::write(&path, bytes).expect("write rejected artifact");
        #[cfg(unix)]
        make_executable(&path);
        let (code, stdout, stderr) = common::run_binary(deploy.path(), &path, &[], &[]);
        assert_eq!(
            code, 2,
            "{name} must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let combined = format!("{stdout}{stderr}");
        assert!(
            combined.contains("integrity error"),
            "{name} must carry the integrity diagnostic: {combined}"
        );
        assert!(
            combined.contains(diagnostic),
            "{name} must name the failure: {combined}"
        );
        assert!(
            !combined.contains("context started"),
            "{name} must not boot: {combined}"
        );
        drop(std::fs::remove_file(&path));
    };

    // Content corruption: flip the last embedded content byte — framing
    // stays intact (both magics), only the checksum breaks.
    let content_len = le(12..20) as usize;
    let mut corrupt_content = valid.clone();
    corrupt_content[content_start + content_len - 1] ^= 0xFF;
    run_rejected(
        "corrupt-content.bin",
        &corrupt_content,
        "trailer checksum mismatch",
    );

    // Footer corruption: flip a byte inside the v2 footer checksum.
    let mut corrupt_footer = valid.clone();
    corrupt_footer[data_end + 40] ^= 0xFF;
    run_rejected(
        "corrupt-footer.bin",
        &corrupt_footer,
        "trailer checksum mismatch",
    );

    // Checksum-consistent schema rejections: a minimal valid store with
    // exactly one schema mutation per artifact, re-sealed via
    // `encode_v2` and prefixed with the executable image so the real
    // self-detect path decodes it.
    let route_text = "routes:\n  - id: demo\n    from: timer:tick?period=300\n    steps:\n      - to: log:demo\n";
    let store = VirtualDocumentStore::build(
        "app.yaml",
        &[StoreDocument {
            path: "app.yaml".to_string(),
            kind: StoreEntryKind::Route,
            bytes: route_text.as_bytes().to_vec(),
        }],
        &[],
        &["app.yaml".to_string()],
    )
    .expect("valid store builds");
    let manifest = camel_cli::compile::manifest::derive_for_store(
        &store,
        TrailerKind::Route,
        &[("app.yaml".to_string(), route_text.to_string())],
    )
    .expect("manifest derives");

    // Unknown store schema in the index.
    let mut bad_index: StoreIndex = store.index.clone();
    bad_index.store_schema = 99;
    let mut bytes = image.clone();
    bytes.extend_from_slice(&trailer::encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content.clone(),
        index: bad_index.encode_canonical().expect("canonical index"),
        manifest: manifest.to_canonical_json().into_bytes(),
    }));
    run_rejected("schema99-index.bin", &bytes, "unsupported store schema 99");

    // Unknown manifest schema.
    let mut manifest_value: serde_json::Value =
        serde_json::from_str(&manifest.to_canonical_json()).expect("manifest JSON");
    manifest_value["manifest_schema"] = serde_json::json!(99);
    let mut bytes = image;
    bytes.extend_from_slice(&trailer::encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content.clone(),
        index: store.index.encode_canonical().expect("canonical index"),
        manifest: serde_json::to_string(&manifest_value)
            .expect("manifest JSON")
            .into_bytes(),
    }));
    run_rejected(
        "schema99-manifest.bin",
        &bytes,
        "unsupported manifest schema 99",
    );
}

/// Truncation through the terminal magic leaves no recognizable trailer,
/// so the image is indistinguishable from a plain executable and falls
/// back to the unchanged Clap path. Each variant's whole-artifact copy
/// is removed after its run (bd rc-fdkta: holding both copies live
/// ENOSPC'd the suite).
#[test]
fn artifact_truncated_without_marker_keeps_clap_fallback() {
    let (deploy, artifact) = deploy_artifact(&fixture().route);
    let mut bytes = std::fs::read(&artifact).expect("artifact bytes");
    // Cut exactly the terminal magic: decode would report an absent
    // trailer, so argv must reach Clap unchanged.
    bytes.truncate(bytes.len() - trailer::MAGIC.len());
    assert_eq!(
        trailer::decode(&bytes),
        Ok(None),
        "truncation must remove the marker"
    );
    let path = deploy.path().join("truncated.bin");
    std::fs::write(&path, bytes).expect("write truncated artifact");
    #[cfg(unix)]
    make_executable(&path);

    // A normal CLI argument: Clap rejects the unknown flag with its own
    // `error:` fingerprint and exit 2 (the artifact argv guard would not
    // print that prefix).
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &path, &["--watch"], &[]);
    assert_eq!(
        code, 2,
        "Clap misuse exits 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.starts_with("error:"),
        "unchanged Clap fallback: {stderr}"
    );
    drop(std::fs::remove_file(&path));

    // v2 (multidoc Task 2.3): a virtual-store artifact truncated through
    // the terminal magic is likewise indistinguishable from a plain
    // executable and falls back to the unchanged Clap path.
    let (deploy, artifact) = deploy_artifact(&fixture().multi_route);
    let mut bytes = std::fs::read(&artifact).expect("artifact bytes");
    bytes.truncate(bytes.len() - trailer::MAGIC.len());
    assert!(
        matches!(trailer::decode_artifact(&bytes), Ok(None)),
        "truncation must remove the v2 marker"
    );
    let path = deploy.path().join("truncated-v2.bin");
    std::fs::write(&path, bytes).expect("write truncated v2 artifact");
    #[cfg(unix)]
    make_executable(&path);

    let (code, stdout, stderr) = common::run_binary(deploy.path(), &path, &["--watch"], &[]);
    assert_eq!(
        code, 2,
        "Clap misuse exits 2 on truncated v2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.starts_with("error:"),
        "unchanged Clap fallback for truncated v2: {stderr}"
    );
    drop(std::fs::remove_file(&path));
}

/// `--help` and `--version` each exit 0 without booting.
#[test]
fn artifact_help_and_version_exit_zero() {
    let (deploy, artifact) = deploy_artifact(&fixture().route);

    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--help"], &[]);
    assert_eq!(
        code, 0,
        "--help exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("camel compiled artifact usage"),
        "artifact usage text: {stdout}"
    );

    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--version"], &[]);
    assert_eq!(
        code, 0,
        "--version exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.trim(),
        format!("camel {}", camel_cli::compile::manifest::RUNTIME_VERSION),
        "artifact version line"
    );

    for stream in [&stdout, &stderr] {
        assert!(!stream.contains("context started"), "no boot: {stream}");
    }
}

// ---------------------------------------------------------------------------
// multidoc Task 2.2: virtual-store runtime for v2 multi-document
// artifacts.
// ---------------------------------------------------------------------------

/// A multi-document route artifact runs with no source tree and no
/// working-directory configuration: the deploy directory holds only the
/// artifact, every embedded route (entry document plus indexed route
/// file) boots and executes, and the embedded configuration/include
/// feed the run.
#[test]
fn compiled_multidocument_route_runs_without_source_tree() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().multi_route);
    for absent in ["multi-app.yaml", "Camel.toml", "routes", "conf"] {
        assert!(
            !deploy.path().join(absent).exists(),
            "no source/config tree: {absent} must not exist"
        );
    }

    let mut child = spawn_child(
        "compiled_multidocument_route_runs_without_source_tree",
        deploy.path(),
        &artifact,
        &[],
        &[],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(&drained, "context started", Duration::from_secs(60)),
        "artifact must boot without its source tree: {}",
        drained.captured()
    );
    // Every embedded route executes: both body markers reach the log.
    assert!(
        wait_for_marker(&drained, "alpha-marker", Duration::from_secs(30)),
        "entry-document route must execute: {}",
        drained.captured()
    );
    assert!(
        wait_for_marker(&drained, "beta-marker", Duration::from_secs(30)),
        "indexed route file must execute: {}",
        drained.captured()
    );
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(&mut child, Duration::from_secs(30));
    assert_eq!(
        code,
        0,
        "graceful shutdown after full multi-document run: {}",
        drained.captured()
    );
}

/// A multi-document job artifact consumes its embedded job document,
/// indexed route sources, and configuration through the existing job
/// outcome/report lifecycle — no source-tree read.
#[test]
fn compiled_job_uses_embedded_route_plan_and_report() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().multi_job);
    for absent in ["ingest-m.job.yaml", "Camel.toml", "routes", "conf"] {
        assert!(
            !deploy.path().join(absent).exists(),
            "no source/config tree: {absent} must not exist"
        );
    }

    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_uses_embedded_route_plan_and_report",
        deploy.path(),
        &artifact,
        &["--report", "report.json"],
        &[],
    );
    assert_eq!(
        code, 0,
        "multi-document job must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("report.json"))
            .expect("job report must be written"),
    )
    .expect("job report is JSON");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(
        report["document"], "compiled://ingest-m.job.yaml",
        "virtual entry-point identity: {report}"
    );
    assert_eq!(report["mode"], "one-shot", "report: {report}");
    assert_eq!(
        report["reply"]["body"], "multi-job-done",
        "indexed route file must drive the pipeline: {report}"
    );
}

/// `${env:NAME}` inside a multi-document artifact survives compilation
/// raw and resolves from the deployment environment only.
#[test]
fn compiled_multidocument_resolves_deployment_environment() {
    child_guard();
    // The fixture compiled the artifact WITH a compile-time value
    // present: it must never enter the artifact.
    let (deploy, artifact) = deploy_artifact(&fixture().multi_env);
    let artifact_bytes = std::fs::read(&artifact).expect("artifact exists");
    assert!(
        artifact_bytes
            .windows(b"${env:DEPLOY_GREETING}".len())
            .any(|w| w == b"${env:DEPLOY_GREETING}"),
        "artifact must keep the env expression"
    );
    assert!(
        !artifact_bytes
            .windows(b"compile-secret-value".len())
            .any(|w| w == b"compile-secret-value"),
        "artifact must not embed the compile-time value"
    );

    let (code, stdout, stderr) = spawn_child_output(
        "compiled_multidocument_resolves_deployment_environment",
        deploy.path(),
        &artifact,
        &["--report", "env-report.json"],
        &[("DEPLOY_GREETING", "deploy-value")],
    );
    assert_eq!(
        code, 0,
        "job must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("env-report.json")).expect("report written"),
    )
    .expect("report is JSON");
    assert_eq!(
        report["reply"]["body"], "deploy-value",
        "route must observe the deployment value only: {report}"
    );
}

/// A multi-document artifact runs on a read-only root with no source
/// files: no temporary extraction, no glob expansion, no watcher
/// activation, and the virtual-store loading seam (not the pattern
/// discovery seam) feeds the routes.
#[test]
fn compiled_multidocument_does_not_extract_glob_or_watch() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().multi_route);

    // Read-only deploy root (owner r-x): any extraction would fail here.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(deploy.path(), std::fs::Permissions::from_mode(0o555))
            .expect("chmod read-only");
    }

    let mut child = spawn_child(
        "compiled_multidocument_does_not_extract_glob_or_watch",
        deploy.path(),
        &artifact,
        &[],
        &[],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(&drained, "context started", Duration::from_secs(60)),
        "artifact must boot on a read-only root: {}",
        drained.captured()
    );
    let all_output = format!(
        "{}{}",
        drained.out.lock().expect("stdout lock"),
        drained.err.lock().expect("stderr lock")
    );
    assert!(
        all_output.contains("virtual store"),
        "the virtual-store loading seam must be visible: {all_output}"
    );
    assert!(
        !all_output.contains("loading routes from patterns"),
        "no glob discovery may run: {all_output}"
    );
    assert!(
        !all_output.contains("hot-reload watching"),
        "the watcher must never activate: {all_output}"
    );
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(&mut child, Duration::from_secs(30));
    assert_eq!(code, 0, "graceful shutdown on read-only root");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(deploy.path(), std::fs::Permissions::from_mode(0o755))
            .expect("restore writable for cleanup");
    }
    let mut entries: Vec<String> = std::fs::read_dir(deploy.path())
        .expect("read deploy dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec!["app.bin".to_string()],
        "no extraction or other writes: {entries:?}"
    );
}

/// A post-compile decoy route file placed beside the deployed artifact
/// is never loaded: only the indexed store routes execute.
#[test]
fn compiled_multidocument_ignores_post_compile_decoy() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().multi_route);

    // Post-compile decoy: a fresh route file beside the artifact.
    std::fs::create_dir_all(deploy.path().join("routes")).expect("mkdir decoy routes");
    std::fs::write(
        deploy.path().join("routes").join("decoy.yaml"),
        "routes:\n  - id: decoy\n    from: timer:tick?period=100\n    steps:\n      - set_body:\n          value: decoy-marker\n      - to: log:decoy\n",
    )
    .expect("write decoy route");

    let mut child = spawn_child(
        "compiled_multidocument_ignores_post_compile_decoy",
        deploy.path(),
        &artifact,
        &[],
        &[],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(&drained, "alpha-marker", Duration::from_secs(60)),
        "embedded entry route must execute: {}",
        drained.captured()
    );
    assert!(
        wait_for_marker(&drained, "beta-marker", Duration::from_secs(30)),
        "embedded indexed route must execute: {}",
        drained.captured()
    );
    // Give the decoy's faster timer a chance to fire, then prove it
    // never did.
    thread::sleep(Duration::from_millis(500));
    let all_output = format!(
        "{}{}",
        drained.out.lock().expect("stdout lock"),
        drained.err.lock().expect("stderr lock")
    );
    assert!(
        !all_output.contains("decoy-marker"),
        "the decoy route must never load: {all_output}"
    );
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(&mut child, Duration::from_secs(30));
    assert_eq!(code, 0, "graceful shutdown with decoy present");
}

/// A v1 single-document artifact still runs through the v1
/// single-entry adapter: the single-document embedded seam boots the
/// payload and the virtual-store runtime stays out of the picture.
#[test]
fn compiled_v1_artifact_uses_single_entry_adapter() {
    child_guard();
    let deploy = tempfile::tempdir().expect("deploy tempdir");

    // Hand-build a v1 artifact: the v1 trailer alone (the library seam
    // decodes from the file tail; no executable image is needed).
    let manifest =
        camel_cli::compile::manifest::derive("app.yaml", trailer::TrailerKind::Route, ROUTE_DOC)
            .expect("v1 manifest derives");
    let v1 = trailer::Trailer {
        kind: trailer::TrailerKind::Route,
        payload: ROUTE_DOC.as_bytes().to_vec(),
        manifest: manifest.to_legacy_json().into_bytes(),
    };
    let artifact = deploy.path().join("app.bin");
    std::fs::write(&artifact, trailer::encode(&v1)).expect("write v1 artifact");

    let mut child = spawn_child(
        "compiled_v1_artifact_uses_single_entry_adapter",
        deploy.path(),
        &artifact,
        &[],
        &[],
    );
    let drained = spawn_drained(&mut child);
    assert!(
        wait_for_marker(&drained, "context started", Duration::from_secs(60)),
        "v1 artifact must boot: {}",
        drained.captured()
    );
    let all_output = format!(
        "{}{}",
        drained.out.lock().expect("stdout lock"),
        drained.err.lock().expect("stderr lock")
    );
    assert!(
        all_output.contains("loading routes from compiled://app.yaml"),
        "the v1 single-document seam must serve the run: {all_output}"
    );
    assert!(
        !all_output.contains("virtual store"),
        "v1 artifacts keep the single-entry adapter path: {all_output}"
    );
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(&mut child, Duration::from_secs(30));
    assert_eq!(code, 0, "graceful shutdown on the v1 path");
}

/// An invalid decoded store fails closed before boot: a structurally
/// valid trailer whose embedded configuration document is not parseable
/// TOML exits 2 naming the configuration, with no route boot. The
/// interim "runtime not available" bridge message is gone — the real
/// virtual-store validation produces the diagnostic.
///
/// The same holds for checksum-consistent stores that violate an index
/// rule. Each child below hand-crafts a v2 artifact from a VALID store
/// with exactly one mutated index field and re-seals it through
/// `encode_v2` (the BLAKE3 over the `rust-camel-trailer-v2` domain is
/// recomputed), so the child's named failure is STORE VALIDATION — never
/// a checksum mismatch — and no route ever boots:
///
/// - unknown store schema (`store_schema` 99);
/// - missing reference (a source-plan reference to an absent entry);
/// - kind mismatch (a `job` entry point under a `route` trailer kind).
#[test]
fn compiled_runtime_rejects_invalid_store_before_boot() {
    child_guard();
    use camel_cli::compile::store::{
        StoreDocument, StoreEntryKind, StoreIndex, VirtualDocumentStore,
    };
    use camel_cli::compile::trailer::{TrailerKind, TrailerV2};

    let deploy = tempfile::tempdir().expect("deploy tempdir");
    let route_text = "routes:\n  - id: demo\n    from: timer:tick?period=300\n    steps:\n      - to: log:demo\n";
    // The store passes every structural invariant (schema, ranges,
    // references, kinds) but its configuration document is not TOML:
    // only the runtime's pre-boot store validation catches it.
    let store = VirtualDocumentStore::build(
        "app.yaml",
        &[
            StoreDocument {
                path: "app.yaml".to_string(),
                kind: StoreEntryKind::Route,
                bytes: route_text.as_bytes().to_vec(),
            },
            StoreDocument {
                path: "Camel.toml".to_string(),
                kind: StoreEntryKind::Config,
                bytes: b"this is not = = valid toml [[\n".to_vec(),
            },
        ],
        &["Camel.toml".to_string()],
        &["app.yaml".to_string()],
    )
    .expect("structurally valid store builds");
    let manifest = camel_cli::compile::manifest::derive_for_store(
        &store,
        TrailerKind::Route,
        &[("app.yaml".to_string(), route_text.to_string())],
    )
    .expect("manifest derives");
    let artifact_bytes = trailer::encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content.clone(),
        index: store.index.encode_canonical().expect("canonical index"),
        manifest: manifest.to_canonical_json().into_bytes(),
    });
    let artifact = deploy.path().join("invalid.bin");
    std::fs::write(&artifact, artifact_bytes).expect("write invalid artifact");

    let (code, stdout, stderr) = spawn_child_output(
        "compiled_runtime_rejects_invalid_store_before_boot",
        deploy.path(),
        &artifact,
        &[],
        &[],
    );
    let combined = format!("{stdout}{stderr}");
    assert_eq!(
        code, 2,
        "invalid store must fail closed with exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        combined.contains("Camel.toml"),
        "the diagnostic must name the malformed configuration: {combined}"
    );
    assert!(
        !combined.contains("not available in this build"),
        "the interim bridge message must be gone: {combined}"
    );
    assert!(
        !combined.contains("context started"),
        "no boot may happen: {combined}"
    );

    // A fully valid store as the base for the index-level corruptions:
    // every mutation below is the ONLY defect in an otherwise valid
    // artifact, so the named diagnostic is attributable to the store
    // rule it violates.
    let valid_store = VirtualDocumentStore::build(
        "app.yaml",
        &[
            StoreDocument {
                path: "app.yaml".to_string(),
                kind: StoreEntryKind::Route,
                bytes: route_text.as_bytes().to_vec(),
            },
            StoreDocument {
                path: "Camel.toml".to_string(),
                kind: StoreEntryKind::Config,
                bytes: b"[profiles.default]\n".to_vec(),
            },
        ],
        &["Camel.toml".to_string()],
        &["app.yaml".to_string()],
    )
    .expect("structurally valid store builds");
    let valid_manifest = camel_cli::compile::manifest::derive_for_store(
        &valid_store,
        TrailerKind::Route,
        &[("app.yaml".to_string(), route_text.to_string())],
    )
    .expect("manifest derives");

    // One index mutation per scenario.
    fn schema_99(index: &mut StoreIndex) {
        index.store_schema = 99;
    }
    fn missing_plan_reference(index: &mut StoreIndex) {
        index
            .source_plan
            .references
            .push("routes/ghost.yaml".to_string());
    }
    fn job_entry_point(index: &mut StoreIndex) {
        for entry in &mut index.entries {
            if entry.path == index.entry_point {
                entry.kind = StoreEntryKind::Job;
            }
        }
    }

    for (label, diagnostic, mutate) in [
        (
            "unknown-store-schema",
            "unsupported store schema 99",
            schema_99 as fn(&mut StoreIndex),
        ),
        (
            "missing-plan-reference",
            "store reference to missing entry \"routes/ghost.yaml\"",
            missing_plan_reference,
        ),
        (
            "entry-point-kind-mismatch",
            "store reference \"app.yaml\" names a job entry, expected route",
            job_entry_point,
        ),
    ] {
        let mut index = valid_store.index.clone();
        mutate(&mut index);
        // `encode_v2` re-seals the footer checksum over the
        // rust-camel-trailer-v2 domain: the child must fail on STORE
        // validation, never on a checksum mismatch.
        let artifact_bytes = trailer::encode_v2(&TrailerV2 {
            kind: TrailerKind::Route,
            content: valid_store.content.clone(),
            index: index.encode_canonical().expect("mutated index encodes"),
            manifest: valid_manifest.to_canonical_json().into_bytes(),
        });
        let artifact = deploy.path().join(format!("{label}.bin"));
        std::fs::write(&artifact, artifact_bytes).expect("write corrupted artifact");

        let (code, stdout, stderr) = spawn_child_output(
            "compiled_runtime_rejects_invalid_store_before_boot",
            deploy.path(),
            &artifact,
            &[],
            &[],
        );
        let combined = format!("{stdout}{stderr}");
        assert_eq!(
            code, 2,
            "{label} must fail closed with exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            combined.contains(diagnostic),
            "{label} must name the store failure: {combined}"
        );
        assert!(
            !combined.contains("checksum mismatch"),
            "{label} must not fail on integrity (the artifact is re-sealed): {combined}"
        );
        assert!(
            !combined.contains("context started"),
            "{label} must boot zero routes: {combined}"
        );
    }
}
