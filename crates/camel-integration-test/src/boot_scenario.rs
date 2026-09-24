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
//! Inbound provisioning (feature `http`, rc-5yon) runs inside the
//! boot: when the document declares `inbound:`, the listener binds
//! `127.0.0.1:0` and stages on the HTTP component's global registry
//! (ADR-0070 staged consumption) before the bundle cascade, and the
//! discovery environment gains the bound URL under the declared
//! bindVar (`LayeredEnv::with_harness_var`) so route-file consumer
//! templates interpolate the staged socket. The boot result carries
//! the bound address in `ScenarioRun::inbound_bound`.
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
    /// The bound address of the document's `inbound:` listener
    /// (rc-5yon, ADR-0070): the same address the discovery environment
    /// resolved under the declared bindVar. `None` when the document
    /// declares no `inbound:` section. Boot-owning library callers and
    /// tests carry it into `DocumentOutcome::inbound_bound` to target
    /// the ephemeral listener without re-deriving it.
    pub inbound_bound: Option<std::net::SocketAddr>,
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
/// `root` is the project root: the directory holding `Camel.toml`.
/// The sealed config load and `routeFilesFromRoot` resolution anchor
/// there; relative `routeFiles` stay anchored to the document's own
/// directory (`source_path`'s parent), so a nested document boots
/// from the nearest ancestor `Camel.toml` without relocating its
/// colocated route files (rc-jjzy5). For flat layouts the two
/// anchors coincide.
pub async fn boot_scenario(
    doc: &ScenarioDocument,
    root: &Path,
    env: &LayeredEnv,
) -> Result<ScenarioRun, CamelError> {
    // An empty root (a document named as a bare relative filename)
    // resolves its joins against the process CWD — fine for config load
    // and route files, fatal for the wasm base dir, whose empty
    // canonicalize() fails. Normalize to "." — the same empty-parent rule
    // `camel run` applies (try_canonical_project_root).
    let root: &Path = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    let doc_dir = doc
        .source_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
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

    // Boot-time lint (ungated): reject per-connection sqlite `:memory:`
    // datasource URLs before any context preparation — a config-shape
    // check, so it runs regardless of cargo features and before any
    // pool is created.
    crate::sql_action::ensure_sqlite_memory_shared(&config)?;

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

    // Inbound provisioning (feature `http`, rc-5yon): stage the
    // document's listener BEFORE any component bundle can spawn a
    // consumer, and extend the discovery environment with the bound
    // URL under the declared bindVar, so route-file consumer templates
    // (`http://${NAME}/...`) resolve to the staged socket (ADR-0070
    // staged consumption). The no-feature build rejects `inbound:` at
    // load and, defense-in-depth, in the `#[cfg(not(feature =
    // "http"))]` arm below, so a declaration reaching this arm implies
    // the feature.
    #[cfg(feature = "http")]
    let provisioned = match doc.inbound.as_ref() {
        Some(entry) => {
            let bound = crate::inbound::provision_inbound(entry).await?;
            Some((
                env.with_harness_var(&entry.bind_var, format!("http://{bound}")),
                bound,
            ))
        }
        None => None,
    };
    #[cfg(feature = "http")]
    let (discovery_env, inbound_bound) = match &provisioned {
        Some((extended, bound)) => (extended, Some(*bound)),
        None => (env, None),
    };
    #[cfg(not(feature = "http"))]
    let (discovery_env, inbound_bound) = if doc.inbound.is_some() {
        return Err(CamelError::Config(
            "inbound listeners need the `http` feature to boot: enable it to \
             provision the staged listener (the load-time doc gate now fires \
             first; this rejection is defense-in-depth for directly-constructed \
             documents)"
                .to_string(),
        ));
    } else {
        (env, None)
    };

    let boot = camel_bundles::boot(&mut ctx, &config, root).await?;

    let defs = camel_dsl::discover_routes_with_threshold_security_and_env(
        &route_patterns(doc, root, &doc_dir)?,
        config.stream_caching.threshold,
        security_ctx,
        &|name| discovery_env.lookup(name),
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
    Ok(ScenarioRun {
        ctx,
        boot,
        inbound_bound,
    })
}

/// Builds the route-discovery patterns for the document's route
/// source.
///
/// `routeFilesFromRoot` resolves against `root` (the nearest
/// ancestor `Camel.toml` directory); relative `routeFiles` resolve
/// against the document's own directory (`doc_dir`), so a nested
/// document keeps its colocated route files while booting from the
/// ancestor root (rc-jjzy5). Each declared file gets an existence
/// pre-check: a glob pattern that matches nothing is silent, and the
/// missing-file error must name the file the document declared. A
/// declared file with discovery's reserved `.test.yaml`/`.test.yml`
/// suffix is rejected here too: the document explicitly names the
/// file, so discovery's silent reserved-suffix skip would boot zero
/// routes — test documents belong to `camel test`, not scenario
/// routeFiles. Entries carrying glob metacharacters are rejected as
/// well (rc-rbde1): discovery glob-interprets its patterns, so a
/// literal metacharacter name would be silently reinterpreted; the
/// load-time doc gate rejects the class first, and this rejection is
/// the defense-in-depth twin.
///
/// Inline routes cannot boot in v1: the document parser owns the
/// definitions, and this entry receives the document by reference, so
/// the definitions cannot move into the context. A FULL-tier
/// scenario that wants the embedded boot declares `routeFiles`. The
/// document parser already rejects inline route sources at load
/// (rc-9dpx), before partners bind; this rejection stays only as
/// defense-in-depth for documents constructed directly, bypassing
/// `parse_scenario_document`.
fn route_patterns(
    doc: &ScenarioDocument,
    root: &Path,
    doc_dir: &Path,
) -> Result<Vec<String>, CamelError> {
    /// Resolves one declared file against its anchor directory and
    /// runs the shared pre-checks (glob metacharacters, existence,
    /// reserved test suffix).
    fn anchored(base: &Path, file: &Path) -> Result<String, CamelError> {
        // rc-rbde1 defense-in-depth: the load-time doc gate rejects
        // glob metacharacters inside `parse_scenario_document`; this
        // boot-level rejection covers documents constructed directly,
        // bypassing the parser. Without it, the existence pre-check
        // below passes for a literal metacharacter file while
        // discovery glob-interprets the entry — loading siblings the
        // document never declared, or nothing at all.
        if let Some(found) = crate::document::glob_metacharacters_in(&file.display().to_string()) {
            return Err(CamelError::Config(format!(
                "{}: scenario routeFiles are literal file paths, not glob patterns \
                 (glob metacharacters {found}; no escape syntax — the load-time doc \
                 gate rejects this class, so this document bypassed \
                 parse_scenario_document)",
                file.display()
            )));
        }
        let full = base.join(file);
        std::fs::metadata(&full).map_err(|e| CamelError::Io(format!("{}: {e}", full.display())))?;
        // Same predicate as discovery's reserved-document gate,
        // but fail loud: the route file was declared, not
        // glob-expanded, so a silent skip has no excuse.
        if camel_dsl::discovery::is_reserved_document(&full) {
            return Err(CamelError::Config(format!(
                "{}: reserved documents (*.test.yaml, *.test.yml belong to \
                 `camel test`; *.job.yaml, *.job.yml belong to `camel job`) \
                 are not scenario routeFiles",
                full.display()
            )));
        }
        Ok(full.display().to_string())
    }
    match &doc.route_source {
        RouteSource::RouteFiles(files) => {
            files.iter().map(|file| anchored(doc_dir, file)).collect()
        }
        RouteSource::RouteFilesFromRoot(files) => {
            files.iter().map(|file| anchored(root, file)).collect()
        }
        RouteSource::Inline(_) => Err(CamelError::Config(
            "inline route sources cannot boot in v1: declare routeFiles \
             (the load-time doc gate now fires first; this rejection is \
             defense-in-depth for directly-constructed documents)"
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
