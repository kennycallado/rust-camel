//! Integration tests for compiled-artifact runtime (openspec change
//! `cli-compile`, Tasks 2.2 and 2.3). The suite compiles each distinct
//! document ONCE with the real `camel compile` into a shared immutable
//! fixture — the compile step copies the full ~287 MB `camel` binary into
//! every artifact, so per-test compiles would write gigabytes under
//! parallel execution and exhaust the disk (ENOSPC). The fixture and the
//! per-test deploy directories live on a space-probed [`fixture_root`]:
//! the OS temp directory when it holds the ~3.7 GiB the suite writes,
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

use std::net::{TcpListener, TcpStream};
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

/// The multi-entry chain job document (r3jobs Task 2.1): `routeFiles`
/// names all four route files of the B → B.C1 → B.C1.C2 → B.C1.C2.A
/// direct-endpoint chain explicitly, and the one-shot send fires the
/// first link with `capture-reply` so the reply proves every entry ran.
const MULTI_N_JOB_DOC: &str = "\
routeFiles:
  - routes/b.yaml
  - routes/c1.yaml
  - routes/c2.yaml
  - routes/a.yaml
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: ping
";

/// Chain link 1: consumes the job's send endpoint and forwards on.
const MULTI_N_ROUTE_B: &str = "\
routes:
  - id: chain-b
    from: direct:transform
    steps:
      - set_body:
          value: B
      - to: direct:step-c1
";

/// Chain link 2.
const MULTI_N_ROUTE_C1: &str = "\
routes:
  - id: chain-c1
    from: direct:step-c1
    steps:
      - set_body:
          value: B.C1
      - to: direct:step-c2
";

/// Chain link 3.
const MULTI_N_ROUTE_C2: &str = "\
routes:
  - id: chain-c2
    from: direct:step-c2
    steps:
      - set_body:
          value: B.C1.C2
      - to: direct:final
";

/// Chain link 4: terminal consumer; its body IS the reply.
const MULTI_N_ROUTE_A: &str = "\
routes:
  - id: chain-a
    from: direct:final
    steps:
      - set_body:
          value: B.C1.C2.A
";

/// r3jobs Task 2.2: a multi-entry job document in the same shape as
/// `MULTI_N_JOB_DOC`, but naming THREE route files: two valid chain
/// links plus `routes/bad.yaml`, which is well-declared (a `direct:`
/// consumer with a `steps:` list) yet structurally invalid (its only
/// step key, `totally_not_a_step:`, matches no `RouteDslStep` variant).
/// Compilation must accept it — declaration checks only; boot must
/// reject it, naming the entry.
const MULTI_BAD_JOB_DOC: &str = "\
routeFiles:
  - routes/b.yaml
  - routes/bad.yaml
  - routes/c1.yaml
execute:
  mode: one-shot
  timeout: 60s
  capture-reply: true
  send:
    to: direct:transform
    body: ping
";

/// The structurally invalid chain link: an unknown step key under a
/// well-formed `direct:` consumer.
const MULTI_BAD_ROUTE: &str = "\
routes:
  - id: chain-bad
    from: direct:step-c2
    steps:
      - totally_not_a_step:
          value: nope
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

/// r4sign Task 1.3: compile with the signing flags — `--sign` with
/// `--signing-key <seed>` plus `--require-signature` when `require` is
/// set. `seed` is a runtime-written synthetic 32-byte key file.
fn compile_signed(dir: &Path, doc: &str, artifact: &str, seed: &Path, require: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    cmd.env_clear()
        .current_dir(dir)
        .args(["compile", doc, "-o", artifact, "--sign", "--signing-key"])
        .arg(seed);
    if require {
        cmd.arg("--require-signature");
    }
    cmd.output().expect("spawn `camel compile --sign`")
}

/// The detached signature envelope path of `artifact`: the artifact
/// path with `.sig` appended — the same lookup rule the runtime uses.
fn sig_path_of(artifact: &Path) -> PathBuf {
    let mut name = artifact.as_os_str().to_owned();
    name.push(".sig");
    PathBuf::from(name)
}

/// The synthetic fixture signing seed (r4sign Task 1.3): 32 pattern
/// bytes written to a runtime key file — obviously non-secret, never
/// committed key material, never real material.
const FIXTURE_SEED: [u8; 32] = *b"r4sign-fixture-seed-00000000000\0";

/// Distinct documents compiled once per test process. `camel compile`
/// copies the full ~287 MB `camel` binary into every artifact, so
/// compiling per test would write gigabytes under parallel execution and
/// exhaust the disk (ENOSPC). The fixture compiles each document exactly
/// once — serialized by the `OnceLock` — and tests share the immutable
/// artifacts; only mutation tests copy (see [`deploy_artifact`]).
///
/// The artifacts live in a single cache directory keyed by this test
/// process (`camel-compiled-fixture-<pid>` under the space-probed
/// [`fixture_root`], the repo's `camel-test-*` convention): thirteen
/// artifacts at the current binary size total ~3.7 GiB, so the root
/// needs [`REQUIRED_ROOT_FREE`] free before the suite starts — the OS
/// temp directory when it has room (CI, unchanged), the workspace
/// target directory as fallback on small-`/tmp` dev machines. A
/// detached reaper child removes that directory once this process dies
/// — normal exit or crash — and the next run sweeps any leftover, so
/// repeated runs never accumulate the ~3.7 GiB of compiled artifacts.
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
    /// Multi-entry chain job artifact (config, job document naming four
    /// route files, the four chained route sources).
    multi_job_n: PathBuf,
    /// Multi-entry job artifact whose route set contains a structurally
    /// invalid entry (`routes/bad.yaml`): compiles clean, boots with
    /// exit 2 naming the entry.
    multi_job_bad: PathBuf,
    /// `TYPED_DEFAULT_ARG_DOC` artifact (typed default coerces at
    /// startup, jobtyped Task 5).
    typed_arg: PathBuf,
    /// r4sign Task 1.3: `JOB_DOC` artifact compiled with `--sign` and
    /// the runtime-written synthetic seed; the detached envelope sits
    /// at `<signed_job>.sig`.
    signed_job: PathBuf,
    /// r4sign Task 1.3: `JOB_DOC` artifact compiled with `--sign
    /// --require-signature`; its manifest marks the signature required.
    signed_required_job: PathBuf,
    /// Wall-clock unix seconds captured immediately before the first
    /// signed twin compiles: a schema-5 freshness marker must be >= this
    /// (the marker is written by THIS suite's compile, never a stale
    /// cached artifact; the upper bound is the test's own `now`, bd
    /// rc-xkdbe).
    signed_compile_started_at: u64,
}

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

/// Free space the fixture root must have before the suite starts
/// compiling: fifteen artifact writes at the current ~287 MB binary
/// (~4.3 GiB — the fourteen fixture compiles including the two r4sign
/// signed twins, plus the accepted loose compile) plus the transient
/// whole-artifact copies (the mutation tests hold up to one copy each
/// in parallel). Bump this when the suite gains artifacts or the binary
/// grows past what the headroom covers.
#[cfg_attr(not(unix), allow(dead_code))]
const REQUIRED_ROOT_FREE: u64 = 8 << 30;

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
/// directory on machines whose `/tmp` is too small for the ~3.7 GiB
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
                             routes: &[(&str, &str)],
                             artifact: &str,
                             envs: &[(&str, &str)]|
         -> PathBuf {
            let root = dir.join(subdir);
            std::fs::create_dir_all(root.join("conf")).expect("mkdir conf");
            std::fs::create_dir_all(root.join("routes")).expect("mkdir routes");
            std::fs::write(root.join("Camel.toml"), config).expect("write config");
            std::fs::write(root.join("conf").join("base.toml"), MULTI_INCLUDE)
                .expect("write include");
            for (route_path, route_doc) in routes {
                std::fs::write(root.join(route_path), route_doc).expect("write indexed route");
            }
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
            &[("routes/beta.yaml", MULTI_INDEXED_ROUTE)],
            "multi-route.bin",
            &[],
        );
        let multi_job = compile_multi(
            "multi-job",
            MULTI_JOB_CONFIG,
            "ingest-m.job.yaml",
            MULTI_JOB_DOC,
            &[("routes/transform.yaml", MULTI_JOB_ROUTE)],
            "multi-job.bin",
            &[],
        );
        let multi_env = compile_multi(
            "multi-env",
            MULTI_JOB_CONFIG,
            "greet-m.job.yaml",
            MULTI_ENV_JOB_DOC,
            &[("routes/greet.yaml", MULTI_ENV_ROUTE)],
            "multi-env.bin",
            &[("DEPLOY_GREETING", "compile-secret-value")],
        );
        // Multi-entry chain job: the job document names FOUR route
        // files, so the helper writes a whole (path, document) list of
        // route sources.
        let multi_job_n = compile_multi(
            "multi-job-n",
            MULTI_JOB_CONFIG,
            "ingest-n.job.yaml",
            MULTI_N_JOB_DOC,
            &[
                ("routes/b.yaml", MULTI_N_ROUTE_B),
                ("routes/c1.yaml", MULTI_N_ROUTE_C1),
                ("routes/c2.yaml", MULTI_N_ROUTE_C2),
                ("routes/a.yaml", MULTI_N_ROUTE_A),
            ],
            "multi-job-n.bin",
            &[],
        );
        // Failing-entry variant: same shape, but one route file is
        // structurally invalid — compile must still accept it.
        let multi_job_bad = compile_multi(
            "multi-job-bad",
            MULTI_JOB_CONFIG,
            "ingest-bad.job.yaml",
            MULTI_BAD_JOB_DOC,
            &[
                ("routes/b.yaml", MULTI_N_ROUTE_B),
                ("routes/bad.yaml", MULTI_BAD_ROUTE),
                ("routes/c1.yaml", MULTI_N_ROUTE_C1),
            ],
            "multi-job-bad.bin",
            &[],
        );
        let typed_arg = compile_one("typed.job.yaml", TYPED_DEFAULT_ARG_DOC, "typed.bin", &[]);
        // r4sign Task 1.3: the signed twins. The synthetic seed file is
        // written at runtime and both compiles emit the detached
        // 148-byte envelope beside the artifact.
        let signed_compile_started_at = unix_now();
        let seed_path = dir.join("fixture-signing.key");
        std::fs::write(&seed_path, FIXTURE_SEED).expect("write synthetic signing seed");
        let signed_job = {
            std::fs::write(dir.join("signed.job.yaml"), JOB_DOC).expect("write document");
            let output =
                compile_signed(&dir, "signed.job.yaml", "signed-job.bin", &seed_path, false);
            assert_eq!(
                output.status.code(),
                Some(0),
                "signed compile must succeed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let sig = sig_path_of(&dir.join("signed-job.bin"));
            assert_eq!(
                std::fs::metadata(&sig).expect("envelope exists").len(),
                148,
                "the detached envelope is exactly 148 bytes"
            );
            dir.join("signed-job.bin")
        };
        let signed_required_job = {
            std::fs::write(dir.join("signed-req.job.yaml"), JOB_DOC).expect("write document");
            let output = compile_signed(
                &dir,
                "signed-req.job.yaml",
                "signed-req.bin",
                &seed_path,
                true,
            );
            assert_eq!(
                output.status.code(),
                Some(0),
                "signed --require-signature compile must succeed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(sig_path_of(&dir.join("signed-req.bin")).is_file());
            dir.join("signed-req.bin")
        };
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
            multi_job_n,
            multi_job_bad,
            typed_arg,
            signed_job,
            signed_required_job,
            signed_compile_started_at,
        }
    })
}

/// Wall-clock unix seconds now; the fixture's signed-compile window and
/// the freshness assertions share one clock reading helper.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after the unix epoch")
        .as_secs()
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

/// Deploy a signed fixture artifact WITH its detached envelope (r4sign
/// Task 1.3): the artifact hardlinks zero-copy exactly like
/// [`deploy_artifact`] and the 148-byte envelope is COPIED beside it
/// under the runtime's `<artifact>.sig` lookup rule. The envelope copy
/// is mutable: tests that alter the envelope overwrite their own copy,
/// never the shared fixture.
fn deploy_signed(artifact: &Path) -> (tempfile::TempDir, PathBuf) {
    let (deploy, target) = deploy_artifact(artifact);
    std::fs::copy(sig_path_of(artifact), sig_path_of(&target)).expect("copy envelope");
    (deploy, target)
}

