//! Hermetic surreal state e2e (surreal-state-tier task 2.6): one
//! fixture document exercises the whole state tier in-memory —
//! `surreal:` prepare (`REMOVE TABLE IF EXISTS order` first: the
//! clean-first idiom — plain `REMOVE TABLE` is NOT absence-tolerant
//! on surrealdb 3.x and fails on a fresh boot), one `direct:` send
//! the route persists through the `surrealdb:create?datasource=statedb&table=order`
//! producer, and a `surreal` validate target with `columns` and a poll
//! `deadline` proving the record landed. `mem://` isolates per boot,
//! so no external SurrealDB and no cleanup.
//!
//! The boot mirrors [`wasm_boot_test`] (the crate's full-boot e2e
//! precedent): the fixture directory IS the project — `Camel.toml`
//! (the mem datasource), `routes/order.yaml` (the direct →
//! `surrealdb:create` route), and the scenario document — booted
//! through [`boot_scenario`], the same composition root `camel run`
//! uses. The sql full-boot e2e lives in camel-cli and stays owned by
//! the integration-sql workflow; this crate has no separate e2e
//! feature, so the harness `surreal` gate carries the binary.
#![cfg(feature = "surreal")]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use camel_component_api::test_support::acquire_deadline;
use camel_integration_test::runner::fill_bind_vars;
use camel_integration_test::{
    DirectStimulus, DocumentOutcome, LayeredEnv, ScenarioFailure, ScenarioVars, ScenarioVerdict,
    ambient_std, boot_scenario, parse_scenario_document, run_scenario_document,
};

/// The fixture project root: `Camel.toml` (the mem surreal
/// datasource), `routes/order.yaml` (the direct consumer feeding the
/// `surrealdb:create` producer), and `order.test.yaml` (the
/// prepare → send → validate document).
fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/surreal_state")
}

/// Boots the fixture document through the full composition root, runs
/// the prepare → send → validate document through the boot's own
/// datasource catalog and the `DirectStimulus` router, and records a
/// shutdown failure into the outcome's post-verdict `final_failure`
/// slot (the boot-owning-caller contract).
async fn boot_run_and_shutdown(doc_path: &Path, root: &Path) -> DocumentOutcome {
    let doc = parse_scenario_document(doc_path).expect("fixture document must parse");
    let env = LayeredEnv::new(
        doc.env.clone().unwrap_or_default(),
        BTreeMap::new(),
        doc.env_passthrough.clone().unwrap_or_default(),
        ambient_std(),
    );
    let run = boot_scenario(&doc, root, &env)
        .await
        .expect("the mem-surreal project must boot");
    let catalog = run.boot.datasource_catalog();
    let ctx = Arc::new(tokio::sync::Mutex::new(run.ctx));
    let router = common::router_for("direct:order", DirectStimulus::new(Arc::clone(&ctx)));
    let wired = common::wired_refs(&doc);
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired, &router, &mut vars);
    let mut outcome = run_scenario_document(&doc, &router, &mut vars, Some(&catalog)).await;

    let mut guard = acquire_deadline(
        &ctx,
        "scenario ctx (surreal_state_test)",
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

/// The e2e: the fixture document runs to a passing verdict — every
/// action Ok (prepare executed, send delivered, validate observed the
/// persisted record inside the poll window), the whole-document
/// verdict Pass, and a clean teardown.
#[tokio::test]
async fn surreal_state_e2e_prepare_route_validate() {
    let root = fixture_root();
    let doc_path = root.join("order.test.yaml");
    let outcome = boot_run_and_shutdown(&doc_path, &root).await;
    assert!(
        outcome.per_action.iter().all(Result::is_ok),
        "every action must pass: {outcome:?}"
    );
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "prepare → route write → validate must pass end to end: {outcome:?}"
    );
    assert!(
        outcome.final_failure.is_none(),
        "shutdown must be clean: {outcome:?}"
    );
}
