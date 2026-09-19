//! Component-bundle registration cascade for rust-camel (ADR-0069 section 10).
//!
//! This crate owns the `ComponentBundle::register_all` cascade extracted
//! verbatim from `camel run`, plus the [`BootHandle`] teardown sequencer for
//! the bridge cleanup and the JMS/CXF pools. `camel run` and the integration
//! harness both register bundles through this one cascade; feature gates
//! forward from each consumer into this crate.
//!
//! Ownership boundary: the caller creates and pre-configures the
//! [`CamelContext`], passes it to [`boot`] by `&mut`, and keeps it. [`boot`]
//! configures and registers only: route loading, discovery, startup checks,
//! and `ctx.start()` stay with the caller. This crate never terminates the
//! process; every failure returns a [`CamelError`].
//!
//! Conditional registration sites that need one bundle outside the
//! always-on set (the CLI exec gate, the integration harness) use
//! [`register_bundle`] instead of [`boot`].

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use camel_api::CamelError;
use camel_api::datasource::DatasourceCatalog;
use camel_config::config::CamelConfig;
use camel_core::CamelContext;
use camel_core::datasource::RuntimeDatasourceCatalog;

pub mod security_boot;
pub mod stream_collision;

// Shared security compile-context builder (task 2.2), doc-visible at the
// crate root for boot consumers.
#[cfg(feature = "security")]
pub use security_boot::build_security_compile_context_from_config;

// stream:out / tracer-stdout collision warning (stream-component task 1.4),
// doc-visible at the crate root for the CLI boot paths.
pub use stream_collision::warn_stream_stdout_collision;

struct BridgeCleanup {
    #[cfg(feature = "xslt")]
    xslt: Arc<camel_xslt::XsltBridgeRuntime>,
    #[cfg(feature = "xj")]
    xj: Arc<camel_xj::XjBridgeRuntime>,
    validator: Option<Arc<camel_component_validator::xsd_bridge::XsdBridgeBackend>>,
}

#[async_trait::async_trait]
impl camel_api::lifecycle::Lifecycle for BridgeCleanup {
    fn name(&self) -> &str {
        "bridge-cleanup"
    }

    async fn start(&mut self) -> Result<(), camel_api::CamelError> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), camel_api::CamelError> {
        #[cfg(feature = "xslt")]
        self.xslt.shutdown().await;
        #[cfg(feature = "xj")]
        self.xj.shutdown().await;
        if let Some(validator) = &self.validator {
            validator.shutdown().await;
        }
        Ok(())
    }
}

/// Default teardown budget for pool shutdown, preserved from `camel run`.
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Teardown sequencer returned by [`boot`].
///
/// Owns the shutdown ordering for the JMS/CXF bridge pools (present under
/// the per-bridge `jms`/`cxf` features) and drives the
/// context stop that drains the context-registered lifecycles
/// (`BridgeCleanup` included). The context itself stays owned by the caller;
/// the handle borrows nothing from it.
pub struct BootHandle {
    #[cfg(feature = "jms")]
    jms_pool: Arc<camel_component_jms::JmsBridgePool>,
    #[cfg(feature = "cxf")]
    cxf_pool: Arc<camel_component_cxf::CxfBridgePool>,
    datasource_catalog: Arc<dyn DatasourceCatalog>,
}

impl BootHandle {
    /// The single datasource catalog of the booted cascade.
    ///
    /// Single-catalog invariant: callers resolve pools through the same
    /// catalog the component cascade registered factories on; one datasource
    /// name, one pool.
    pub fn datasource_catalog(&self) -> Arc<dyn DatasourceCatalog> {
        Arc::clone(&self.datasource_catalog)
    }

    /// Graceful teardown with the default 30-second pool deadline.
    pub async fn shutdown(&self, ctx: &mut CamelContext) -> Result<(), CamelError> {
        self.shutdown_with_deadline(ctx, DEFAULT_SHUTDOWN_TIMEOUT)
            .await
    }