/// Tamper-on-COPY discipline (r4sign Task 1.3): copy the deployed
/// artifact to `name` with one byte flipped at `offset`, copy the
/// envelope beside it, and make the copy executable. The shared fixture
/// and its hardlinked deploys are never mutated.
fn deploy_flipped_copy(
    deploy: &tempfile::TempDir,
    artifact: &Path,
    name: &str,
    offset: usize,
) -> PathBuf {
    let mut bytes = std::fs::read(artifact).expect("read artifact bytes");
    assert!(offset < bytes.len(), "flip offset inside the artifact");
    bytes[offset] ^= 0xFF;
    let path = deploy.path().join(name);
    std::fs::write(&path, bytes).expect("write tampered copy");
    #[cfg(unix)]
    make_executable(&path);
    std::fs::copy(sig_path_of(artifact), sig_path_of(&path)).expect("copy envelope to the copy");
    path
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

/// Drop every `CAMEL_*` variable the parent carries from `cmd`'s
/// environment (r5routesrv Task 1.1): the fleet dev shell exports
/// bridge-path overrides (`CAMEL_CXF_BRIDGE_BINARY_PATH`,
/// `CAMEL_XML_BRIDGE_BINARY_PATH`, `CAMEL_JMS_BRIDGE_BINARY_PATH`) and
/// `camel compile v2` fails closed on any `CAMEL_*` presence. The
/// compile invocations already build their commands with `env_clear()`;
/// the harness-child spawns inherit the parent environment, so they
/// must drop the keys explicitly. [`CHILD_ENV`] itself is exempt — it
/// names the artifact for the child branch and is set right after.
fn scrub_camel_env(cmd: &mut Command) {
    let camel_keys: Vec<std::ffi::OsString> = std::env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| {
            let name = key.to_string_lossy();
            name.starts_with("CAMEL_") && name != CHILD_ENV
        })
        .collect();
    for key in camel_keys {
        cmd.env_remove(key);
    }
}

/// Anti-forkbomb invariant (bd rc-yhnq0): only a PARENT test process
/// may spawn a harness child. A child reaching a spawn helper means the
/// test forgot `child_guard()` — fail this one test loudly instead of
/// self-spawning without bound (bd rc-wvydl killed the host five times).
fn assert_not_harness_child() {
    assert!(
        std::env::var_os(CHILD_ENV).is_none(),
        "harness child re-entered a spawn helper: the enclosing test forgot child_guard() \
         (bd rc-yhnq0)"
    );
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
    assert_not_harness_child();
    let mut cmd = Command::new(std::env::current_exe().expect("current test exe"));
    scrub_camel_env(&mut cmd);
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
    assert_not_harness_child();
    let mut cmd = Command::new(std::env::current_exe().expect("current test exe"));
    scrub_camel_env(&mut cmd);
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

/// `--manifest` on a v2 virtual-store artifact prints the schema-3
/// canonical manifest — `manifest_schema` 3, the runtime version, and
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
    assert_eq!(manifest["manifest_schema"], 3, "manifest: {manifest}");
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

// ── r5routesrv: long-running route-server artifact batteries ─────────
//
// A compiled ROUTE artifact runs in the deployment posture of
// `camel run --no-watch`: it binds every listener its embedded
// documents declare, serves until the first stop signal, drains
// in-flight listener work inside the configured drain budget, and
// exits 0 (a second signal during teardown force-exits 1 — Task 1.4,
// not covered here). The tests below pin that contract end to end on
// the REST transport, the canonical listener the manifest scanner
// itself documents.

/// The exact completed-route report JSON: compact, key order
/// `kind`,`status`,`error` (the `RouteReport` wire shape).
const COMPLETED_ROUTE_REPORT: &str = r#"{"kind":"route","status":"completed","error":null}"#;

/// The Linux bind-failure text: the loud, retriable signature of the
/// ADR-0070 port-probe race (another process grabbed the probed port
/// between the listener drop and the server's bind).
const BIND_RACE_MARK: &str = "Address already in use";

/// The exact test named by the harness-child spawn of
/// [`serve_listener_flow`]: the flow re-enters this binary through
/// [`spawn_child`], whose child branch is selected by `--exact <test>`.
const SERVE_TEST: &str = "route_server_serves_listener_until_sigterm";

/// The exact test named by the harness-child spawn of
/// [`grpc_serve_flow`] (same re-entry mechanism as [`SERVE_TEST`]).
const GRPC_SERVE_TEST: &str = "route_server_serves_grpc_listener_until_sigterm";

/// Probe a free localhost port by binding `127.0.0.1:0`, reading the
/// assigned port, and dropping the listener. ADR-0070 SUBPROCESS
/// EXCEPTION: the spawned artifact cannot receive an in-process staged
/// listener, so the port is released before the child binds — the same
/// pattern as `job_coexistence_test::reserve_two_ports`, whose module
/// header documents the exception. The port-toctou window is closed by
/// the [`BIND_RACE_MARK`] retry convention (see
/// [`with_bind_race_retry`]), not by staging.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let port = listener.local_addr().expect("probe addr").port();
    drop(listener);
    port
}

/// One raw HTTP/1.1 GET over a fresh TCP connection: a 5 s read
/// timeout, `Connection: close`, read to EOF, the status code parsed
/// from the status line, and the full body String returned; `None` on
/// any connect/read failure.
fn http_get(port: u16, path: &str) -> Option<(u16, String)> {
    use std::io::{Read, Write};
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).ok()?;
    let response = String::from_utf8_lossy(&bytes).into_owned();
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())?;
    let body = match response.split_once("\r\n\r\n") {
        Some((_, body)) => body.to_string(),
        None => response,
    };
    Some((status, body))
}

/// The probe-validated REST listener document: a `direct:ping` back
/// route answering `pong` plus a `rest:` block exposing `GET /ping`
/// under the base path `/api` — REQUIRED, the DSL rejects an empty
/// rest base path — bound on `127.0.0.1:port`.
fn rest_listener_doc(port: u16) -> String {
    format!(
        "\
routes:
  - id: ping-route
    from: direct:ping
    steps:
      - set_body: \"pong\"
rest:
  - host: 127.0.0.1
    port: {port}
    path: /api
    operations:
      - method: GET
        path: /ping
        to: direct:ping
"
    )
}

/// Run one full listener flow (port pick → doc write → compile →
/// deploy → spawn → assertions), retrying the ENTIRE flow exactly once
/// when its failure text carries [`BIND_RACE_MARK`]: the race window
/// is millisecond-scale and a second collision is not observed in
/// practice. Any other failure fails the test immediately. Copied
/// convention from `job_coexistence_test`.
fn with_bind_race_retry(flow: impl Fn() -> Result<(), String>) {
    match flow() {
        Ok(()) => {}
        Err(e) if e.contains(BIND_RACE_MARK) => {
            if let Err(retry) = flow() {
                panic!("listener flow failed on retry after bind race: {retry}");
            }
        }
        Err(e) => panic!("listener flow failed: {e}"),
    }
}

