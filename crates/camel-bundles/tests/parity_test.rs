//! Parity and teardown suite for the extracted `camel run` cascade
//! (ADR-0069 section 10, task 1.3).
//!
//! Proves the extraction is behavior-neutral:
//!
//! - two consecutive [`boot`] cycles on the same fixture register identical
//!   component-name sets and read identical per-bundle config keys,
//! - [`BootHandle::shutdown`] tears a boot down such that a second
//!   configure + boot on the same fixture succeeds (no poisoned pool or
//!   process-global state),
//! - two boots kept up concurrently in one process never panic on tracing
//!   double-init — the `try_init()` guard in `init_tracing_subscriber`
//!   (`configure_context_with_beans`), the watch-reload regression lock.

use std::path::Path;

use camel_bundles::{BootHandle, boot};
use camel_config::config::CamelConfig;
use camel_core::CamelContext;

/// Root of the parity fixture project (parent of its Camel.toml), shaped
/// like the `project_root` the CLI resolves for a real project.
fn fixture_root() -> String {
    format!("{}/tests/fixtures/parity", env!("CARGO_MANIFEST_DIR"))
}

/// Path of the parity fixture project's Camel.toml.
fn fixture_config_path() -> String {
    format!("{}/Camel.toml", fixture_root())
}

/// One full CLI-shaped boot cycle: load the fixture config, prepare the
/// context with `configure_context_with_beans` (the caller entry `camel run`
/// uses), and run [`boot`]. Panics name the step that failed, so an `Ok`
/// return asserts that step of the parity contract.
async fn boot_cycle() -> (CamelContext, BootHandle, CamelConfig) {
    let config = CamelConfig::from_file(&fixture_config_path())
        .unwrap_or_else(|e| panic!("fixture {}: {e}", fixture_config_path()));
    let mut ctx = CamelConfig::configure_context_with_beans(&config, None)
        .await
        .expect("configure_context_with_beans must succeed");
    let handle = boot(&mut ctx, &config, Path::new(&fixture_root()))
        .await
        .expect("boot must register the cascade");
    (ctx, handle, config)
}

/// Registered component-name set of a booted context, sorted for a stable
/// comparison (registry iteration order is a HashMap artifact).
fn sorted_component_schemes(ctx: &CamelContext) -> Vec<String> {
    let mut names = ctx.registry().metadata_schemes();
    names.sort();
    names
}

/// Per-bundle config keys the cascade reads `[components.<key>]` tables
/// from, sorted for a stable comparison.
fn sorted_config_keys(config: &CamelConfig) -> Vec<String> {
    let mut keys: Vec<String> = config.components.raw.keys().cloned().collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn two_boots_register_identical_sets() {
    let (ctx_a, handle_a, config_a) = boot_cycle().await;
    let (ctx_b, handle_b, config_b) = boot_cycle().await;

    // Guard against a vacuous pass: the fixture must have registered the
    // bundles it configures, and must carry their tables.
    let schemes_a = sorted_component_schemes(&ctx_a);
    for scheme in ["http", "file", "container", "template"] {
        assert!(
            schemes_a.contains(&scheme.to_string()),
            "fixture boot must register '{scheme}': {schemes_a:?}"
        );
        assert!(
            config_a.components.raw.contains_key(scheme),
            "fixture must carry [components.{scheme}]"
        );
    }

    assert_eq!(
        schemes_a,
        sorted_component_schemes(&ctx_b),
        "two boots of the same fixture must register identical component sets"
    );
    assert_eq!(
        sorted_config_keys(&config_a),
        sorted_config_keys(&config_b),
        "two config loads must yield identical per-bundle config keys"
    );

    let mut ctx_a = ctx_a;
    let mut ctx_b = ctx_b;
    handle_a
        .shutdown(&mut ctx_a)
        .await
        .expect("first teardown must succeed");
    handle_b
        .shutdown(&mut ctx_b)
        .await
        .expect("second teardown must succeed");
}

/// Teardown contract: [`BootHandle::shutdown`] after a boot closes pools
/// such that a second configure + boot on the same fixture succeeds. The
/// bite exercised here is pool/global-state poisoning: a JMS/CXF pool that
/// refused teardown or wedged process-global state would fail the first
/// shutdown or the second boot. It is not a socket-leak probe: `boot`
/// stops before route start (http binds happen at `HttpConsumer::start`),
/// so this suite never opens a socket; a leaked-bind regression is
/// cross-process and out of this suite's scope.
#[tokio::test]
async fn shutdown_then_boot_succeeds() {
    let (mut ctx, handle, _config) = boot_cycle().await;
    handle
        .shutdown(&mut ctx)
        .await
        .expect("first teardown must succeed");

    // Second configure + boot on the same fixture: only reaches Ok if the
    // first teardown released the exclusive resources boot created
    // (JMS/CXF bridge pools) and left no poisoned process-global state.
    let (mut ctx_second, handle_second, _config_second) = boot_cycle().await;
    handle_second
        .shutdown(&mut ctx_second)
        .await
        .expect("second teardown must succeed");
}

/// Tracing double-init regression lock (ADR-0069): `camel run --watch`
/// re-runs `configure_context_with_beans` + [`boot`] in one process on
/// reload. `init_tracing_subscriber` must guard its install with
/// `try_init()` — the second call takes the warning path by construction;
/// a swap to a panicking global init fails this test.
#[tokio::test]
async fn consecutive_boots_do_not_panic_tracing() {
    // First boot.
    let (mut ctx_a, handle_a, _config_a) = boot_cycle().await;

    // Second configure + boot WHILE the first is still up — the
    // watch-reload shape. Both boots stay up concurrently.
    let (mut ctx_b, handle_b, _config_b) = boot_cycle().await;

    // Tear both down cleanly: no panic anywhere, every shutdown Ok.
    handle_a
        .shutdown(&mut ctx_a)
        .await
        .expect("first shutdown must succeed");
    handle_b
        .shutdown(&mut ctx_b)
        .await
        .expect("second shutdown must succeed");
}