    /// Graceful teardown with an explicit pool deadline. Ordering is the
    /// `camel run` teardown, preserved exactly:
    ///
    /// 1. `begin_shutdown()` on both pools (stop restarting before context
    ///    shutdown),
    /// 2. `ctx.stop()` (routes plus context-registered lifecycles;
    ///    `BridgeCleanup` drains here),
    /// 3. deadline-wrapped `pool.shutdown()` for JMS, then CXF,
    /// 4. deadline-wrapped `datasource_catalog.close_all()` — the boot's
    ///    datasource pools drain here, so a shared-cache sqlite in-memory
    ///    database dies with its boot (bd rc-25lup.4).
    ///
    /// Pool steps 1 and 3 are cfg-conditional on the `jms`/`cxf` features,
    /// one block per pool (rc-9720m); steps 2 and 4 run in every feature
    /// combination.
    ///
    /// Every step runs even when an earlier one fails; the first failure is
    /// returned after the sequence completes. Failures are logged by the
    /// handle (`system-broken` per ADR-0012) with the message `camel run`
    /// produced; callers that need the value may inspect it. Pool timeouts
    /// warn and do not fail the shutdown, matching `camel run`.
    pub async fn shutdown_with_deadline(
        &self,
        ctx: &mut CamelContext,
        deadline: Duration,
    ) -> Result<(), CamelError> {
        // Signal pools to stop restarting BEFORE context shutdown
        #[cfg(feature = "jms")]
        self.jms_pool.begin_shutdown();
        #[cfg(feature = "cxf")]
        self.cxf_pool.begin_shutdown();

        // Stop context (routes + lifecycle services)
        let mut failure = ctx.stop().await.err();
        if let Some(e) = &failure {
            // log-policy: system-broken
            tracing::error!("Error during shutdown: {}", e);
        }

        // Tear down bridge pools with timeouts
        #[cfg(feature = "jms")]
        match tokio::time::timeout(deadline, self.jms_pool.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                // log-policy: system-broken
                tracing::error!("JMS pool shutdown failed: {}", e);
                if failure.is_none() {
                    failure = Some(e);
                }
            }
            Err(_) => {
                tracing::warn!("JMS pool shutdown timed out after {}s", deadline.as_secs())
            }
        }