/// One per-flow compile+deploy of a MULTI-file source tree: write each
/// `(name, content)` pair into a fresh source tempdir on the fixture
/// root (`create_dir_all` on each entry's parent first, so nested
/// assets like `protos/*.proto` land where the document's relative
/// references expect them), `camel compile` the FIRST pair's name as
/// the entry document (the compile commands build with `env_clear()`,
/// so no `CAMEL_*` override can leak in), and deploy the artifact into
/// a fresh source-free directory. The compile is per flow because the
/// listener port is baked into the embedded document — a fresh port
/// per flow is the point. `Err` carries the failure text (retried on
/// the bind race by [`with_bind_race_retry`]).
fn compile_and_deploy_listener_files(
    files: &[(&str, &str)],
) -> Result<(tempfile::TempDir, PathBuf), String> {
    let src = tempfile::Builder::new()
        .prefix("camel-routesrv-src-")
        .tempdir_in(fixture_root())
        .map_err(|e| format!("source tempdir: {e}"))?;
    for (name, content) in files {
        if let Some(parent) = std::path::Path::new(name).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(src.path().join(parent))
                .map_err(|e| format!("create dir {name}: {e}"))?;
        }
        std::fs::write(src.path().join(name), content).map_err(|e| format!("write {name}: {e}"))?;
    }
    let (doc_name, _) = files
        .first()
        .ok_or_else(|| "at least one source file is required".to_string())?;
    let output = compile(src.path(), doc_name, "rest.bin", &[]);
    if output.status.code() != Some(0) {
        return Err(format!(
            "entry document must compile: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(deploy_artifact(&src.path().join("rest.bin")))
}

/// Single-document convenience wrapper over
/// [`compile_and_deploy_listener_files`]: the REST batteries all deploy
/// exactly one `rest.yaml`, so their call sites stay one-argument clean.
fn compile_and_deploy_listener(doc: &str) -> Result<(tempfile::TempDir, PathBuf), String> {
    compile_and_deploy_listener_files(&[("rest.yaml", doc)])
}

/// One full serve flow: fresh port → doc → compile → deploy → spawn
/// with `--report` → boot → HTTP 200 `pong` on the listener → SIGTERM
/// → exit 0 → the exact completed report → a `--manifest` run listing
/// `127.0.0.1:<port>` with `artifact_kind` `server`. `Err` carries the
/// failure text; the [`BIND_RACE_MARK`] signature inside it makes the
/// caller retry the whole flow once (see [`with_bind_race_retry`]).
fn serve_listener_flow() -> Result<(), String> {
    let port = free_port();
    let (deploy, artifact) = compile_and_deploy_listener(&rest_listener_doc(port))?;
    let mut child = spawn_child(
        SERVE_TEST,
        deploy.path(),
        &artifact,
        &["--report", "report.json"],
        &[],
    );
    let drained = spawn_drained(&mut child);
    if !wait_for_marker(&drained, "context started", Duration::from_secs(60)) {
        let captured = drained.captured();
        return Err(format!("artifact never booted:\n{captured}"));
    }
    match http_get(port, "/api/ping") {
        Some((200, body)) if body.contains("pong") => {}
        other => {
            return Err(format!(
                "listener must answer 200 with a `pong` body before the signal, got {other:?}"
            ));
        }
    }
    // Liveness probe: after a successful serve the process must still
    // be running — the artifact serves while alive and must not
    // self-exit after boot (bounded self-exit is the job kind's
    // contract, not the server's).
    assert!(
        child
            .0
            .try_wait()
            .expect("child must be pollable")
            .is_none(),
        "server artifact must not self-exit after boot; it serves until signaled:\n{}",
        drained.captured()
    );
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(&mut child, Duration::from_secs(30));
    if code != 0 {
        return Err(format!(
            "SIGTERM must shut down the serving artifact gracefully (exit 0), got {code}:\n{}",
            drained.captured()
        ));
    }
    let report = std::fs::read_to_string(deploy.path().join("report.json"))
        .map_err(|e| format!("route report must be written: {e}"))?;
    if report.trim() != COMPLETED_ROUTE_REPORT {
        return Err(format!(
            "report must be exactly {COMPLETED_ROUTE_REPORT}, got {report}"
        ));
    }
    // The manifest check (same deployed artifact, `--manifest` never
    // boots, so no `CAMEL_*` env scrub is needed): run it through the
    // canonical harness helper.
    let (mcode, mstdout, mstderr) =
        common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    if mcode != 0 {
        return Err(format!(
            "--manifest must exit 0;\nstdout:\n{mstdout}\nstderr:\n{mstderr}"
        ));
    }
    let manifest: serde_json::Value = serde_json::from_str(mstdout.trim())
        .map_err(|e| format!("--manifest stdout is not JSON ({e}):\n{mstdout}"))?;
    let listener = format!("127.0.0.1:{port}");
    let listeners: Vec<String> = manifest["listeners"]
        .as_array()
        .ok_or_else(|| format!("manifest carries a listeners array: {manifest}"))?
        .iter()
        .map(|v| v.as_str().expect("listener string").to_string())
        .collect();
    if !listeners.contains(&listener) {
        return Err(format!("manifest must list {listener}: {listeners:?}"));
    }
    if manifest["artifact_kind"] != "server" {
        return Err(format!(
            "manifest artifact_kind must be `server`: {manifest}"
        ));
    }
    Ok(())
}

/// The compiled route artifact serves its declared REST listener until
/// SIGTERM: HTTP 200 with body `pong` after `context started`, graceful
/// exit 0 on `kill -TERM` (30 s bound), the report file exactly
/// `{"kind":"route","status":"completed","error":null}`, and a
/// `--manifest` run listing `127.0.0.1:<port>` with `artifact_kind`
/// `server` (openspec r5routesrv, cli-compile "Route artifact serves
/// its declared listener until SIGTERM").
#[test]
fn route_server_serves_listener_until_sigterm() {
    child_guard();
    with_bind_race_retry(serve_listener_flow);
}

// ── r5batteries: gRPC serve battery ─────────────────────────────────
//
// The same sealed-artifact serve contract as [`serve_listener_flow`],
// pinned on the gRPC transport (bd rc-z332y): the `protoFile`
// descriptor must embed with the artifact (r5batteries Task 1.2), the
// consumer must resolve its descriptors and bind the h2 listener
// BEFORE declaring readiness, and SIGTERM must still exit 0 with the
// canonical completed-route report.

/// The verbatim `examples/grpc-example/protos/helloworld.proto` text:
/// the embedded descriptor is the REAL example proto, not a trimmed
/// fixture, so the battery pins exactly what the documented example
/// ships (a service plus two messages — enough for descriptor
/// resolution to fail loudly if embedding loses a byte).
fn hello_world_proto() -> &'static str {
    "\
syntax = \"proto3\";
package helloworld;

service Greeter {
  rpc SayHello (HelloRequest) returns (HelloReply) {}
}

message HelloRequest {
  string name = 1;
}

message HelloReply {
  string message = 1;
}
"
}

/// The gRPC listener document: a single `grpc://` consumer route bound
/// on `127.0.0.1:port` for `helloworld.Greeter/SayHello`, reading its
/// descriptor through the same RELATIVE `protoFile` reference the
/// example ships (the sealed artifact materializes it to an absolute
/// path at boot) and running `transport=plaintext` (TLS is a separate
/// battery's concern).
fn grpc_listener_doc(port: u16) -> String {
    format!(
        "\
routes:
  - id: grpc-serve
    from: grpc://127.0.0.1:{port}/helloworld.Greeter/SayHello?protoFile=protos/helloworld.proto&transport=plaintext
    steps:
      - log: \"grpc-request\"
"
    )
}

/// One wire-level HTTP/2 readiness probe: connect raw TCP, send the
/// h2 client preface plus an empty SETTINGS frame, and require the
/// server's FIRST frame to be a SETTINGS (type 0x04). Wire-level on
/// purpose — an HTTP/1-only listener or a plain TCP acceptor would
/// fail here — and `None` on any connect/read failure, mirroring
/// [`http_get`]'s `.ok()?` error style.
fn h2_settings_probe(port: u16) -> Option<()> {
    use std::io::{Read, Write};
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    // The 24-byte client connection preface (RFC 9113 §3.4)...
    let preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    // ...followed by a 9-byte EMPTY SETTINGS frame header: length 0,
    // type 0x04 (SETTINGS), flags 0, stream id 0.
    let settings = [0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00];
    stream.write_all(preface).ok()?;
    stream.write_all(&settings).ok()?;
    let mut header = [0u8; 9];
    stream.read_exact(&mut header).ok()?;
    (header[3] == 0x04).then_some(())
}

/// One full gRPC serve flow: fresh port → doc + proto → compile →
/// deploy → spawn with `--report` → boot → the grpc readiness marker
/// (logged only after the registry binds AND the descriptors resolve)
/// → a wire-level h2 SETTINGS exchange on the listener → liveness →
/// SIGTERM → exit 0 → the exact completed report → a `--manifest` run
/// reporting `artifact_kind` `server` with `grpc` among the embedded
/// components. `Err` carries the failure text; the [`BIND_RACE_MARK`]
/// signature inside it makes the caller retry the whole flow once (see
/// [`with_bind_race_retry`]).
fn grpc_serve_flow() -> Result<(), String> {
    let port = free_port();
    let (deploy, artifact) = compile_and_deploy_listener_files(&[
        ("doc.yaml", &grpc_listener_doc(port)),
        ("protos/helloworld.proto", hello_world_proto()),
    ])?;
    let mut child = spawn_child(
        GRPC_SERVE_TEST,
        deploy.path(),
        &artifact,
        &["--report", "grpc-report.json"],
        &[],
    );
    let drained = spawn_drained(&mut child);
    if !wait_for_marker(&drained, "context started", Duration::from_secs(60)) {
        let captured = drained.captured();
        return Err(format!("artifact never booted:\n{captured}"));
    }
    if !wait_for_marker(
        &drained,
        "grpc consumer started, waiting for requests",
        Duration::from_secs(20),
    ) {
        let captured = drained.captured();
        return Err(format!(
            "grpc consumer never became ready (registry bind + descriptor resolution must \
             precede the marker):\n{captured}"
        ));
    }
    if h2_settings_probe(port).is_none() {
        return Err(format!(
            "grpc listener must answer the h2 preface with a SETTINGS frame:\n{}",
            drained.captured()
        ));
    }
    // Liveness probe: after a successful handshake the process must
    // still be running — the artifact serves while alive and must not
    // self-exit after boot (bounded self-exit is the job kind's
    // contract, not the server's).
    if child
        .0
        .try_wait()
        .expect("child must be pollable")
        .is_some()
    {
        return Err(format!(
            "artifact must stay alive while serving:\n{}",
            drained.captured()
        ));
    }
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(&mut child, Duration::from_secs(30));
    if code != 0 {
        return Err(format!(
            "SIGTERM must shut down the serving artifact gracefully (exit 0), got {code}:\n{}",
            drained.captured()
        ));
    }
    let report = std::fs::read_to_string(deploy.path().join("grpc-report.json"))
        .map_err(|e| format!("route report must be written: {e}"))?;
    if report.trim() != COMPLETED_ROUTE_REPORT {
        return Err(format!(
            "report must be exactly {COMPLETED_ROUTE_REPORT}, got {report}"
        ));
    }
    // The manifest check (same deployed artifact, `--manifest` never
    // boots, so no `CAMEL_*` env scrub is needed): run it through the
    // canonical harness helper.
    let (mcode, mstdout, mstderr) =
        common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    if mcode != 0 {
        return Err(format!(
            "--manifest must exit 0;\nstdout:\n{mstdout}\nstderr:\n{mstderr}"
        ));
    }
    let manifest: serde_json::Value = serde_json::from_str(mstdout.trim())
        .map_err(|e| format!("--manifest stdout is not JSON ({e}):\n{mstdout}"))?;
    if manifest["artifact_kind"] != "server" {
        return Err(format!(
            "manifest artifact_kind must be `server`: {manifest}"
        ));
    }
    let grpc_listed = manifest["components"]
        .as_array()
        .is_some_and(|c| c.iter().any(|s| s.as_str() == Some("grpc")));
    if !grpc_listed {
        return Err(format!(
            "manifest must list grpc among the embedded components: {manifest}"
        ));
    }
    Ok(())
}

/// The compiled route artifact serves its declared gRPC listener until
/// SIGTERM (bd rc-z332y, spec scenario "gRPC consumer serves from a
/// sealed artifact until SIGTERM"): the grpc readiness marker after
/// `context started`, a wire-level h2 SETTINGS exchange on the
/// listener, graceful exit 0 on `kill -TERM` (30 s bound), the report
/// file exactly `{"kind":"route","status":"completed","error":null}`,
/// and a `--manifest` run reporting `artifact_kind` `server` with
/// `grpc` among the components.
#[test]
fn route_server_serves_grpc_listener_until_sigterm() {
    child_guard();
    with_bind_race_retry(grpc_serve_flow);
}

/// The exact test named by the harness-child spawn of
/// [`drain_inflight_flow`] (same re-entry mechanism as [`SERVE_TEST`]).
const DRAIN_TEST: &str = "route_server_drains_inflight_request";

/// The exact test named by the harness-child spawn of
/// [`ws_drain_flow`] (same re-entry mechanism as [`SERVE_TEST`]).
const WS_DRAIN_TEST: &str = "route_server_drains_inflight_ws_exchange";

/// The slow listener document: the same shape as [`rest_listener_doc`]
/// but the back route is `direct:slow` with the steps `log:
/// "slow-enter"` (the observable request-entry marker), `delay` of
/// `delay_ms`, then `set_body: "slow-pong"`; the operation is
/// `GET /slow` under the same `/api` base, targeting `direct:slow`.
fn slow_rest_doc(port: u16, delay_ms: u64) -> String {
    format!(
        "\
routes:
  - id: slow-route
    from: direct:slow
    steps:
      - log: \"slow-enter\"
      - delay: {delay_ms}
      - set_body: \"slow-pong\"
rest:
  - host: 127.0.0.1
    port: {port}
    path: /api
    operations:
      - method: GET
        path: /slow
        to: direct:slow
"
    )
}

/// Poll the captured buffers for `marker` with a 5 ms step (vs the
/// 20 ms of [`wait_for_marker`]): the signal tests must observe a boot
/// or request marker while the window is still open, so marker-
/// observation staleness has to stay well under the boot stretch or
/// the request stretch, not just under the process lifetime. Same
/// contract as `run_signal_test::wait_for_marker_tight`. Returns
/// `false` when the child dies on its own or the deadline elapses.
fn wait_for_marker_tight(
    child: &mut KillOnDrop,
    drained: &Drained,
    marker: &str,
    timeout: Duration,
) -> bool {
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
        if let Ok(Some(_)) = child.0.try_wait() {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// One full drain flow: fresh port → slow doc (3 s delay route) →
/// compile → deploy → spawn with `--report` → boot → GET `/api/slow`
/// on a thread → `slow-enter` observed (the request is PROVABLY inside
/// the delayed step) → SIGTERM mid-delay → the in-flight response
/// still completes inside the drain budget → exit 0 → the exact
/// completed report. `Err` carries the failure text; the
/// [`BIND_RACE_MARK`] signature inside it makes the caller retry the
/// whole flow once (see [`with_bind_race_retry`]).
fn drain_inflight_flow() -> Result<(), String> {
    let port = free_port();
    let (deploy, artifact) = compile_and_deploy_listener(&slow_rest_doc(port, 3000))?;
    let mut child = spawn_child(
        DRAIN_TEST,
        deploy.path(),
        &artifact,
        &["--report", "drain.json"],
        &[],
    );
    let drained = spawn_drained(&mut child);
    if !wait_for_marker(&drained, "context started", Duration::from_secs(60)) {
        let captured = drained.captured();
        return Err(format!("artifact never booted:\n{captured}"));
    }
    // Fire the request on a thread; the flow then waits for the
    // request-entry log, so the signal below lands while the exchange
    // is inside the delayed step, not merely queued at the listener.
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(http_get(port, "/api/slow"));
    });
    if !wait_for_marker_tight(&mut child, &drained, "slow-enter", Duration::from_secs(20)) {
        let captured = drained.captured();
        return Err(format!(
            "request never reached the delayed step (missing `slow-enter`):\n{captured}"
        ));
    }
    // SIGTERM lands immediately after the `slow-enter` marker, i.e.
    // ≈0 s into the 3 s delay step, so the full 3 s still fit inside
    // the 10 s default drain budget (`default_drain_timeout_ms` in
    // camel-config); the response must still be delivered, then exit 0.
    send_signal(&child.0, "-TERM");
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Some((200, body))) if body.contains("slow-pong") => {}
        Ok(other) => {
            return Err(format!(
                "in-flight request must complete 200 with a `slow-pong` body \
                 inside the drain budget, got {other:?}"
            ));
        }
        Err(e) => {
            return Err(format!("in-flight request never completed within 20s: {e}"));
        }
    }
    let code = wait_exit_code(&mut child, Duration::from_secs(30));
    if code != 0 {
        return Err(format!(
            "the drained shutdown must exit 0, got {code}:\n{}",
            drained.captured()
        ));
    }
    let report = std::fs::read_to_string(deploy.path().join("drain.json"))
        .map_err(|e| format!("route report must be written: {e}"))?;
    if report.trim() != COMPLETED_ROUTE_REPORT {
        return Err(format!(
            "report must be exactly {COMPLETED_ROUTE_REPORT}, got {report}"
        ));
    }
    Ok(())
}

/// The serving artifact with a 3 s listener route, request provably in
/// flight (the `slow-enter` marker fired) → SIGTERM immediately after
/// the marker, ≈0 s into the 3 s delay → the in-flight response
/// `slow-pong` is still delivered inside the 10 s drain budget, and the
/// process exits 0 (openspec r5routesrv, cli-compile
/// "First signal drains gracefully and exits 0").
#[test]
fn route_server_drains_inflight_request() {
    child_guard();
    with_bind_race_retry(drain_inflight_flow);
}

/// The slow WebSocket listener document: the same step chain as
/// [`slow_rest_doc`] (`log: "slow-enter"`, `delay`, `set_body:
/// "slow-pong"`) hung off a `ws://` consumer instead of a REST
/// operation. The `to:` producer targets the same `ws://` URI the
/// consumer serves: because a local consumer exists, the producer runs
/// in server-send mode and echoes back on the sender's connection key
/// (the same shape as `examples/ws-server`).
fn ws_slow_doc(port: u16, delay_ms: u64) -> String {
    format!(
        "\
routes:
  - id: ws-slow-echo
    from: ws://127.0.0.1:{port}/echo
    steps:
      - log: \"slow-enter\"
      - delay: {delay_ms}
      - set_body: \"slow-pong\"
      - to: ws://127.0.0.1:{port}/echo
"
    )
}

/// Perform the RFC 6455 opening handshake as a raw TCP client and
/// return the upgraded stream: a bounded-retry connect loop (50
/// attempts, 100 ms apart — the same bounded-poll posture as
/// [`wait_for_marker`]'s 20 ms steps, since the artifact binds its ws
/// listener only after boot), then one handshake exchange with a 5 s
/// read timeout. Returns the stream only when the server answered
/// `HTTP/1.1 101`; anything else (refused connect, rejected upgrade,
/// early EOF) yields `None`.
fn ws_connect(port: u16) -> Option<TcpStream> {
    use std::io::{Read, Write};
    for _ in 0..50 {
        let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
            thread::sleep(Duration::from_millis(100));
            continue;
        };
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let request = format!(
            "GET /echo HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             \r\n"
        );
        stream.write_all(request.as_bytes()).ok()?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 512];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut chunk).ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let head = String::from_utf8_lossy(&buf);
        return head.starts_with("HTTP/1.1 101").then_some(stream);
    }
    None
}

/// Send one masked text frame (RFC 6455 client framing: every
/// client-to-server frame MUST be masked): FIN+text opcode `0x81`,
/// length byte with the mask bit set (`len < 126` for this battery),
/// the fixed mask, and the payload XOR-masked. I/O failures surface
/// later as a read timeout in [`ws_read_text`], which the flow
/// already reports.
fn ws_send_text(stream: &mut TcpStream, text: &str) {
    use std::io::Write;
    let mask = [0x37u8, 0xfa, 0x21, 0x3d];
    let payload = text.as_bytes();
    let mut frame = Vec::with_capacity(payload.len() + 6);
    frame.push(0x81);
    frame.push(0x80 | payload.len() as u8);
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    let _ = stream.write_all(&frame);
}

