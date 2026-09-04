//! The embedded FULL-tier scenario boot (ADR-0069 sections 4, 5, 10).
//!
//! [`boot_scenario`] boots the real composition root for one scenario
//! document: the sealed config load (pinned profile, no ambient
//! `CAMEL_*` overrides, `${env:}` through the layered environment),
//! context preparation through `camel_config`, the `camel run`
//! component-bundle cascade through `camel_bundles::boot`, the
//! document's route source (file placeholders resolve through the
//! layered environment, never the process environment), and
//! `ctx.start()`.
//!
//! Partners are NOT owned here: the caller constructs them before the
//! boot (bind `127.0.0.1:0`), builds the harness-provisioned map into
//! the [`LayeredEnv`] passed in, and tears the partners down after
//! [`BootHandle::shutdown`]. Route stimulus for `direct:` endpoints
//! rides the caller's router through
//! [`DirectStimulus`](crate::adapters::DirectStimulus).

use std::path::Path;

use camel_api::CamelError;
use camel_bundles::BootHandle;
use camel_config::config::CamelConfig;
use camel_core::CamelContext;
use camel_core::RouteDefinition;

use crate::document::{RouteSource, ScenarioDocument};
use crate::env_layers::LayeredEnv;

/// A booted scenario: the started context the caller drives (route
/// stimulus, shutdown) and the teardown handle that owns pool
/// shutdown ordering.
pub struct ScenarioRun {
    /// The started, route-loaded context. The caller keeps ownership;
    /// wrap it in an `Arc<tokio::sync::Mutex<..>>` to share it with a
    /// [`DirectStimulus`](crate::adapters::DirectStimulus) adapter.
    pub ctx: CamelContext,
    /// The `camel_bundles` teardown sequencer; call
    /// `shutdown(&mut ctx)` after the verdict to drain lifecycles and
    /// pools.
    pub boot: BootHandle,
}

/// Boots the full composition root for one scenario document.
///
/// Sequence: load `<root>/Camel.toml` through the sealed loader with
/// the document's pinned profile (defaulting to `"default"`; ambient
/// `CAMEL_PROFILE` and allowlisted `CAMEL_*` overrides never apply),
/// prepare the context from that config, register the component
/// cascade through `camel_bundles::boot`, load the document's route
/// source, and start the context. Binding waits at `ctx.start()`
/// through the operator readiness signal.
///
/// `root` is the project root: the directory holding `Camel.toml`
/// and the base for route file resolution. Both route source file
/// forms (`routeFiles`, `routeFilesFromRoot`) resolve against it —
/// the v1 harness keeps the document in the project root.
pub async fn boot_scenario(
    doc: &ScenarioDocument,
    root: &Path,
    env: &LayeredEnv,
) -> Result<ScenarioRun, CamelError> {
    let config_path = root.join("Camel.toml");
    let config = CamelConfig::from_file_sealed(
        config_path.to_str().ok_or_else(|| {
            CamelError::Config(format!(
                "scenario config path is not valid utf-8: {}",
                config_path.display()
            ))
        })?,
        doc.profile.as_deref().unwrap_or("default"),
        &|name| env.lookup(name),
    )
    .map_err(|e| {
        CamelError::Config(format!(
            "failed to load scenario config {}: {e}",
            config_path.display()
        ))
    })?;

    let mut ctx = CamelConfig::configure_context_with_beans(&config, None).await?;
    let boot = camel_bundles::boot(&mut ctx, &config, root).await?;

    for def in load_route_definitions(doc, root, env)? {
        ctx.add_route_definition(def).await?;
    }
    ctx.start().await?;
    Ok(ScenarioRun { ctx, boot })
}

/// Loads the document's route source for the boot.
///
/// File forms read each file (capped like `camel run` route
/// discovery), resolve `${env:NAME}` placeholders through the layered
/// environment — never the process environment, the harness must not
/// read global state (ADR-0069 section 4) — and parse through
/// `camel_dsl::parse_yaml`, the same per-file YAML parser that sits
/// under `camel run` route discovery.
///
/// Inline routes cannot boot in v1: the document parser owns the
/// definitions, and this entry receives the document by reference, so
/// the definitions cannot move into the context. A FULL-tier
/// scenario that wants the embedded boot declares `routeFiles`.
fn load_route_definitions(
    doc: &ScenarioDocument,
    root: &Path,
    env: &LayeredEnv,
) -> Result<Vec<RouteDefinition>, CamelError> {
    match &doc.route_source {
        RouteSource::RouteFiles(files) | RouteSource::RouteFilesFromRoot(files) => {
            let mut defs = Vec::new();
            for file in files {
                let full = root.join(file);
                let metadata = std::fs::metadata(&full)
                    .map_err(|e| CamelError::Io(format!("{}: {e}", full.display())))?;
                if metadata.len() > camel_dsl::MAX_ROUTE_FILE_SIZE {
                    return Err(CamelError::RouteError(format!(
                        "{}: route file exceeds the {} byte cap",
                        full.display(),
                        camel_dsl::MAX_ROUTE_FILE_SIZE
                    )));
                }
                let content = std::fs::read_to_string(&full)
                    .map_err(|e| CamelError::Io(format!("{}: {e}", full.display())))?;
                let interpolated =
                    camel_dsl::env_interpolation::interpolate_env_with(&content, &|name| {
                        env.lookup(name)
                    })
                    .map_err(|name| {
                        CamelError::Config(format!(
                            "{}: unresolved ${{env:{name}}} placeholder \
                             (no layer of the scenario environment defines it)",
                            full.display()
                        ))
                    })?;
                defs.extend(
                    camel_dsl::parse_yaml(&interpolated)
                        .map_err(|e| CamelError::RouteError(format!("{}: {e}", full.display())))?,
                );
            }
            Ok(defs)
        }
        RouteSource::Inline(_) => Err(CamelError::Config(
            "inline route sources cannot boot in v1: declare routeFiles \
             (the document parser owns inline definitions; the boot \
             receives the document by reference)"
                .to_string(),
        )),
    }
}
