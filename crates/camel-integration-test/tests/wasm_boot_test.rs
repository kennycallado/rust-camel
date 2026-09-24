//! Wasm boot-root parity (rc-l3zrr): a scenario document invoked as a
//! bare relative filename (`cd project && camel test wdoc.test.yaml`)
//! yields an empty boot root (`Path::parent()` of the filename is
//! `Some("")`), which used to reach the wasm bundle as an empty base
//! dir whose `canonicalize()` failed (`failed to resolve base
//! directory: `). The boot normalizes the empty root to `.` — the same
//! empty-parent rule `camel run` applies (try_canonical_project_root)
//! — so a `wasm:` route boots identically under both invocations.
//!
//! Both documents here run `direct:start → wasm:echo.wasm →
//! log:wasmout` with a `logs.contains` assertion on the body: the
//! asserted line exists only if the exchange traversed the wasm step,
//! so the log assertion doubles as the executed-step proof. That proof
//! is traversal-only: the echo guest is a pass-through, so the
//! assertion shows the exchange crossed the wasm endpoint without
//! error — it does not assert a guest-side transformation.
#![cfg(feature = "wasm")]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use camel_component_api::test_support::acquire_deadline;
use camel_integration_test::runner::fill_bind_vars;
use camel_integration_test::{
    DirectStimulus, DocumentOutcome, LayeredEnv, ScenarioFailure, ScenarioVars, ScenarioVerdict,
    ambient_std, boot_scenario, ensure_capture_subscriber, parse_scenario_document,
    run_scenario_document,
};
use tokio::sync::Mutex;

use common::lock_run;

/// Restores the process working directory on drop: the empty-root test
/// holds the defect's CWD premise (the project root as CWD) for the
/// whole run, and the premise must be undone even when an assertion
/// panics — a test thread dying inside the foreign CWD would poison
/// every later relative-path operation in the binary.
struct CwdGuard(PathBuf);

impl Drop for CwdGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.0).expect("the saved working directory must restore");
    }
}

/// Writes the scenario project into `dir`: an empty `Camel.toml`, the
/// wasm route file under `routes/`, the echo guest at the project root
/// (the route's `wasm:echo.wasm` resolves against the boot root's base
/// dir), and the scenario document declaring the route file plus the
/// body-marker log assertion.
fn write_project(dir: &Path) {
    std::fs::write(dir.join("Camel.toml"), "").expect("write Camel.toml");
    std::fs::create_dir(dir.join("routes")).expect("create routes dir");
    std::fs::write(
        dir.join("routes/wasm.yaml"),
        "routes:\n\
         \x20 - id: wasm-echo\n\
         \x20   from: direct:start\n\
         \x20   steps:\n\
         \x20     - to: wasm:echo.wasm\n\
         \x20     - to: log:wasmout\n",
    )
    .expect("write route file");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm/echo.wasm"),
        dir.join("echo.wasm"),
    )
    .expect("copy wasm guest");
    std::fs::write(
        dir.join("wasm.test.yaml"),
        r#"
routeFiles: [routes/wasm.yaml]
scenario:
- send:
    to: direct:start
    body: hello-wasm
logs:
  contains: ['hello-wasm']
"#,
    )
    .expect("write scenario document");
}

/// Boots the document at `doc_path` from the caller-chosen `root` (the
/// parameter under test), runs the send + logs document through the
/// `DirectStimulus` router, and records a shutdown failure into the
/// outcome's post-verdict `final_failure` slot (the boot-owning-caller
/// contract). The caller holds [`common::RUN_LOCK`]: the harness's
/// log-capture windows are process-global.
async fn boot_run_and_shutdown(doc_path: &Path, root: &Path) -> DocumentOutcome {
    ensure_capture_subscriber();
    let doc = parse_scenario_document(doc_path).expect("document must load");
    let env = LayeredEnv::new(
        doc.env.clone().unwrap_or_default(),
        BTreeMap::new(),
        doc.env_passthrough.clone().unwrap_or_default(),
        ambient_std(),
    );
    let run = boot_scenario(&doc, root, &env).await.expect(
        "the wasm boot must succeed (empty root: `failed to resolve base \
         directory: ` is the rc-l3zrr defect)",
    );
    let ctx = Arc::new(Mutex::new(run.ctx));
    let router = common::router_for("direct:start", DirectStimulus::new(Arc::clone(&ctx)));
    let wired = common::wired_refs(&doc);
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired, &router, &mut vars);
    let mut outcome = run_scenario_document(&doc, &router, &mut vars, None).await;

    let mut guard = acquire_deadline(
        &ctx,
        "scenario ctx (wasm_boot_test)",
        Duration::from_secs(10),
    )
    .await;
    if let Err(e) = run.boot.shutdown(&mut guard).await {
        outcome.final_failure = Some(ScenarioFailure::ShutdownFailure {
            message: e.to_string(),
        });
    }
    outcome
}

/// Asserts the full-pass shape shared by both boot-root shapes: every
/// executed action Ok, the whole-document verdict Pass (send executed,
/// the logs window saw the body marker), and a clean teardown.
fn assert_all_pass(outcome: &DocumentOutcome) {
    assert!(
        outcome.per_action.iter().all(Result::is_ok),
        "every action must pass: {outcome:?}"
    );
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the wasm route must execute end to end: {outcome:?}"
    );
    assert!(
        outcome.logs_failure.is_none(),
        "the logs assertion must pass: {outcome:?}"
    );
    assert!(
        outcome.final_failure.is_none(),
        "shutdown must be clean: {outcome:?}"
    );
}

/// The defect premise (rc-l3zrr): the document named as a bare
/// relative filename from the project root — the boot root arrives
/// empty, and every empty-root join resolves against the process CWD.
/// The test therefore holds the CWD premise under [`common::RUN_LOCK`]
/// (chdir into the tempdir project for the whole run, restore via
/// drop guard): with the CWD elsewhere the empty root's `Camel.toml`
/// join and the `.` base dir would resolve against the wrong directory
/// and the test would fail for the wrong reason.
#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn empty_boot_root_boots_wasm_route_and_executes_step() {
    let _run_guard = lock_run();
    let dir = tempfile::tempdir().expect("temp dir");
    write_project(dir.path());

    let previous = std::env::current_dir().expect("read current dir");
    std::env::set_current_dir(dir.path()).expect("chdir into the project root");
    let _cwd_guard = CwdGuard(previous);

    let root = Path::new("");
    // `Path::new("").join(..)` is the bare relative filename — exactly
    // what the CLI hands over for `camel test wdoc.test.yaml`.
    let doc_path = root.join("wasm.test.yaml");
    let outcome = boot_run_and_shutdown(&doc_path, root).await;
    assert_all_pass(&outcome);
}

/// The absolute-root twin: same project, no CWD change, the boot root
/// is the tempdir's absolute path — the pre-existing working shape,
/// unchanged by the empty-root normalization.
#[allow(clippy::await_holding_lock)] // RUN_LOCK serializes document runs; the guard outlives the run
#[tokio::test]
async fn absolute_boot_root_wasm_boot_unchanged() {
    let _run_guard = lock_run();
    let dir = tempfile::tempdir().expect("temp dir");
    write_project(dir.path());

    let root = dir.path().to_path_buf();
    let doc_path = root.join("wasm.test.yaml");
    let outcome = boot_run_and_shutdown(&doc_path, &root).await;
    assert_all_pass(&outcome);
}