/// Read one unmasked server text frame and return its UTF-8 payload:
/// two header bytes first — a set mask bit means a server framing
/// violation, so `None` — then `header[1] & 0x7f` payload bytes (both
/// sides keep frames under 126 bytes in this battery). `None` on any
/// I/O failure or non-UTF-8 payload.
fn ws_read_text(stream: &mut TcpStream) -> Option<String> {
    use std::io::Read;
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).ok()?;
    if header[1] & 0x80 != 0 {
        return None;
    }
    let mut payload = vec![0u8; usize::from(header[1] & 0x7f)];
    stream.read_exact(&mut payload).ok()?;
    String::from_utf8(payload).ok()
}

/// One full WebSocket drain flow: fresh port → slow ws doc (3 s delay
/// route) → compile → deploy → spawn with `--report` → boot → raw
/// RFC 6455 handshake → one masked text frame → `slow-enter` observed
/// (the exchange is PROVABLY inside the delayed step) → SIGTERM
/// mid-delay → the in-flight echo still completes inside the drain
/// budget → exit 0 → the exact completed report. `Err` carries the
/// failure text; the [`BIND_RACE_MARK`] signature inside it makes the
/// caller retry the whole flow once (see [`with_bind_race_retry`]).
fn ws_drain_flow() -> Result<(), String> {
    let port = free_port();
    let doc = ws_slow_doc(port, 3000);
    let (deploy, artifact) = compile_and_deploy_listener_files(&[("doc.yaml", &doc)])?;
    let mut child = spawn_child(
        WS_DRAIN_TEST,
        deploy.path(),
        &artifact,
        &["--report", "drain-ws.json"],
        &[],
    );
    let drained = spawn_drained(&mut child);
    if !wait_for_marker(&drained, "context started", Duration::from_secs(60)) {
        let captured = drained.captured();
        return Err(format!("artifact never booted:\n{captured}"));
    }
    let Some(mut stream) = ws_connect(port) else {
        let captured = drained.captured();
        return Err(format!(
            "ws listener never accepted the upgrade:\n{captured}"
        ));
    };
    ws_send_text(&mut stream, "hello");
    // The reader thread owns the upgraded stream: the flow must be
    // free to send SIGTERM while the echo is still in flight.
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(ws_read_text(&mut stream));
    });
    if !wait_for_marker_tight(&mut child, &drained, "slow-enter", Duration::from_secs(20)) {
        let captured = drained.captured();
        return Err(format!(
            "ws exchange never reached the delayed step (missing `slow-enter`):\n{captured}"
        ));
    }
    // SIGTERM lands immediately after the `slow-enter` marker, i.e.
    // ≈0 s into the 3 s delay step, so the full 3 s still fit inside
    // the 10 s default drain budget (`default_drain_timeout_ms` in
    // camel-config); the in-flight echo must still be delivered, then
    // exit 0.
    send_signal(&child.0, "-TERM");
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Some(s)) if s.contains("slow-pong") => {}
        Ok(other) => {
            let captured = drained.captured();
            return Err(format!(
                "in-flight ws exchange must complete with a `slow-pong` echo \
                 inside the drain budget, got {other:?}:\n{captured}"
            ));
        }
        Err(e) => {
            let captured = drained.captured();
            return Err(format!(
                "in-flight ws exchange never completed within 20s ({e}):\n{captured}"
            ));
        }
    }
    let code = wait_exit_code(&mut child, Duration::from_secs(30));
    if code != 0 {
        return Err(format!(
            "the drained shutdown must exit 0, got {code}:\n{}",
            drained.captured()
        ));
    }
    let report = std::fs::read_to_string(deploy.path().join("drain-ws.json"))
        .map_err(|e| format!("route report must be written: {e}"))?;
    if report.trim() != COMPLETED_ROUTE_REPORT {
        return Err(format!(
            "report must be exactly {COMPLETED_ROUTE_REPORT}, got {report}"
        ));
    }
    Ok(())
}

/// The serving artifact with a 3 s ws consumer route, exchange
/// provably in flight (the `slow-enter` marker fired) → SIGTERM
/// immediately after the marker, ≈0 s into the 3 s delay → the
/// in-flight echo `slow-pong` is still delivered on the same
/// connection inside the 10 s drain budget, and the process exits 0
/// (bd rc-z332y, openspec r5batteries, cli-compile "WebSocket consumer
/// drains an in-flight exchange").
#[test]
fn route_server_drains_inflight_ws_exchange() {
    child_guard();
    with_bind_race_retry(ws_drain_flow);
}

/// The exact test named by the harness-child spawn of
/// [`boot_signal_buffered_flow`] (same re-entry mechanism as
/// [`SERVE_TEST`]).
const BOOT_SIGNAL_TEST: &str = "route_server_boot_signal_is_buffered";

/// One full boot-buffer flow: fresh port → doc → compile → deploy →
/// spawn → SIGTERM at the EARLIEST artifact boot marker → boot
/// completes → graceful shutdown → exit 0. `Err` carries the failure
/// text; the [`BIND_RACE_MARK`] signature inside it makes the caller
/// retry the whole flow once (see [`with_bind_race_retry`]).
fn boot_signal_buffered_flow() -> Result<(), String> {
    let port = free_port();
    let (deploy, artifact) = compile_and_deploy_listener(&rest_listener_doc(port))?;
    let mut child = spawn_child(BOOT_SIGNAL_TEST, deploy.path(), &artifact, &[], &[]);
    let drained = spawn_drained(&mut child);
    // The EARLIEST artifact boot marker: the embedded virtual store
    // load precedes `Starting CamelContext` and the whole component
    // cascade — mirroring `run_signal_test`'s use of the earliest
    // CWD-trust marker — so it leaves the whole boot stretch as the
    // signal-delivery window. The 5 ms poll keeps marker-observation
    // staleness well under that stretch.
    if !wait_for_marker_tight(
        &mut child,
        &drained,
        "from the embedded virtual store",
        Duration::from_secs(30),
    ) {
        let captured = drained.captured();
        return Err(format!("artifact never reached mid-boot:\n{captured}"));
    }
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(&mut child, Duration::from_secs(90));
    let captured = drained.captured();
    if code != 0 {
        return Err(format!(
            "a mid-boot SIGTERM must be buffered and shut down gracefully (exit 0); \
             a default-disposition kill would surface as -1, got {code}:\n{captured}"
        ));
    }
    if !captured.contains("CamelContext started") {
        return Err(format!(
            "boot must complete past the marker (missing `CamelContext started`):\n{captured}"
        ));
    }
    if !captured.contains("shutting down") {
        return Err(format!(
            "the buffered signal must end in the graceful shutdown \
             (missing `shutting down`):\n{captured}"
        ));
    }
    Ok(())
}

/// A compiled route artifact mid-boot (observed at the earliest boot
/// marker, `from the embedded virtual store`) receives SIGTERM → the
/// signal is buffered, boot completes (`CamelContext started`), the
/// graceful shutdown runs (`shutting down`), and the process exits 0
/// within 90 s — never a default-disposition death (openspec
/// r5routesrv, cli-compile "Route artifact signal during boot is
/// buffered").
#[test]
fn route_server_boot_signal_is_buffered() {
    child_guard();
    with_bind_race_retry(boot_signal_buffered_flow);
}

/// The exact test named by the harness-child spawn of
/// [`second_signal_force_exit_flow`] (same re-entry mechanism as
/// [`SERVE_TEST`]).
const SECOND_SIGNAL_TEST: &str = "route_server_second_signal_force_exits";

/// One full force-exit flow: fresh port → doc → compile → deploy →
/// spawn → INT+TERM pair at the EARLIEST boot marker, delivered as ONE
/// `sh -c` invocation → exit 1 with the `forcing exit` WARN. `Err`
/// carries the failure text; the [`BIND_RACE_MARK`] signature inside it
/// makes the caller retry the whole flow once (see
/// [`with_bind_race_retry`]).
fn second_signal_force_exit_flow() -> Result<(), String> {
    let port = free_port();
    let (deploy, artifact) = compile_and_deploy_listener(&rest_listener_doc(port))?;
    let mut child = spawn_child(SECOND_SIGNAL_TEST, deploy.path(), &artifact, &[], &[]);
    let drained = spawn_drained(&mut child);
    // The EARLIEST artifact boot marker (see [`boot_signal_buffered_flow`]):
    // the whole boot stretch stays open as the pair-delivery window, and
    // the 5 ms poll keeps marker-observation staleness well under it.
    if !wait_for_marker_tight(
        &mut child,
        &drained,
        "from the embedded virtual store",
        Duration::from_secs(30),
    ) {
        let captured = drained.captured();
        return Err(format!("artifact never reached mid-boot:\n{captured}"));
    }
    // The escape-hatch pair as ONE shell invocation: `kill` is a shell
    // builtin, and two separate spawn(2)s would leave a multi-ms exec
    // gap that can push the second signal past teardown under load —
    // the same discipline as
    // `run_signal_test::second_sigterm_during_teardown_force_exits`.
    let pair = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "kill -INT {pid}; kill -TERM {pid}",
            pid = child.id()
        ))
        .status()
        .map_err(|e| format!("spawn signal pair: {e}"))?;
    if !pair.success() {
        return Err(format!("signal pair returned non-zero: {pair:?}"));
    }
    let code = wait_exit_code(&mut child, Duration::from_secs(90));
    let captured = drained.captured();
    if code != 1 {
        return Err(format!(
            "the buffered second signal must force-exit 1; exit 0 means \
             the force-exit arm never fired, -1 means a \
             default-disposition kill, got {code}:\n{captured}"
        ));
    }
    if !captured.contains("forcing exit") {
        return Err(format!(
            "the force exit must log its `forcing exit` WARN:\n{captured}"
        ));
    }
    Ok(())
}

/// A compiled route artifact mid-boot (observed at the earliest boot
/// marker) receives an INT+TERM pair buffered during boot → the
/// shutdown select consumes the first, the force-exit guard polls the
/// already-queued second during teardown → exit 1 within 90 s with the
/// `forcing exit` WARN (openspec r5routesrv, cli-compile "Second signal
/// force-exits during teardown").
#[test]
fn route_server_second_signal_force_exits() {
    child_guard();
    with_bind_race_retry(second_signal_force_exit_flow);
}

/// The exact test named by the harness-child spawn of
/// [`deployment_equivalence_flow`] (same re-entry mechanism as
/// [`SERVE_TEST`]).
const EQUIVALENCE_TEST: &str = "route_server_matches_camel_run_deployment_posture";

/// Shared leg assertion for [`deployment_equivalence_flow`]: wait for
/// `context started`, require HTTP 200 with a `pong` body on the leg's
/// port, send SIGTERM, and require exit 0 (30 s bound).
fn serve_once_and_terminate(
    child: &mut KillOnDrop,
    drained: &Drained,
    port: u16,
    leg: &str,
) -> Result<(), String> {
    if !wait_for_marker(drained, "context started", Duration::from_secs(60)) {
        return Err(format!("{leg} never booted:\n{}", drained.captured()));
    }
    match http_get(port, "/api/ping") {
        Some((200, body)) if body.contains("pong") => {}
        other => {
            return Err(format!(
                "{leg} must answer 200 with a `pong` body before the signal, got {other:?}"
            ));
        }
    }
    send_signal(&child.0, "-TERM");
    let code = wait_exit_code(child, Duration::from_secs(30));
    if code != 0 {
        return Err(format!(
            "{leg} must shut down gracefully on SIGTERM (exit 0), got {code}:\n{}",
            drained.captured()
        ));
    }
    Ok(())
}