        #[cfg(feature = "cxf")]
        match tokio::time::timeout(deadline, self.cxf_pool.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                // log-policy: system-broken
                tracing::error!("CXF pool shutdown failed: {}", e);
                if failure.is_none() {
                    failure = Some(e);
                }
            }
            Err(_) => {
                tracing::warn!("CXF pool shutdown timed out after {}s", deadline.as_secs())
            }
        }

        // Drain the boot's datasource pools: without this step a sqlx pool
        // with `min_connections = 1` outlives the boot's scope, keeping a
        // shared-cache sqlite in-memory database alive into the next
        // document's boot in the same process (bd rc-25lup.4).
        match tokio::time::timeout(deadline, self.datasource_catalog.close_all()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                // log-policy: system-broken
                tracing::error!("datasource pool close failed: {}", e);
                if failure.is_none() {
                    failure = Some(e);
                }
            }
            Err(_) => tracing::warn!(
                "datasource pool close timed out after {}s",
                deadline.as_secs()
            ),
        }

        match failure {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Parse one bundle from its `[components.<config_key()>]` table in
/// `config`, falling back to an empty table so the bundle always parses
/// with its serde defaults. Returns the bundle's own `from_toml` error
/// verbatim.
///
/// The parse half of the [`register_bundle`] seam: conditional registration
/// sites (the CLI exec gate, the integration harness) and the
/// catalog-threaded Sql/SurrealDb blocks need the parsed bundle before
/// registration.
pub fn bundle_from_config<B: camel_component_api::ComponentBundle>(
    config: &CamelConfig,
) -> Result<B, CamelError> {
    let raw = config
        .components
        .raw
        .get(<B as camel_component_api::ComponentBundle>::config_key())
        .cloned()
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    <B as camel_component_api::ComponentBundle>::from_toml(raw)
}

/// Register one bundle on `ctx` from its `[components.<config_key()>]`
/// table in `config`, falling back to an empty table so the bundle always
/// registers with its serde defaults. A parse failure returns
/// `CamelError::Config` with the cascade's message
/// (`Failed to load <key> config: <e>`).
///
/// This is the single-bundle seam behind the [`boot`] cascade. Conditional
/// gates use it to register one bundle outside the always-on set.
pub fn register_bundle<B: camel_component_api::ComponentBundle>(
    ctx: &mut CamelContext,
    config: &CamelConfig,
) -> Result<(), CamelError> {
    register_bundle_with::<B, ()>(ctx, config, |_: &B| ())
}

/// Register one bundle and return a value extracted from it before
/// registration (the pool handles of the jms and cxf bundles). Same parse,
/// fallback, and error text as [`register_bundle`].
pub fn register_bundle_with<B, T>(
    ctx: &mut CamelContext,
    config: &CamelConfig,
    extract: impl FnOnce(&B) -> T,
) -> Result<T, CamelError>
where
    B: camel_component_api::ComponentBundle,
{
    match bundle_from_config::<B>(config) {
        Ok(bundle) => {
            let extracted = extract(&bundle);
            <B as camel_component_api::ComponentBundle>::register_all(bundle, ctx);
            Ok(extracted)
        }
        Err(e) => Err(camel_api::CamelError::Config(format!(
            "Failed to load {} config: {}",
            <B as camel_component_api::ComponentBundle>::config_key(),
            e
        ))),
    }
}

/// Register every component of the `camel run` cascade on a caller-owned
/// context (ADR-0069 section 10).
///
/// The caller creates and pre-configures `ctx` (function lifecycle, security
/// context, bind acknowledgements, WASM beans; `camel_config`'s
/// `configure_context_with_beans` remains the caller's entry) and passes it
/// in. `boot` then:
///
/// - builds the datasource catalog from `config.datasources` against the
///   prepared context's health registry and threads it into the Sql and
///   SurrealDb bundles,
/// - constructs the `WasmBundle` from `ctx.registry_arc()` and
///   `project_root` when the `wasm` feature is on,
/// - registers the built-in components and every bundle of the cascade,
///   reading each `[components.<key>]` table from `config` with an
///   empty-table fallback (serde defaults),
/// - registers `BridgeCleanup` as a context lifecycle, drained by
///   [`BootHandle::shutdown`] ordering.
///
/// Route loading, discovery, startup checks, and `ctx.start()` stay with the
/// caller. Returns only the [`BootHandle`]; the context stays owned by the
/// caller.
pub async fn boot(
    ctx: &mut CamelContext,
    config: &CamelConfig,
    project_root: &Path,
) -> Result<BootHandle, CamelError> {
    // `project_root` feeds only the wasm bundle base dir today; keep the
    // parameter live under every feature combination.
    let _ = project_root;

    // Datasource catalog from configured datasources, wiring the health
    // registry of the prepared context (moved here from `camel run`).
    let datasource_catalog: Arc<dyn DatasourceCatalog> = {
        let catalog = RuntimeDatasourceCatalog::new(config.datasources.clone())
            .with_health_registry(ctx.health_registry());
        Arc::new(catalog)
    };

    // The cascade registers each bundle through the `register_bundle` seam
    // (config-key lookup with empty-table fallback, serde defaults).

    // Register built-in components (no config needed)
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.register_component(camel_component_cron::CronComponent::new());
    ctx.register_component(camel_component_log::LogComponent::new());
    ctx.register_component(camel_component_stream::StreamComponent::new());
    ctx.register_component(camel_component_direct::DirectComponent::new());
    ctx.register_component(camel_component_seda::SedaComponent::new());
    ctx.register_component(camel_component_mock::MockComponent::new());
    ctx.register_component(camel_component_controlbus::ControlBusComponent::new());
    let validator_component = camel_component_validator::ValidatorComponent::new();
    let validator_backend = validator_component.xsd_bridge_backend();
    ctx.register_component(validator_component);

    #[cfg(feature = "xslt")]
    let xslt_runtime = {
        let xslt_component = camel_xslt::XsltComponent::default();
        let runtime = xslt_component.bridge_runtime();
        ctx.register_component(xslt_component);
        runtime
    };

    #[cfg(feature = "xj")]
    let xj_runtime = {
        let xj_component = camel_xj::XjComponent::default();
        let runtime = xj_component.bridge_runtime();
        ctx.register_component(xj_component);
        runtime
    };

    ctx.add_lifecycle(BridgeCleanup {
        #[cfg(feature = "xslt")]
        xslt: xslt_runtime,
        #[cfg(feature = "xj")]
        xj: xj_runtime,
        validator: validator_backend,
    });

    // Register HTTP, File, Template (always-on in the cascade). Container
    // registers under the `containers` gate (infrastructure-daemon client;
    // one feature with camel-function, function⇒container per ADR-0005).
    // WS registers under its own per-bridge `ws` gate (rc-9720m): the gate
    // below is the ws gate, no longer a carrier rider.
    register_bundle::<camel_component_http::HttpBundle>(ctx, config)?;
    #[cfg(feature = "http-static")]
    register_bundle::<camel_component_http::HttpStaticBundle>(ctx, config)?;
    #[cfg(feature = "ws")]
    register_bundle::<camel_component_ws::WsBundle>(ctx, config)?;
    register_bundle::<camel_component_file::FileBundle>(ctx, config)?;
    #[cfg(feature = "containers")]
    register_bundle::<camel_component_container::ContainerBundle>(ctx, config)?;
    // External template renderer (ADR-0047 Stage 2): always-on built-in.
    register_bundle::<camel_template::TemplateBundle>(ctx, config)?;

    // Register optional/feature-gated bundles
    #[cfg(feature = "jms")]
    let jms_pool =
        register_bundle_with(ctx, config, |b: &camel_component_jms::JmsBundle| b.pool())?;

    #[cfg(feature = "cxf")]
    let cxf_pool =
        register_bundle_with(ctx, config, |b: &camel_component_cxf::CxfBundle| b.pool())?;

    #[cfg(feature = "kafka")]
    register_bundle::<camel_component_kafka::KafkaBundle>(ctx, config)?;
    #[cfg(feature = "mqtt")]
    register_bundle::<camel_component_mqtt::MqttBundle>(ctx, config)?;
    register_bundle::<camel_master::MasterBundle>(ctx, config)?;
    #[cfg(feature = "opensearch")]
    register_bundle::<camel_component_opensearch::OpenSearchBundle>(ctx, config)?;
    #[cfg(feature = "redis")]
    register_bundle::<camel_component_redis::RedisBundle>(ctx, config)?;
    #[cfg(feature = "sql")]
    {
        match bundle_from_config::<camel_component_sql::SqlBundle>(config) {
            Ok(bundle) => {
                let bundle = bundle.with_catalog(Arc::clone(&datasource_catalog));
                <camel_component_sql::SqlBundle as camel_component_api::ComponentBundle>::register_all(bundle, ctx);
            }
            Err(e) => {
                // log-policy: system-broken
                tracing::error!("failed to initialize SQL bundle: {}", e);
            }
        }
    }
    #[cfg(feature = "surrealdb")]
    {
        match bundle_from_config::<camel_component_surrealdb::SurrealDbBundle>(config) {
            Ok(bundle) => {
                let bundle = bundle.with_catalog(Arc::clone(&datasource_catalog));
                <camel_component_surrealdb::SurrealDbBundle as camel_component_api::ComponentBundle>::register_all(
                    bundle, ctx,
                );
            }
            Err(e) => {
                // log-policy: system-broken
                tracing::error!("failed to initialize SurrealDB bundle: {}", e);
            }
        }
    }
    #[cfg(feature = "grpc")]
    register_bundle::<camel_component_grpc::GrpcBundle>(ctx, config)?;

    #[cfg(feature = "llm")]
    register_bundle::<camel_component_llm::LlmBundle>(ctx, config)?;

    #[cfg(feature = "mcp")]
    register_bundle::<camel_component_mcp::McpBundle>(ctx, config)?;

    #[cfg(feature = "wasm")]
    {
        let wasm_bundle = camel_component_wasm::WasmBundle::new(
            Arc::new(camel_core::RegistryComponentContext::new(
                ctx.registry_arc(),
                Some(ctx.metrics()),
                camel_component_api::ComponentContext::component_metrics_enabled(&*ctx),
            )),
            project_root.to_path_buf(),
        );
        <camel_component_wasm::WasmBundle as camel_component_api::ComponentBundle>::register_all(
            wasm_bundle,
            ctx,
        );
    }

    // Languages are registered in `configure_context_with_beans` (camel-config)
    // via `camel_core::languages_from_config(&config.languages)`, which applies
    // Camel.toml [languages.*.limits] and registers js/javascript/rhai/
    // jsonpath/xpath under feature gates. A direct `CamelContext::builder().build()`
    // caller gets `LanguagesConfig::default()` (rust-camel runtime defaults).

    Ok(BootHandle {
        #[cfg(feature = "jms")]
        jms_pool,
        #[cfg(feature = "cxf")]
        cxf_pool,
        datasource_catalog: Arc::clone(&datasource_catalog),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the not-kafka gating probe builds a route; its cfg-gated
    // kafka twin asserts registry presence alone.
    #[cfg(not(feature = "kafka"))]
    use camel_core::{BuilderStep, RouteDefinition};

    fn fixture(rel: &str) -> String {
        format!("{}/tests/fixtures/{rel}", env!("CARGO_MANIFEST_DIR"))
    }

    async fn booted_context(fixture_rel: &str) -> (CamelContext, BootHandle, CamelConfig) {
        let path = fixture(fixture_rel);
        let config =
            CamelConfig::from_file(&path).unwrap_or_else(|e| panic!("fixture {path}: {e}"));
        let mut ctx = CamelConfig::configure_context_with_beans(&config, None)
            .await
            .expect("configure_context_with_beans must succeed");
        let handle = boot(&mut ctx, &config, Path::new(env!("CARGO_MANIFEST_DIR")))
            .await
            .expect("boot must register the cascade");
        (ctx, handle, config)
    }

    /// All eight bridges register on the bundles-present fixture — the
    /// runtime regression probe for every boot-registration gate on this
    /// page (one scheme per gated bundle site).
    #[cfg(all(
        feature = "jms",
        feature = "sql",
        feature = "redis",
        feature = "opensearch",
        feature = "ws",
        feature = "cxf",
        feature = "xj",
        feature = "xslt"
    ))]
    #[tokio::test]
    async fn boot_registers_all_bundles_from_fixture_config() {
        let (ctx, _handle, _config) = booted_context("bundles-present/Camel.toml").await;

        for scheme in [
            "http",
            "https",
            "ws",
            "file",
            "container",
            "template",
            "jms",
            "xslt",
            "xj",
            "cxf",
            "sql",
            "redis",
            "opensearch",
        ] {
            assert!(
                ctx.registry().get(scheme).is_some(),
                "scheme '{scheme}' must resolve after boot"
            );
        }
    }

    /// Slim polarity of the boot cascade: with the per-bridge features off
    /// (bridges off), the eight bridges are not compiled in, so boot
    /// registers the core components only, no bridge scheme resolves, and
    /// shutdown drains without the gated pools. Companion to the gated
    /// bundles-present test above.
    #[cfg(not(feature = "jms"))]
    #[tokio::test]
    async fn boot_slim_registers_core_without_bridges() {
        let (mut ctx, handle, _config) = booted_context("no-http/Camel.toml").await;

        for scheme in [
            "timer",
            "log",
            "stream",
            "direct",
            "seda",
            "mock",
            "controlbus",
        ] {
            assert!(
                ctx.registry().get(scheme).is_some(),
                "core scheme '{scheme}' must resolve after slim boot"
            );
        }
        assert!(
            ctx.registry().get("jms").is_none(),
            "bridge scheme 'jms' must be absent without the jms feature"
        );
        handle
            .shutdown(&mut ctx)
            .await
            .expect("slim shutdown must drain without the gated pools");
    }

    /// `http-static` reverts to its pre-115 meaning: the HttpStaticBundle
    /// (`http-static:` scheme, ADR-0009) registers by itself and no bridge
    /// dependency rides the gate.
    #[cfg(all(
        feature = "http-static",
        not(feature = "jms"),
        not(feature = "sql"),
        not(feature = "redis"),
        not(feature = "opensearch"),
        not(feature = "ws"),
        not(feature = "cxf"),
        not(feature = "xj"),
        not(feature = "xslt")
    ))]
    #[tokio::test]
    async fn http_static_registers_without_bridges() {
        let (ctx, _handle, _config) = booted_context("no-http/Camel.toml").await;

        assert!(
            ctx.registry().get("http-static").is_some(),
            "scheme 'http-static' must resolve under the http-static feature"
        );
        assert!(
            ctx.registry().get("jms").is_none(),
            "bridge scheme 'jms' must be absent without the jms feature"
        );
    }

    /// Per-bridge subset: with only `sql` and `jms` selected, boot registers
    /// exactly those two bridges, the six unselected ones stay absent, and
    /// shutdown drains the jms pool without a cxf pool compiled in.
    #[cfg(all(
        feature = "sql",
        feature = "jms",
        not(feature = "cxf"),
        not(feature = "opensearch"),
        not(feature = "redis"),
        not(feature = "ws"),
        not(feature = "xj"),
        not(feature = "xslt")
    ))]
    #[tokio::test]
    async fn boot_subset_sql_jms_registers_only_selected() {
        let (mut ctx, handle, _config) = booted_context("bundles-present/Camel.toml").await;

        assert!(
            ctx.registry().get("sql").is_some(),
            "scheme 'sql' must resolve with the sql feature selected"
        );
        assert!(
            ctx.registry().get("jms").is_some(),
            "scheme 'jms' must resolve with the jms feature selected"
        );
        for scheme in ["cxf", "opensearch", "redis", "ws", "xj", "xslt"] {
            assert!(
                ctx.registry().get(scheme).is_none(),
                "unselected bridge scheme '{scheme}' must stay absent"
            );
        }
        handle
            .shutdown(&mut ctx)
            .await
            .expect("subset shutdown must drain the jms pool without a cxf pool");
    }

    /// xj-alone polarity: the xj gate does not imply the xslt registration
    /// gate. camel-xj links camel-xslt transitively (library-level XSLT
    /// client), but the `xslt:` scheme registers only under the xslt
    /// feature — pin that xj-without-xslt is a coherent build.
    #[cfg(all(
        feature = "xj",
        not(feature = "xslt"),
        not(feature = "jms"),
        not(feature = "sql"),
        not(feature = "redis"),
        not(feature = "opensearch"),
        not(feature = "ws"),
        not(feature = "cxf")
    ))]
    #[tokio::test]
    async fn boot_xj_alone_registers_xj_not_xslt() {
        let (ctx, _handle, _config) = booted_context("bundles-present/Camel.toml").await;
        assert!(
            ctx.registry().get("xj").is_some(),
            "xj must resolve under the xj feature alone"
        );
        assert!(
            ctx.registry().get("xslt").is_none(),
            "xslt scheme must stay unregistered without the xslt feature"
        );
    }

    /// Disabled-feature half of the gating probe: without the `kafka` cargo
    /// feature the registry has no kafka component, and resolving a kafka:
    /// send step fails with a component-not-found error naming kafka.
    /// The cfg-gated companion below asserts the enabled half.
    #[cfg(not(feature = "kafka"))]
    #[tokio::test]
    async fn boot_feature_gating_matches_flags() {
        let (ctx, _handle, _config) = booted_context("bundles-present/Camel.toml").await;

        assert!(ctx.registry().get("kafka").is_none());

        let def = RouteDefinition::new(
            "direct:kafka-gate-probe",
            vec![BuilderStep::To("kafka:orders".to_string())],
        )
        .with_route_id("kafka-gate-probe");
        let err = ctx
            .add_route_definition(def)
            .await
            .expect_err("kafka send step must not resolve without the kafka feature");
        match err {
            // Registration wraps compile failures in RouteError; both shapes
            // must name kafka as the missing component.
            CamelError::ComponentNotFound(name) => assert_eq!(name, "kafka"),
            CamelError::RouteError(msg) => assert!(
                msg.contains("Component not found: kafka"),
                "error must name the kafka component: {msg}"
            ),
            other => panic!("expected ComponentNotFound, got {other:?}"),
        }
    }

    /// cfg-gated re-run of the gating probe: with the `kafka` feature the
    /// same boot registers the kafka bundle. Executed by
    /// `cargo test -p camel-bundles --features kafka --lib
    /// boot_feature_gating_matches_flags`.
    #[cfg(feature = "kafka")]
    #[tokio::test]
    async fn boot_feature_gating_matches_flags_kafka_enabled() {
        let (ctx, _handle, _config) = booted_context("bundles-present/Camel.toml").await;

        assert!(
            ctx.registry().get("kafka").is_some(),
            "kafka must resolve with the kafka feature enabled"
        );
    }

    /// The booted handle exposes the datasource catalog the cascade threaded
    /// into the Sql/SurrealDb bundles, so scenario runners can resolve pools
    /// through the same single catalog (one datasource name, one pool).
    #[tokio::test]
    async fn boot_handle_exposes_datasource_catalog() {
        use camel_api::datasource::DatasourceConfig;
        use std::collections::HashMap;

        let mut datasources = HashMap::new();
        datasources.insert(
            "appdb".to_string(),
            DatasourceConfig {
                // Named shared-memory URI: no automatic datasource
                // factory prefix matches `sqlite:file:`, so the sqlx
                // provider is pinned explicitly.
                db_url: "sqlite:file:memdb_bundles_catalog_probe?mode=memory&cache=shared"
                    .to_string(),
                provider: Some("sqlx".to_string()),
                max_connections: None,
                min_connections: None,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                ssl_mode: None,
                ssl_root_cert: None,
                ssl_cert: None,
                ssl_key: None,
                extra: HashMap::new(),
            },
        );
        let config = CamelConfig {
            datasources,
            ..CamelConfig::default()
        };
        let mut ctx = CamelConfig::configure_context_with_beans(&config, None)
            .await
            .expect("configure_context_with_beans must succeed");
        let handle = boot(&mut ctx, &config, Path::new(env!("CARGO_MANIFEST_DIR")))
            .await
            .expect("boot must register the cascade");

        let appdb = handle
            .datasource_catalog()
            .get_config("appdb")
            .expect("booted handle must expose the appdb datasource");
        assert_eq!(
            appdb.db_url,
            "sqlite:file:memdb_bundles_catalog_probe?mode=memory&cache=shared"
        );
    }

    #[tokio::test]
    async fn boot_missing_config_key_falls_back_to_bundle_defaults() {
        let (ctx, _handle, config) = booted_context("no-http/Camel.toml").await;

        assert!(
            !config.components.raw.contains_key("http"),
            "fixture must not carry an [components.http] table"
        );
        // Empty-table fallback path: the bundle registers with serde defaults.
        assert!(
            ctx.registry().get("http").is_some(),
            "http must resolve without an [components.http] table"
        );
        assert!(ctx.registry().get("https").is_some());
    }

    /// The single-bundle seam: `register_bundle` registers exactly one
    /// bundle on a prepared context, without the rest of the cascade. This
    /// is the shape conditional gates (CLI exec, harness) call.
    #[tokio::test]
    async fn register_bundle_registers_one_bundle_without_boot() {
        let path = fixture("no-http/Camel.toml");
        let config =
            CamelConfig::from_file(&path).unwrap_or_else(|e| panic!("fixture {path}: {e}"));
        let mut ctx = CamelConfig::configure_context_with_beans(&config, None)
            .await
            .expect("configure_context_with_beans must succeed");

        register_bundle::<camel_component_http::HttpBundle>(&mut ctx, &config)
            .expect("single-bundle registration must succeed");

        assert!(ctx.registry().get("http").is_some());
        // Only the requested bundle: cascade built-ins stay unregistered.
        assert!(ctx.registry().get("timer").is_none());
    }
}
