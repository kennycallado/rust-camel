//! The embedded FULL-tier scenario boot (ADR-0069 sections 4, 5, 10).
//!
//! [`boot_scenario`] boots the same composition root `camel run`
//! boots, through the same seams and in the same order: the sealed
//! config load (pinned profile, no ambient `CAMEL_*` overrides,
//! `${env:}` through the layered environment), context preparation
//! through `camel_config`, the offline tier security gate (keycloak/
//! oidc and wasm policies/permissions fail closed — the tier has no
//! network and v1 supports the native provider only), the security
//! compile context through the shared `camel_bundles` builder, the
//! `[binds]` public-exposure acknowledgements (ADR-0061), the
//! component-bundle cascade through `camel_bundles::boot`, the
//! document's route source through `camel_dsl` route discovery
//! (two-pass template materialization included; every `${env:NAME}`
//! resolves through the layered environment, never the process
//! environment), the ADR-0033 fail-closed SQL startup checks from the
//! discovered definitions, and `ctx.start()`. Oversize route files now
//! surface as `CamelError::Io` (discovery's capped-read error),
//! matching `camel run`; the pre-delegation loader used `RouteError`
//! for this case.
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
/// Sequence (the `camel run` wiring order, ADR-0069 sections 4, 10):
/// load `<root>/Camel.toml` through the sealed loader with the
/// document's pinned profile (defaulting to `"default"`; ambient
/// `CAMEL_PROFILE` and allowlisted `CAMEL_*` overrides never apply),
/// prepare the context from that config, gate `[security.*]` for the
/// offline tier, build the security compile context through the
/// shared builder, install the `[binds]` exposure acknowledgements,
/// register the component cascade through `camel_bundles::boot`,
/// discover the document's route source with the config's
/// `stream_caching.threshold`, register the ADR-0033 SQL startup
/// checks from the discovered routes, and start the context. Binding
/// waits at `ctx.start()` through the operator readiness signal.
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

    // Tier security gate — a config-shape check, so it runs ungated
    // by cargo features and before any builder call: the scenario
    // tier has no network, so keycloak/oidc (network-prefetching auth
    // providers) and wasm policies/permissions (a later wave) fail
    // closed here, never at a fetch.
    if config.security.keycloak.is_some() || config.security.oidc.is_some() {
        return Err(CamelError::AuthProviderUnavailable(
            "scenario tier runs offline (no network): keycloak/oidc security requires \
             a network-prefetching auth provider; v1 supports the native provider only"
                .to_string(),
        ));
    }
    if config.security.policies.is_some() || config.security.permissions.is_some() {
        return Err(CamelError::Config(
            "wasm security policies/permissions are not supported in the scenario \
             tier in v1 (offline tier; later wave)"
                .to_string(),
        ));
    }

    // Security compile context through the shared builder (the `camel
    // run` seam): with the `security` feature the builder owns every
    // remaining `[security.*]` section (native); without it the
    // fail-closed guard rejects any configured section and the
    // default context compiles the routes.
    #[cfg(feature = "security")]
    let security_ctx = camel_bundles::security_boot::build_security_compile_context_from_config(
        &config,
        ctx.registry_arc(),
    )
    .await?;
    #[cfg(not(feature = "security"))]
    let security_ctx = {
        camel_bundles::security_boot::ensure_security_supported(&config)?;
        camel_dsl::SecurityCompileContext::default()
    };

    // ADR-0061: per-bind public-exposure acknowledgements from
    // `[binds]`, installed before any route starts staging — the
    // shared installer, same as `camel run`.
    camel_bundles::security_boot::install_bind_exposure_acks(&mut ctx, &config).await;

    let boot = camel_bundles::boot(&mut ctx, &config, root).await?;

    let defs = camel_dsl::discover_routes_with_threshold_security_and_env(
        &route_patterns(doc, root)?,
        config.stream_caching.threshold,
        security_ctx,
        &|name| env.lookup(name),
    )
    .map_err(map_discovery_error)?;

    // ADR-0033: fail-closed SQL startup checks from the discovered
    // routes; they run at the head of `ctx.start()`, before any route
    // consumer starts.
    camel_bundles::security_boot::install_sql_startup_checks(&mut ctx, &defs);

    for def in defs {
        ctx.add_route_definition(def).await?;
    }
    ctx.start().await?;
    Ok(ScenarioRun { ctx, boot })
}

/// Builds the route-discovery patterns for the document's route
/// source.
///
/// Both file forms resolve against `root`, as today. Each declared
/// file gets an existence pre-check: a glob pattern that matches
/// nothing is silent, and the missing-file error must name the file
/// the document declared. A declared file with discovery's reserved
/// `.test.yaml`/`.test.yml` suffix is rejected here too: the document
/// explicitly names the file, so discovery's silent reserved-suffix
/// skip would boot zero routes — test documents belong to `camel
/// test`, not scenario routeFiles.
///
/// Inline routes cannot boot in v1: the document parser owns the
/// definitions, and this entry receives the document by reference, so
/// the definitions cannot move into the context. A FULL-tier
/// scenario that wants the embedded boot declares `routeFiles`.
fn route_patterns(doc: &ScenarioDocument, root: &Path) -> Result<Vec<String>, CamelError> {
    match &doc.route_source {
        RouteSource::RouteFiles(files) | RouteSource::RouteFilesFromRoot(files) => files
            .iter()
            .map(|file| {
                let full = root.join(file);
                std::fs::metadata(&full)
                    .map_err(|e| CamelError::Io(format!("{}: {e}", full.display())))?;
                // Same predicate as discovery's reserved-suffix gate,
                // but fail loud: the route file was declared, not
                // glob-expanded, so a silent skip has no excuse.
                if camel_dsl::discovery::is_test_document(&full) {
                    return Err(CamelError::Config(format!(
                        "{}: test documents (*.test.yaml, *.test.yml) belong to \
                         `camel test`, not scenario routeFiles",
                        full.display()
                    )));
                }
                Ok(full.display().to_string())
            })
            .collect(),
        RouteSource::Inline(_) => Err(CamelError::Config(
            "inline route sources cannot boot in v1: declare routeFiles \
             (the document parser owns inline definitions; the boot \
             receives the document by reference)"
                .to_string(),
        )),
    }
}

/// Maps a discovery error into the `CamelError` shapes the scenario
/// boot reports. The env mapping keeps the message shape the per-file
/// loader produced (file path, variable name, and the no-layer
/// hermeticity note); the io and parse mappings keep the previous
/// per-file read/parse error classes.
fn map_discovery_error(err: camel_dsl::DiscoveryError) -> CamelError {
    match err {
        camel_dsl::DiscoveryError::Env { path, var_name } => CamelError::Config(format!(
            "{path}: unresolved ${{env:{var_name}}} placeholder \
             (no layer of the scenario environment defines it)"
        )),
        camel_dsl::DiscoveryError::Io { path, source } => {
            CamelError::Io(format!("{path}: {source}"))
        }
        camel_dsl::DiscoveryError::Yaml { path, error } => {
            CamelError::RouteError(format!("{path}: {error}"))
        }
        camel_dsl::DiscoveryError::Json { path, error } => {
            CamelError::RouteError(format!("{path}: {error}"))
        }
        other => CamelError::RouteError(other.to_string()),
    }
}