/// One full deployment-equivalence flow: one fixture dir holding two
/// independently probed listener documents — `rest.yaml` served by the
/// compiled artifact (leg A), `rest2.yaml` served by
/// `camel run --routes rest2.yaml --no-watch` (leg B) — and each leg
/// must serve HTTP 200 `pong` after `context started`, then exit 0 on
/// SIGTERM. `Err` carries the failure text; the [`BIND_RACE_MARK`]
/// signature inside it makes the caller retry the whole flow once (see
/// [`with_bind_race_retry`]).
fn deployment_equivalence_flow() -> Result<(), String> {
    // A separate free_port() per leg: camel-http binds without
    // SO_REUSEADDR and `Connection: close` leaves server-side TIME_WAIT
    // on the port, so reusing one port across legs risks EADDRINUSE
    // false-reds; behavioral equivalence does not require byte-identical
    // documents.
    let artifact_port = free_port();
    let run_port = free_port();
    let dir = tempfile::Builder::new()
        .prefix("camel-routesrv-equiv-")
        .tempdir_in(fixture_root())
        .map_err(|e| format!("fixture tempdir: {e}"))?;
    std::fs::write(
        dir.path().join("rest.yaml"),
        rest_listener_doc(artifact_port),
    )
    .map_err(|e| format!("write rest.yaml: {e}"))?;
    std::fs::write(dir.path().join("rest2.yaml"), rest_listener_doc(run_port))
        .map_err(|e| format!("write rest2.yaml: {e}"))?;

    // Leg A: the compiled artifact.
    let output = compile(dir.path(), "rest.yaml", "rest.bin", &[]);
    if output.status.code() != Some(0) {
        return Err(format!(
            "rest.yaml must compile: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let (deploy, artifact) = deploy_artifact(&dir.path().join("rest.bin"));
    let mut child = spawn_child(EQUIVALENCE_TEST, deploy.path(), &artifact, &[], &[]);
    let drained = spawn_drained(&mut child);
    serve_once_and_terminate(&mut child, &drained, artifact_port, "leg A (artifact)")?;

    // Leg B: `camel run` over the same document shape — cwd = the
    // fixture dir, the parent's `CAMEL_*` keys scrubbed (see
    // [`scrub_camel_env`]), the canonical `env!("CARGO_BIN_EXE_camel")`
    // binary path (harness precedent `common::spawn_camel_run`).
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_camel"));
    scrub_camel_env(&mut cmd);
    cmd.args(["run", "--routes", "rest2.yaml", "--no-watch"])
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut run_child = KillOnDrop(cmd.spawn().map_err(|e| format!("spawn `camel run`: {e}"))?);
    let run_drained = spawn_drained(&mut run_child);
    serve_once_and_terminate(&mut run_child, &run_drained, run_port, "leg B (camel run)")?;
    Ok(())
}

/// The same REST listener document deployed as a compiled artifact and
/// run through `camel run --routes <doc> --no-watch` observe identical
/// serve + exit behavior: HTTP 200 `pong` after `context started`,
/// graceful exit 0 on SIGTERM (openspec r5routesrv, cli-compile
/// "Deployment-equivalence with camel run").
#[test]
fn route_server_matches_camel_run_deployment_posture() {
    child_guard();
    with_bind_race_retry(deployment_equivalence_flow);
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

/// `--truststore` with a missing trailing value exits 2 naming the
/// argument, without booting (keypin Task 1.1).
#[test]
fn truststore_argument_requires_value() {
    let (deploy, artifact) = deploy_artifact(&fixture().multi_route);
    let (code, stdout, stderr) =
        common::run_binary(deploy.path(), &artifact, &["--truststore"], &[]);
    assert_eq!(
        code, 2,
        "a trailing --truststore must exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("requires a value"),
        "the diagnostic must carry the missing-value message: {combined}"
    );
    assert!(!combined.contains("context started"), "no boot: {combined}");
}

/// `--truststore` is a modifier, not an exclusive mode: paired with
/// `--manifest`, `--help`, or `--version` it exits 2 with the
/// duplicate-exclusive diagnostic, without booting (keypin Task 1.1).
#[test]
fn truststore_rejected_with_exclusive_modes() {
    let (deploy, artifact) = deploy_artifact(&fixture().multi_route);
    for mode in ["--manifest", "--help", "--version"] {
        let argv = ["--truststore", "truststore.keys", mode];
        let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &argv, &[]);
        assert_eq!(
            code, 2,
            "argv {argv:?} must exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let combined = format!("{stdout}{stderr}");
        assert!(
            combined.contains("mutually exclusive"),
            "argv {argv:?} must carry the duplicate-exclusive diagnostic: {combined}"
        );
        assert!(
            !combined.contains("context started"),
            "argv {argv:?} must not boot: {combined}"
        );
    }
}

/// `CAMEL_TRUSTSTORE` is benign at compile time (keypin Task 1.1): the
/// clean-environment guard accepts the verify-side variable — it is
/// never read at compile time — so an unsigned compile with it set
/// succeeds.
#[test]
fn compile_with_camel_truststore_env_is_benign() {
    let dir = tempfile::Builder::new()
        .prefix("camel-truststore-compile-")
        .tempdir_in(fixture_root())
        .expect("compile tempdir on the fixture root");
    std::fs::write(dir.path().join("benign.job.yaml"), JOB_DOC).expect("write document");
    let output = compile(
        dir.path(),
        "benign.job.yaml",
        "benign.bin",
        &[("CAMEL_TRUSTSTORE", "/deploy/truststore.keys")],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "compile with CAMEL_TRUSTSTORE set must succeed;\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
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

/// r3jobs Task 2.1: a multi-entry chain job — `routeFiles` naming four
/// route files whose direct endpoints chain B → B.C1 → B.C1.C2 →
/// B.C1.C2.A — boots EVERY embedded entry, not just the first. The
/// pinned parity: `camel job` on the same document set and the compiled
/// artifact agree on outcome `Completed` and reply `B.C1.C2.A`, so the
/// reply value alone proves all four files' routes were present and ran.
#[test]
fn compiled_job_multi_entry_boots_every_entry() {
    child_guard();
    let source_dir = fixture()
        .multi_job_n
        .parent()
        .expect("chain artifact has its source subtree")
        .to_path_buf();

    // Leg 1 — CLI parity: the same source set through `camel job`.
    let (code, stdout, stderr) = common::run_binary(
        &source_dir,
        Path::new(env!("CARGO_BIN_EXE_camel")),
        &[
            "job",
            "ingest-n.job.yaml",
            "--config",
            "Camel.toml",
            "--report",
            "cli-report.json",
        ],
        &[],
    );
    assert_eq!(
        code, 0,
        "CLI job must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let cli_report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(source_dir.join("cli-report.json")).expect("CLI report written"),
    )
    .expect("CLI report is JSON");
    assert_eq!(cli_report["outcome"], "Completed", "report: {cli_report}");
    assert_eq!(
        cli_report["reply"]["body"], "B.C1.C2.A",
        "chain must traverse all four files' routes: {cli_report}"
    );

    // Leg 2 — artifact: deployed WITHOUT the source tree, config, or
    // routes; the embedded plan alone must boot the whole chain.
    let (deploy, artifact) = deploy_artifact(&fixture().multi_job_n);
    for absent in ["ingest-n.job.yaml", "Camel.toml", "routes", "conf"] {
        assert!(
            !deploy.path().join(absent).exists(),
            "no source/config tree: {absent} must not exist"
        );
    }
    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_multi_entry_boots_every_entry",
        deploy.path(),
        &artifact,
        &["--report", "report.json"],
        &[],
    );
    assert_eq!(
        code, 0,
        "multi-entry artifact must complete;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("report.json"))
            .expect("artifact report written"),
    )
    .expect("artifact report is JSON");
    assert_eq!(
        report["outcome"], "Completed",
        "job-level parity with the CLI run: {report}"
    );
    assert_eq!(
        report["reply"]["body"], "B.C1.C2.A",
        "compiled chain must traverse all four entries: {report}"
    );
    assert_eq!(
        report["document"], "compiled://ingest-n.job.yaml",
        "virtual entry-point identity: {report}"
    );
}

/// r3jobs Task 2.2: a multi-entry job whose route file set holds a
/// STRUCTURALLY INVALID entry (`routes/bad.yaml`, unknown step key
/// under a `direct:` consumer) still compiles — declaration checks
/// only — but boot must reject it with exit 2, naming the offending
/// entry, before any boot and without an outcome report.
#[test]
fn compiled_job_multi_entry_failure_names_entry() {
    child_guard();
    let (deploy, artifact) = deploy_artifact(&fixture().multi_job_bad);
    for absent in ["ingest-bad.job.yaml", "Camel.toml", "routes", "conf"] {
        assert!(
            !deploy.path().join(absent).exists(),
            "no source/config tree: {absent} must not exist"
        );
    }
    let (code, stdout, stderr) = spawn_child_output(
        "compiled_job_multi_entry_failure_names_entry",
        deploy.path(),
        &artifact,
        &["--report", "bad-report.json"],
        &[],
    );
    assert_eq!(
        code, 2,
        "structure-invalid entry must fail the boot;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("compiled://routes/bad.yaml"),
        "diagnostic must name the failing entry;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !deploy.path().join("bad-report.json").exists(),
        "a boot-class failure writes no outcome report"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(!combined.contains("context started"), "no boot: {combined}");
}

/// r3jobs Task 2.3: `--manifest` on the multi-entry chain job artifact
/// lists the job entry, every chained route entry, and the
/// config/include entries — each document entry with its byte length
/// and content digest — and exits 0 without booting any route.
#[test]
fn artifact_manifest_lists_multi_entry_job_without_boot() {
    let (deploy, artifact) = deploy_artifact(&fixture().multi_job_n);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    assert_eq!(
        code, 0,
        "--manifest exits 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is manifest JSON");
    assert_eq!(manifest["kind"], "job", "manifest: {manifest}");
    assert_eq!(manifest["artifact_kind"], "job", "manifest: {manifest}");
    let files = manifest["embedded_files"]
        .as_array()
        .expect("embedded_files array");

    // The canonical entry order: configuration chain first, then the
    // job entry point, then the four chained route entries. The exact
    // list pins the full entry set, not just membership.
    let expected = [
        ("Camel.toml", "config"),
        ("conf/base.toml", "include"),
        ("ingest-n.job.yaml", "job"),
        ("routes/a.yaml", "route"),
        ("routes/b.yaml", "route"),
        ("routes/c1.yaml", "route"),
        ("routes/c2.yaml", "route"),
    ];
    let listed: Vec<(String, String)> = files
        .iter()
        .map(|f| {
            (
                f["path"].as_str().expect("path").to_string(),
                f["kind"].as_str().expect("kind").to_string(),
            )
        })
        .collect();
    let want: Vec<(String, String)> = expected
        .iter()
        .map(|(p, k)| (p.to_string(), k.to_string()))
        .collect();
    assert_eq!(
        listed, want,
        "every embedded logical path is listed in canonical order: {manifest}"
    );

    // Each document entry carries its kind plus a non-null byte length
    // and a content digest.
    for (entry, (path, kind)) in files.iter().zip(expected) {
        assert_eq!(
            entry["kind"].as_str(),
            Some(kind),
            "kind for {path}: {manifest}"
        );
        assert!(
            entry["length"].as_u64().is_some(),
            "byte length listed for {path}: {manifest}"
        );
        assert!(
            entry["digest"].as_str().is_some_and(|d| !d.is_empty()),
            "content digest listed for {path}: {manifest}"
        );
    }

    let all = format!("{stdout}{stderr}");
    assert!(!all.contains("context started"), "no job boot: {all}");
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

// ---------------------------------------------------------------------------
// r4sign Task 1.3: run-side verification. Every case below drives the REAL
// self-detect entry — the artifact binary itself — because the signature
// chain lives in `self_detect_artifact`, before the arg dispatch. The
// tamper cases follow the tamper-on-COPY discipline: the shared fixture
// and its hardlinked deploys are never mutated.
// ---------------------------------------------------------------------------

/// keypin Task 2.1: a signed compile emits manifest schema 5 whose
/// signing block carries the unix-seconds freshness marker, pinned to
/// the fixture's signed-compile window (the marker is written by THIS
/// suite's compile — a stale cached artifact or a clock-skewed marker
/// falls outside it), beside the unchanged algorithm name, key
/// fingerprint, and required bit — and `--manifest` prints the stored
/// canonical JSON verbatim, marker included. The window replaces an
/// earlier 60-seconds-from-now assert that flaked whenever libtest
/// scheduled this test late in a long battery (bd rc-xkdbe).
#[test]
fn signed_compile_emits_schema5_freshness_marker() {
    let fixture = fixture();
    let (deploy, artifact) = deploy_signed(&fixture.signed_job);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    assert_eq!(
        code, 0,
        "--manifest on a signed artifact must succeed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is manifest JSON");
    assert_eq!(
        manifest["manifest_schema"].as_u64(),
        Some(5),
        "signed compiles emit manifest schema 5: {manifest}"
    );
    let freshness = manifest["signing"]["freshness"]
        .as_u64()
        .expect("the schema-5 signing block carries the freshness marker");
    let now = unix_now();
    assert!(
        freshness >= fixture.signed_compile_started_at && freshness <= now,
        "freshness marker {freshness} must sit inside the fixture compile window \
         [started {}, now {now}] — a marker below the window means a stale cached \
         artifact, above now means a clock skew (bd rc-xkdbe)",
        fixture.signed_compile_started_at
    );
    assert_eq!(
        manifest["signing"]["algorithm"].as_str(),
        Some("ed25519ph"),
        "the signing block keeps the algorithm name: {manifest}"
    );
    assert_eq!(
        manifest["signing"]["required"].as_bool(),
        Some(false),
        "the signing block keeps the required bit: {manifest}"
    );
    assert!(
        manifest["signing"]["key_fingerprint"]
            .as_str()
            .is_some_and(|f| f.starts_with("blake3:")),
        "the signing block keeps the key fingerprint: {manifest}"
    );
}

/// A signed artifact boots and completes exactly as its unsigned twin:
/// the same exit code, the same job outcome and reply, and no signature
/// diagnostic anywhere (the valid envelope verifies silently).
#[test]
fn signed_artifact_boots_with_valid_envelope() {
    let (unsigned_deploy, unsigned_artifact) = deploy_artifact(&fixture().job);
    let (unsigned_code, _, _) = common::run_binary(
        unsigned_deploy.path(),
        &unsigned_artifact,
        &["--report", "unsigned-report.json"],
        &[],
    );
    assert_eq!(
        unsigned_code, 0,
        "the unsigned twin must complete (fixture sanity)"
    );

    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let (code, stdout, stderr) =
        common::run_binary(deploy.path(), &artifact, &["--report", "report.json"], &[]);
    assert_eq!(
        code, unsigned_code,
        "signed artifact must exit exactly like its unsigned twin;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("report.json"))
            .expect("signed artifact must write its report"),
    )
    .expect("report is JSON");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert_eq!(
        report["reply"]["body"], "job-done",
        "the embedded route must run unchanged: {report}"
    );
    let all = format!("{stdout}{stderr}");
    assert!(
        !all.contains("signature"),
        "a valid envelope verifies silently: {all}"
    );
}

/// End of the executable image inside artifact bytes: everything
/// before the appended trailer (leading magic + content + index +
/// manifest + footer). The v2 footer carries the three section lengths
/// as little-endian `u64`s at offsets 12/20/28.
fn exe_body_end(bytes: &[u8]) -> usize {
    let footer = bytes.len() - trailer::FOOTER_LEN_V2;
    let section_len = |at: usize| {
        u64::from_le_bytes(
            bytes[footer + at..footer + at + 8]
                .try_into()
                .expect("footer window"),
        ) as usize
    };
    footer - 8 - section_len(12) - section_len(20) - section_len(28)
}

/// One flipped byte in the executable body (outside the trailer
/// checksum domain) breaks only the detached signature: the trailer
/// decodes, the envelope verifies against the streamed bytes, and the
/// artifact exits 2 naming the signature failure — no boot.
///
/// The flip lands on the LAST image byte (section-header-table
/// metadata the ELF loader never reads): a low offset such as 1000
/// sits inside `.dynsym`, where one flipped byte kills exec itself
/// (exit 127) before any verification code can run.
#[test]
fn tampered_exe_body_fails_closed() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let bytes = std::fs::read(&artifact).expect("artifact bytes");
    let flip = exe_body_end(&bytes) - 1;
    let tampered = deploy_flipped_copy(&deploy, &artifact, "tampered-body.bin", flip);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &tampered, &[], &[]);
    assert_eq!(
        code, 2,
        "tampered body must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("signature"),
        "the diagnostic must name the signature failure: {combined}"
    );
    assert!(
        !combined.contains("context started"),
        "no boot after tampering: {combined}"
    );
}

/// One flipped byte inside the trailer's manifest span breaks the
/// trailer checksum before any signature step: exit 2 with the trailer
/// integrity diagnostic, no boot.
#[test]
fn tampered_trailer_span_fails_closed() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let len = std::fs::metadata(&artifact).expect("artifact size").len() as usize;
    // The manifest is the last section before the v2 footer, so its
    // final byte sits directly in front of the footer window.
    let tampered = deploy_flipped_copy(
        &deploy,
        &artifact,
        "tampered-trailer.bin",
        len - trailer::FOOTER_LEN_V2 - 1,
    );
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &tampered, &[], &[]);
    assert_eq!(
        code, 2,
        "tampered trailer must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("integrity error"),
        "the diagnostic must be the trailer integrity error: {combined}"
    );
    assert!(
        !combined.contains("context started"),
        "no boot after tampering: {combined}"
    );
}

/// An envelope re-created under a SECOND key over the same (untampered)
/// artifact digest fails closed: the envelope is well-formed but its
/// verifying key does not match the manifest fingerprint — exit 2
/// naming the fingerprint mismatch, no boot.
#[test]
fn wrong_key_envelope_fails_closed() {
    use camel_cli::compile::signature;
    use ed25519_dalek::{Digest as _, Sha512, SigningKey};

    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let bytes = std::fs::read(&artifact).expect("artifact bytes");
    let digest: [u8; 64] = Sha512::digest(&bytes).into();
    // Second synthetic key: 32 obvious pattern bytes, never key material.
    let second_key = SigningKey::from_bytes(b"r4sign-second-key-0000000000000\0");
    // The deployed envelope is a copy: overwriting it never touches the
    // shared fixture.
    std::fs::write(
        sig_path_of(&artifact),
        signature::encode_envelope(&second_key, &digest),
    )
    .expect("write wrong-key envelope");

    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &[], &[]);
    assert_eq!(
        code, 2,
        "wrong-key envelope must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("fingerprint"),
        "the diagnostic must name the fingerprint mismatch: {combined}"
    );
    assert!(
        !combined.contains("context started"),
        "no boot on fingerprint mismatch: {combined}"
    );
}

/// An envelope truncated by one byte fails closed with the ENVELOPE
/// diagnostic — distinguishable from a signature failure — and no boot.
#[test]
fn corrupt_envelope_fails_closed() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let sig = sig_path_of(&artifact);
    let mut bytes = std::fs::read(&sig).expect("envelope bytes");
    assert_eq!(bytes.len(), 148, "fixture envelope is intact");
    bytes.pop();
    std::fs::write(&sig, bytes).expect("write truncated envelope");

    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &[], &[]);
    assert_eq!(
        code, 2,
        "truncated envelope must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("envelope"),
        "the diagnostic must name the envelope step: {combined}"
    );
    assert!(
        !combined.contains("context started"),
        "no boot on corrupt envelope: {combined}"
    );
}

/// A `--sign --require-signature` artifact whose envelope was removed
/// refuses to boot: exit 2 naming the missing REQUIRED signature.
#[test]
fn required_signature_missing_envelope_fails_closed() {
    let (deploy, artifact) = deploy_artifact(&fixture().signed_required_job);
    assert!(
        !sig_path_of(&artifact).exists(),
        "the required twin deploys without its envelope"
    );
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &[], &[]);
    assert_eq!(
        code, 2,
        "missing required signature must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("required") && combined.contains("signature"),
        "the diagnostic must name the required signature: {combined}"
    );
    assert!(
        !combined.contains("context started"),
        "no boot without the required envelope: {combined}"
    );
}

/// An envelope that cannot be READ at runtime (here: `.sig` is a
/// directory) fails closed with a named diagnostic, never boots.
#[test]
fn envelope_io_error_fails_closed() {
    // `.sig` exists but is a directory: the read fails mid-verify and
    // the artifact must fail closed with a named diagnostic (r_glm
    // holistic finding: the IO-error arm of the verify chain had no
    // battery coverage).
    let (deploy, artifact) = deploy_artifact(&fixture().signed_job);
    std::fs::create_dir_all(sig_path_of(&artifact)).expect("create .sig dir");
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &[], &[]);
    assert_eq!(
        code, 2,
        "unreadable envelope must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("signature verification") || combined.contains("envelope"),
        "the diagnostic must name the verification step: {combined}"
    );
    assert!(
        !combined.contains("context started"),
        "no boot with an unreadable envelope: {combined}"
    );
}

/// An unsigned artifact with ANY envelope beside it fails closed as an
/// unpaired envelope: the schema-3 manifest carries no signing block,
/// so no envelope may be present — exit 2, no boot.
#[test]
fn unsigned_with_stray_envelope_fails_closed() {
    let (deploy, artifact) = deploy_artifact(&fixture().job);
    std::fs::write(sig_path_of(&artifact), b"stray").expect("write stray envelope");
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &[], &[]);
    assert_eq!(
        code, 2,
        "stray envelope on an unsigned artifact must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("unpaired"),
        "the diagnostic must name the unpaired envelope: {combined}"
    );
    assert!(
        !combined.contains("context started"),
        "no boot on a stray envelope: {combined}"
    );
}

/// A v1 legacy artifact with ANY envelope beside it fails closed as an
/// unpaired envelope: the v1 trailer's schema-less manifest decodes as
/// schema 1 and carries no signing block, so no envelope may be present
/// — exit 2, no boot. Direct twin of the schema-3 case, bd rc-5u0jx
/// item (a). The artifact is a REAL executable — the camel binary with
/// a hand-assembled v1 trailer appended (the compiler emits only v2
/// trailers now) — because the unpaired check lives in the binary
/// self-detect path, which the harness child branch never enters.
#[test]
fn v1_artifact_with_stray_envelope_fails_closed() {
    // No harness-child spawn below (run_binary executes the artifact),
    // but the guard keeps this test safe against any future conversion
    // to a spawn helper (bd rc-wvydl).
    child_guard();
    use std::io::Write as _;
    let deploy = tempfile::tempdir().expect("deploy tempdir");

    let manifest =
        camel_cli::compile::manifest::derive("app.yaml", trailer::TrailerKind::Route, ROUTE_DOC)
            .expect("v1 manifest derives");
    let v1 = trailer::Trailer {
        kind: trailer::TrailerKind::Route,
        payload: ROUTE_DOC.as_bytes().to_vec(),
        manifest: manifest.to_legacy_json().into_bytes(),
    };
    let artifact = deploy.path().join("app.bin");
    std::fs::copy(env!("CARGO_BIN_EXE_camel"), &artifact).expect("copy camel binary");
    let mut tail = std::fs::OpenOptions::new()
        .append(true)
        .open(&artifact)
        .expect("open artifact for trailer append");
    tail.write_all(&trailer::encode(&v1))
        .expect("append v1 trailer");
    drop(tail);
    std::fs::write(sig_path_of(&artifact), b"stray").expect("write stray envelope");

    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &[], &[]);
    assert_eq!(
        code, 2,
        "stray envelope on a v1 artifact must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("unpaired"),
        "the diagnostic must name the unpaired envelope: {combined}"
    );
    assert!(
        combined.contains("schema 1"),
        "the diagnostic must pin the legacy manifest's default schema 1: {combined}"
    );
    assert!(
        !combined.contains("context started"),
        "no boot on a stray envelope: {combined}"
    );
}

/// An unsigned artifact without an envelope still boots: the v1
/// compatibility regression guard — no signature step, zero hashing.
#[test]
fn unsigned_without_envelope_still_runs() {
    let (deploy, artifact) = deploy_artifact(&fixture().job);
    assert!(
        !sig_path_of(&artifact).exists(),
        "no envelope beside the unsigned twin"
    );
    let (code, stdout, stderr) =
        common::run_binary(deploy.path(), &artifact, &["--report", "report.json"], &[]);
    assert_eq!(
        code, 0,
        "unsigned artifact must boot unchanged;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("report.json"))
            .expect("report must be written"),
    )
    .expect("report is JSON");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    let all = format!("{stdout}{stderr}");
    assert!(!all.contains("signature"), "no signature step: {all}");
}

/// `--verify` on a required-signature artifact whose envelope is absent
/// exits 2 with the diagnostic naming the requirement (r_glm task-1.3
/// finding: the required arm of the verify-only wording was untested).
#[test]
fn verify_on_required_missing_envelope_names_the_requirement() {
    let (deploy, artifact) = deploy_artifact(&fixture().signed_required_job);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--verify"], &[]);
    assert_eq!(
        code, 2,
        "--verify on required-without-envelope must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("required") && combined.contains("signature"),
        "the verify-only diagnostic must name the required signature: {combined}"
    );
}

/// `--verify` runs the verification chain without booting: on the
/// signed fixture it exits 0 printing the algorithm and the manifest's
/// key fingerprint; on an unsigned artifact it exits 2 naming the
/// missing envelope.
#[test]
fn verify_flag_roundtrip_and_output() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &["--verify"], &[]);
    assert_eq!(
        code, 0,
        "--verify on a signed artifact must succeed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("algorithm: ed25519ph"),
        "stdout must name the algorithm: {stdout}"
    );
    assert!(!stderr.contains("context started"), "no boot: {stderr}");

    // The printed fingerprint is exactly the manifest signing block's.
    let (manifest_code, manifest_out, _) =
        common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    assert_eq!(manifest_code, 0, "--manifest must still work when signed");
    let manifest: serde_json::Value =
        serde_json::from_str(manifest_out.trim()).expect("stdout is manifest JSON");
    let fingerprint = manifest["signing"]["key_fingerprint"]
        .as_str()
        .expect("signed manifest records the key fingerprint");
    assert!(
        stdout.contains(fingerprint),
        "--verify must print the manifest fingerprint {fingerprint}: {stdout}"
    );

    // Unsigned side: no envelope → exit 2 naming its absence.
    let (unsigned_deploy, unsigned_artifact) = deploy_artifact(&fixture().route);
    let (unsigned_code, _, unsigned_stderr) = common::run_binary(
        unsigned_deploy.path(),
        &unsigned_artifact,
        &["--verify"],
        &[],
    );
    assert_eq!(
        unsigned_code, 2,
        "--verify without an envelope must exit 2;\nstderr:\n{unsigned_stderr}"
    );
    assert!(
        unsigned_stderr.contains("no signature envelope present"),
        "the diagnostic must name the missing envelope: {unsigned_stderr}"
    );
}

/// `--verify` stays exclusive with the other artifact flags: combined
/// with `--manifest` (either order) it exits 2 with the
/// duplicate-exclusive diagnostic and prints no manifest.
#[test]
fn verify_stays_exclusive() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    for argv in [
        vec!["--verify", "--manifest"],
        vec!["--manifest", "--verify"],
    ] {
        let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &argv, &[]);
        assert_eq!(
            code, 2,
            "argv {argv:?} must exit 2;\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let combined = format!("{stdout}{stderr}");
        assert!(
            combined.contains("mutually exclusive"),
            "argv {argv:?} must carry the duplicate-exclusive diagnostic: {combined}"
        );
        assert!(
            !stdout.contains("manifest_schema"),
            "argv {argv:?} must not print the manifest: {stdout}"
        );
        assert!(
            !combined.contains("context started"),
            "argv {argv:?} must not boot: {combined}"
        );
    }
}

/// The job-side fixture of the boundedness battery (openspec r5routesrv
/// Task 1.6): write `route.yaml` — a `direct:ping` route answering
/// `pong`; the job-safety allowlist is `{direct, seda, log, mock}`, so
/// no timer and no rest — plus the one-shot `job.job.yaml` referencing
/// it through `routeFiles`, into a fresh tempdir on the fixture root.
/// Returns the tempdir; the compile reads both documents from it.
fn job_doc_with_direct_route() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("camel-routesrv-job-")
        .tempdir_in(fixture_root())
        .expect("job fixture tempdir");
    std::fs::write(
        dir.path().join("route.yaml"),
        "\
routes:
  - id: ping-route
    from: direct:ping
    steps:
      - set_body: \"pong\"
",
    )
    .expect("write route.yaml");
    std::fs::write(
        dir.path().join("job.job.yaml"),
        "\
execute:
  mode: one-shot
  send:
    to: direct:ping
    body: hi
  timeout: 10s
routeFiles:
  - route.yaml
",
    )
    .expect("write job.job.yaml");
    dir
}

/// A compiled job artifact is bounded: with no signal ever sent it
/// completes the one-shot send by itself, exits 0 within 120 s, its
/// stdout carries the `"outcome": "Completed"` job report JSON, and its
/// `--manifest` pins `artifact_kind` `job` with an empty `listeners`
/// array (openspec r5routesrv, cli-compile "Job artifacts stay
/// bounded").
#[test]
fn job_artifact_exits_without_signal() {
    child_guard();
    let dir = job_doc_with_direct_route();
    let output = compile(dir.path(), "job.job.yaml", "job.bin", &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "job document must compile: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (deploy, artifact) = deploy_artifact(&dir.path().join("job.bin"));
    let mut child = spawn_child(
        "job_artifact_exits_without_signal",
        deploy.path(),
        &artifact,
        &[],
        &[],
    );
    let drained = spawn_drained(&mut child);
    let code = wait_exit_code(&mut child, Duration::from_secs(120));
    let stdout = drained.out.lock().expect("stdout lock").clone();
    assert_eq!(
        code,
        0,
        "the one-shot job must self-exit 0 with no signal ever sent;\n{}",
        drained.captured()
    );
    assert!(
        stdout.contains("\"outcome\": \"Completed\""),
        "stdout must carry the Completed job report:\n{stdout}"
    );
    // "Never serving" pinned on the manifest (`--manifest` never boots,
    // so no `CAMEL_*` env scrub is needed): a job artifact declares no
    // listeners.
    let (mcode, mstdout, mstderr) =
        common::run_binary(deploy.path(), &artifact, &["--manifest"], &[]);
    assert_eq!(
        mcode, 0,
        "--manifest exits 0;\nstdout:\n{mstdout}\nstderr:\n{mstderr}"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(mstdout.trim()).expect("--manifest stdout is not JSON");
    assert_eq!(
        manifest["artifact_kind"], "job",
        "manifest artifact_kind must be `job`: {manifest}"
    );
    let listeners = manifest["listeners"].as_array();
    assert!(
        listeners.is_some_and(|l| l.is_empty()),
        "a job manifest must carry an empty listeners array: {manifest}"
    );
}

/// Envelope verification precedes listener binding (openspec r5routesrv,
/// cli-compile "Envelope verification precedes listener binding"): a
/// required-signature artifact whose ENVELOPE bytes are corrupted — the
/// artifact trailer stays valid, so `decode_artifact` succeeds and boot
/// verification is the failing step — fails closed with exit 2 naming
/// signature verification, and the child never binds the declared
/// listener: the test binds `TcpListener` on the declared port BEFORE
/// the spawn and holds it across the child's entire execution, so any
/// bind attempt would surface as [`BIND_RACE_MARK`]. No
/// [`with_bind_race_retry`] here: the held port is the witness — a bind
/// diagnostic would be the very failure the test detects, so retrying
/// would discard it.
#[test]
fn envelope_corruption_binds_no_listener() {
    let port = free_port();
    let src = tempfile::Builder::new()
        .prefix("camel-routesrv-sig-")
        .tempdir_in(fixture_root())
        .expect("source tempdir");
    std::fs::write(src.path().join("rest.yaml"), rest_listener_doc(port))
        .expect("write rest document");
    let seed_path = src.path().join("fixture-signing.key");
    std::fs::write(&seed_path, FIXTURE_SEED).expect("write synthetic signing seed");
    let output = compile_signed(src.path(), "rest.yaml", "rest.bin", &seed_path, true);
    assert_eq!(
        output.status.code(),
        Some(0),
        "signed compile must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Deploy the artifact WITH its mutable envelope copy and corrupt
    // ONLY the envelope: one flipped byte near its middle.
    let (deploy, artifact) = deploy_signed(&src.path().join("rest.bin"));
    let sig = sig_path_of(&artifact);
    let mut bytes = std::fs::read(&sig).expect("read envelope bytes");
    assert!(bytes.len() > 1, "envelope must have a flippable middle");
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    std::fs::write(&sig, bytes).expect("write corrupted envelope");

    // Held-listener witness: bound before the spawn, held across the
    // child's entire execution — "never bound" is distinguished from
    // "bound, then closed" because a bind attempt fails loudly.
    let witness = TcpListener::bind(("127.0.0.1", port)).expect("hold declared listener port");
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &[], &[]);
    assert_eq!(
        code, 2,
        "corrupted envelope must fail closed;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("compiled artifact signature verification failed"),
        "the diagnostic must name signature verification:\nstderr:\n{stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        !combined.contains(BIND_RACE_MARK),
        "the child must never have attempted the listener bind: {combined}"
    );
    // Drop the held listener only after the child exit is reaped
    // (`run_binary` returns post-reap).
    drop(witness);
}
// ---------------------------------------------------------------------------
// keypin Task 1.2: deployment truststore pin check + strip rule at both
// verify sites. Like the r4sign battery above, every case drives the REAL
// self-detect entry — the artifact binary itself — so boot and `--verify`
// both cross the shared trust policy. The signed cases reuse the shared
// OnceLock fixtures via the deploy-to-tempdir pattern; truststores are tiny
// hand-written text files in the per-test deploy directory. The tamper cases
// keep the deploy-copy discipline: shared fixtures are never mutated.
// ---------------------------------------------------------------------------

/// A different but well-formed pin, for truststores that must NOT match
/// the fixture manifest fingerprint.
const OTHER_PIN_HEX: &str = concat!(
    "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
    "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
);

/// Read the deployed signed artifact's manifest `key_fingerprint` (the
/// exact `blake3:<hex>` string the truststore must pin) via `--manifest`.
fn manifest_fingerprint(deploy: &Path, artifact: &Path) -> String {
    let (code, stdout, stderr) = common::run_binary(deploy, artifact, &["--manifest"], &[]);
    assert_eq!(
        code, 0,
        "--manifest must report the manifest;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is manifest JSON");
    manifest["signing"]["key_fingerprint"]
        .as_str()
        .expect("signed manifest records the key fingerprint")
        .to_string()
}

/// Read the deployed signed artifact's schema-5 freshness marker via
/// `--manifest` (keypin Task 2.2): the unix-seconds value the truststore
/// floor is compared against.
fn manifest_marker(deploy: &Path, artifact: &Path) -> u64 {
    let (code, stdout, stderr) = common::run_binary(deploy, artifact, &["--manifest"], &[]);
    assert_eq!(
        code, 0,
        "--manifest must report the manifest;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout is manifest JSON");
    manifest["signing"]["freshness"]
        .as_u64()
        .expect("a schema-5 signing block carries the freshness marker")
}

/// Write a tiny truststore text file into the deploy directory and
/// return its path as a command-line argument string.
fn write_truststore(deploy: &Path, name: &str, body: &str) -> String {
    let path = deploy.join(name);
    std::fs::write(&path, body).expect("write truststore file");
    path.to_string_lossy().into_owned()
}

/// A signed artifact whose manifest key fingerprint is pinned boots AND
/// passes `--verify --truststore` (exit 0 both).
#[test]
fn pinned_key_verifies_under_truststore() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
    let truststore = write_truststore(deploy.path(), "pins.keys", &format!("{fingerprint}\n"));

    let (boot_code, boot_stdout, boot_stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--truststore", &truststore],
        &[],
    );
    assert_eq!(
        boot_code, 0,
        "the pinned artifact must boot;\nstdout:\n{boot_stdout}\nstderr:\n{boot_stderr}"
    );

    let (verify_code, verify_stdout, verify_stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--verify", "--truststore", &truststore],
        &[],
    );
    assert_eq!(
        verify_code, 0,
        "--verify under a pinning truststore must succeed;\nstdout:\n{verify_stdout}\nstderr:\n{verify_stderr}"
    );
    assert!(
        !format!("{verify_stdout}{verify_stderr}").contains("context started"),
        "--verify must not boot: {verify_stdout}"
    );
}

/// (a) With no argument, `CAMEL_TRUSTSTORE` supplies a pinning
/// truststore and `--verify` exits 0. (b) With BOTH set, the argument
/// wins: a comments-only argument truststore pins nothing, so `--verify`
/// exits 2 with the `truststore-pin` diagnostic — proving the argument
/// path was used, not the env one.
#[test]
fn env_truststore_supplies_and_argument_wins() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
    let pinning = write_truststore(deploy.path(), "env-pins.keys", &format!("{fingerprint}\n"));
    let empty = write_truststore(deploy.path(), "arg-empty.keys", "# no pins\n\n");

    let (code, stdout, stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--verify"],
        &[("CAMEL_TRUSTSTORE", &pinning)],
    );
    assert_eq!(
        code, 0,
        "the env-supplied truststore must pin the key;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let (code, stdout, stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--verify", "--truststore", &empty],
        &[("CAMEL_TRUSTSTORE", &pinning)],
    );
    assert_eq!(
        code, 2,
        "the argument truststore must win over the env var;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("truststore-pin"),
        "the diagnostic must name the pin step: {stderr}"
    );
}

/// `CAMEL_TRUSTSTORE` also supplies the BARE boot path (no
/// `--truststore`, no `--verify`): a store pinning the deployed
/// `signed_job` fingerprint boots, and a store pinning nothing refuses
/// the same boot with `truststore-pin` — proving the env var is read on
/// the boot chain, not only by `--verify`.
#[test]
fn env_truststore_supplies_boot_without_argument() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
    let pinning = write_truststore(
        deploy.path(),
        "env-boot-pins.keys",
        &format!("{fingerprint}\n"),
    );
    let empty = write_truststore(deploy.path(), "env-boot-empty.keys", "# no pins\n\n");

    let (code, stdout, stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &[],
        &[("CAMEL_TRUSTSTORE", &pinning)],
    );
    assert_eq!(
        code, 0,
        "the env-supplied truststore must let the pinned artifact boot;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let (code, stdout, stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &[],
        &[("CAMEL_TRUSTSTORE", &empty)],
    );
    assert_eq!(
        code, 2,
        "an env-supplied store pinning nothing must refuse the boot;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        format!("{stdout}{stderr}").contains("truststore-pin"),
        "the boot diagnostic must name the pin step: {stdout}{stderr}"
    );
}

/// A truststore pinning a DIFFERENT valid fingerprint fails closed at
/// BOTH sites: boot and `--verify` exit 2 with `truststore-pin` naming
/// the manifest fingerprint, and nothing boots.
#[test]
fn unpinned_key_fails_closed_at_boot_and_verify() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
    let truststore = write_truststore(
        deploy.path(),
        "other.keys",
        &format!("blake3:{OTHER_PIN_HEX}\n"),
    );

    let (boot_code, boot_stdout, boot_stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--truststore", &truststore],
        &[],
    );
    assert_eq!(
        boot_code, 2,
        "an unpinned key must refuse the boot;\nstdout:\n{boot_stdout}\nstderr:\n{boot_stderr}"
    );
    let boot_all = format!("{boot_stdout}{boot_stderr}");
    assert!(
        boot_all.contains("truststore-pin") && boot_all.contains(&fingerprint),
        "the boot diagnostic must name the pin step and the manifest fingerprint: {boot_all}"
    );
    assert!(
        !boot_all.contains("context started"),
        "no boot on an unpinned key: {boot_all}"
    );

    let (verify_code, _, verify_stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--verify", "--truststore", &truststore],
        &[],
    );
    assert_eq!(
        verify_code, 2,
        "--verify on an unpinned key must exit 2;\nstderr:\n{verify_stderr}"
    );
    assert!(
        verify_stderr.contains("truststore-pin") && verify_stderr.contains(&fingerprint),
        "the verify diagnostic must name the pin step and the fingerprint: {verify_stderr}"
    );
}

/// The strip rule at BOTH sites (required bit NOT set): a signed
/// artifact deployed WITHOUT its envelope must not boot or verify under
/// a truststore — exit 2 with `truststore-pin` naming the manifest
/// fingerprint, the same diagnostic the required-bit path would print
/// with no truststore only on the no-truststore path.
#[test]
fn stripped_envelope_fails_closed_under_truststore() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
    let truststore = write_truststore(deploy.path(), "pins.keys", &format!("{fingerprint}\n"));
    std::fs::remove_file(sig_path_of(&artifact)).expect("strip the envelope");

    let (boot_code, boot_stdout, boot_stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--truststore", &truststore],
        &[],
    );
    assert_eq!(
        boot_code, 2,
        "a stripped envelope must refuse the boot under a truststore;\nstdout:\n{boot_stdout}\nstderr:\n{boot_stderr}"
    );
    let boot_all = format!("{boot_stdout}{boot_stderr}");
    assert!(
        boot_all.contains("truststore-pin") && boot_all.contains(&fingerprint),
        "the boot diagnostic must name the pin step and the fingerprint: {boot_all}"
    );
    assert!(
        !boot_all.contains("context started"),
        "no boot after stripping: {boot_all}"
    );

    let (verify_code, _, verify_stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--verify", "--truststore", &truststore],
        &[],
    );
    assert_eq!(
        verify_code, 2,
        "--verify on a stripped envelope must exit 2 under a truststore;\nstderr:\n{verify_stderr}"
    );
    assert!(
        verify_stderr.contains("truststore-pin") && verify_stderr.contains(&fingerprint),
        "the verify diagnostic must name the pin step and the fingerprint: {verify_stderr}"
    );
}

/// The strip rule with the required bit SET (keypin Phase-1 review gap):
/// a `--sign --require-signature` artifact deployed without its envelope
/// under a truststore still hits the strip rule FIRST at BOTH sites —
/// boot and `--verify` exit 2 with `truststore-pin` naming the manifest
/// fingerprint, never the required-signature diagnostic.
#[test]
fn stripped_envelope_with_required_bit_fails_closed_under_truststore() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_required_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
    let truststore = write_truststore(deploy.path(), "pins.keys", &format!("{fingerprint}\n"));
    std::fs::remove_file(sig_path_of(&artifact)).expect("strip the envelope");

    let (boot_code, boot_stdout, boot_stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--truststore", &truststore],
        &[],
    );
    assert_eq!(
        boot_code, 2,
        "a stripped required-signature envelope must refuse the boot under a truststore;\nstdout:\n{boot_stdout}\nstderr:\n{boot_stderr}"
    );
    let boot_all = format!("{boot_stdout}{boot_stderr}");
    assert!(
        boot_all.contains("truststore-pin") && boot_all.contains(&fingerprint),
        "the boot diagnostic must name the pin step and the fingerprint: {boot_all}"
    );
    assert!(
        !boot_all.contains("context started"),
        "no boot after stripping a required envelope: {boot_all}"
    );

    let (verify_code, _, verify_stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--verify", "--truststore", &truststore],
        &[],
    );
    assert_eq!(
        verify_code, 2,
        "--verify on a stripped required envelope must exit 2 under a truststore;\nstderr:\n{verify_stderr}"
    );
    assert!(
        verify_stderr.contains("truststore-pin") && verify_stderr.contains(&fingerprint),
        "the verify diagnostic must name the pin step and the fingerprint: {verify_stderr}"
    );
}

/// Parse failures fail closed at boot: (a) a malformed line 2 exits 2
/// naming the path and `line 2` (step `truststore-parse`); (b) a missing
/// truststore file exits 2 naming the path.
#[test]
fn malformed_truststore_fails_closed_at_boot() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);

    let malformed = write_truststore(
        deploy.path(),
        "malformed.keys",
        &format!("{fingerprint}\nnot-a-pin\n"),
    );
    let (code, _, stderr) =
        common::run_binary(deploy.path(), &artifact, &["--truststore", &malformed], &[]);
    assert_eq!(
        code, 2,
        "a malformed truststore must refuse the boot;\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("truststore-parse")
            && stderr.contains(&malformed)
            && stderr.contains("line 2"),
        "the diagnostic must name the parse step, the path, and line 2: {stderr}"
    );

    let missing = deploy.path().join("absent.keys");
    let missing = missing.to_string_lossy().into_owned();
    let (code, _, stderr) =
        common::run_binary(deploy.path(), &artifact, &["--truststore", &missing], &[]);
    assert_eq!(
        code, 2,
        "a missing truststore file must refuse the boot;\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("truststore-parse") && stderr.contains(&missing),
        "the diagnostic must name the parse step and the path: {stderr}"
    );
}

/// An empty — comments-only — truststore is valid but pins nothing, so
/// a signed artifact fails its pin check: boot exits 2 with
/// `truststore-pin`.
#[test]
fn empty_truststore_pins_nothing_at_boot() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let truststore = write_truststore(deploy.path(), "empty.keys", "# no pins\n\n");

    let (code, stdout, stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--truststore", &truststore],
        &[],
    );
    assert_eq!(
        code, 2,
        "an empty truststore pins nothing, so the boot must fail;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("truststore-pin"),
        "the diagnostic must name the pin step: {stderr}"
    );
}

/// An unsigned artifact (schema 3, no signing block) ignores the
/// truststore entirely: it boots unchanged with one supplied — no pin
/// step, no new reads beyond the ignored path.
#[test]
fn unsigned_artifact_ignores_truststore() {
    let (deploy, artifact) = deploy_artifact(&fixture().job);
    let truststore = write_truststore(
        deploy.path(),
        "pins.keys",
        &format!("blake3:{OTHER_PIN_HEX}\n"),
    );

    let (code, stdout, stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--report", "report.json", "--truststore", &truststore],
        &[],
    );
    assert_eq!(
        code, 0,
        "an unsigned artifact must boot unchanged under a truststore;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(deploy.path().join("report.json")).expect("report written"),
    )
    .expect("report is JSON");
    assert_eq!(report["outcome"], "Completed", "report: {report}");
    assert!(
        !format!("{stdout}{stderr}").contains("truststore"),
        "no truststore step on an unsigned artifact: {stdout}{stderr}"
    );
}

// ---------------------------------------------------------------------------
// keypin Task 2.2: freshness floors. The decision and floor write share one
// lock-serialized boot critical section; `--verify` is a dry run. The cases
// reuse the shared schema-5 `signed_job` fixture via `deploy_signed` and read
// the signed marker from `--manifest`; truststores are hand-written tiny text
// files in the per-test deploy directory.
// ---------------------------------------------------------------------------

/// A marker below the recorded floor fails closed at BOTH sites (keypin
/// Task 2.2): boot and `--verify --truststore` exit 2 with the
/// `freshness-rollback` diagnostic naming the fingerprint and the floor.
#[test]
fn rollback_below_floor_fails_closed() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
    let marker = manifest_marker(deploy.path(), &artifact);
    let truststore = write_truststore(
        deploy.path(),
        "ahead.keys",
        &format!("{fingerprint} {}\n", marker + 1000),
    );

    let (boot_code, boot_stdout, boot_stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--truststore", &truststore],
        &[],
    );
    let floor = (marker + 1000).to_string();
    assert_eq!(
        boot_code, 2,
        "a marker below the floor must refuse the boot;\nstdout:\n{boot_stdout}\nstderr:\n{boot_stderr}"
    );
    let boot_all = format!("{boot_stdout}{boot_stderr}");
    assert!(
        boot_all.contains("freshness-rollback")
            && boot_all.contains(&fingerprint)
            && boot_all.contains(&floor),
        "the boot diagnostic must name the rollback step, the fingerprint, and the floor: {boot_all}"
    );
    assert!(
        !boot_all.contains("context started"),
        "no boot below the floor: {boot_all}"
    );

    let (verify_code, _, verify_stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--verify", "--truststore", &truststore],
        &[],
    );
    assert_eq!(
        verify_code, 2,
        "--verify below the floor must exit 2;\nstderr:\n{verify_stderr}"
    );
    assert!(
        verify_stderr.contains("freshness-rollback")
            && verify_stderr.contains(&fingerprint)
            && verify_stderr.contains(&floor),
        "the verify diagnostic must name the rollback step, the fingerprint, and the floor: {verify_stderr}"
    );
}

/// The first sight of a pinned key with no recorded floor boots and records
/// the artifact's marker as that key's floor (keypin Task 2.2).
#[test]
fn first_sight_records_floor() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
    let marker = manifest_marker(deploy.path(), &artifact);
    let truststore = write_truststore(deploy.path(), "first.keys", &format!("{fingerprint}\n"));

    let (code, stdout, stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--truststore", &truststore],
        &[],
    );
    assert_eq!(
        code, 0,
        "the first sight of the pinned key must boot;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let body = std::fs::read_to_string(&truststore).expect("read the rewritten truststore");
    let line = body
        .lines()
        .find(|line| line.trim_start().starts_with(&fingerprint))
        .expect("the pin survives the rewrite");
    assert!(
        line.ends_with(&marker.to_string()),
        "the first sight records the marker as the floor: {line:?} (marker {marker})"
    );
}

/// The floor never decreases (keypin Task 2.2): an equal floor boots with
/// the truststore bytes unchanged; a lower floor rises to the marker.
#[test]
fn floor_never_decreases() {
    // (a) floor == marker: boot succeeds and is not rewritten.
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
    let marker = manifest_marker(deploy.path(), &artifact);
    let equal = write_truststore(
        deploy.path(),
        "equal.keys",
        &format!("{fingerprint} {marker}\n"),
    );
    let before = std::fs::read(&equal).expect("read the equal-floor store");
    let (code, stdout, stderr) =
        common::run_binary(deploy.path(), &artifact, &["--truststore", &equal], &[]);
    assert_eq!(
        code, 0,
        "a marker at the floor boots;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let after = std::fs::read(&equal).expect("read the equal-floor store");
    assert_eq!(before, after, "an equal floor is not rewritten");

    // (b) floor = marker - 5: boot succeeds and the floor becomes marker.
    let lower = write_truststore(
        deploy.path(),
        "lower.keys",
        &format!("{fingerprint} {}\n", marker - 5),
    );
    let (code, stdout, stderr) =
        common::run_binary(deploy.path(), &artifact, &["--truststore", &lower], &[]);
    assert_eq!(
        code, 0,
        "a marker above the floor boots;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let body = std::fs::read_to_string(&lower).expect("read the raised-floor store");
    let line = body
        .lines()
        .find(|line| line.trim_start().starts_with(&fingerprint))
        .expect("the pin survives the rewrite");
    assert!(
        line.ends_with(&marker.to_string()),
        "the floor rises to the marker: {line:?} (marker {marker})"
    );
}

/// `--verify` is a dry run (keypin Task 2.2): a marker above the floor
/// exits 0 and the truststore file is byte-identical before and after.
#[test]
fn verify_is_a_dry_run() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
    let marker = manifest_marker(deploy.path(), &artifact);
    let truststore = write_truststore(
        deploy.path(),
        "dry-run.keys",
        &format!("{fingerprint} {}\n", marker - 5),
    );
    let before = std::fs::read(&truststore).expect("read the store before verify");

    let (code, stdout, stderr) = common::run_binary(
        deploy.path(),
        &artifact,
        &["--verify", "--truststore", &truststore],
        &[],
    );
    assert_eq!(
        code, 0,
        "--verify above the floor must exit 0;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let after = std::fs::read(&truststore).expect("read the store after verify");
    assert_eq!(
        before, after,
        "--verify must never lock or write the truststore (dry run)"
    );
}

/// A boot that must record a floor against an unwritable truststore fails
/// closed with `truststore-update` (keypin Task 2.2). The read-only
/// directory is restored on every path, including failure.
#[test]
fn unwritable_truststore_fails_closed() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        /// Restores writable permissions on the deploy directory even if
        /// the test fails part way (Drop runs on unwind too).
        struct Writable(PathBuf);
        impl Drop for Writable {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
        }

        let (deploy, artifact) = deploy_signed(&fixture().signed_job);
        let fingerprint = manifest_fingerprint(deploy.path(), &artifact);
        let truststore =
            write_truststore(deploy.path(), "read-only.keys", &format!("{fingerprint}\n"));
        let _restore = Writable(deploy.path().to_path_buf());
        std::fs::set_permissions(deploy.path(), std::fs::Permissions::from_mode(0o555))
            .expect("chmod read-only");

        let (code, stdout, stderr) = common::run_binary(
            deploy.path(),
            &artifact,
            &["--truststore", &truststore],
            &[],
        );
        assert_eq!(
            code, 2,
            "an unwritable truststore must refuse the boot;\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            format!("{stdout}{stderr}").contains("truststore-update"),
            "the diagnostic must name the update step: {stdout}{stderr}"
        );
    }
}

/// A schema-5 signed artifact with no truststore skips the freshness step
/// entirely (keypin Task 2.2): the plain R4 chain boots it unchanged.
#[test]
fn freshness_without_truststore_skips_the_step() {
    let (deploy, artifact) = deploy_signed(&fixture().signed_job);
    let (code, stdout, stderr) = common::run_binary(deploy.path(), &artifact, &[], &[]);
    assert_eq!(
        code, 0,
        "a signed artifact boots on the plain R4 chain;\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let all = format!("{stdout}{stderr}");
    assert!(
        !all.contains("freshness"),
        "no freshness step without a truststore: {all}"
    );
    assert!(
        !all.contains("truststore"),
        "no truststore step without a truststore: {all}"
    );
}
