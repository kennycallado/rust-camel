mod allocator_metrics;
pub mod commands;
mod security;
pub mod template;

use std::sync::Arc;

// ---------------------------------------------------------------------------
// Lint catalog registration — handle-free mirror of `commands::run`
// ---------------------------------------------------------------------------

/// Register a `ComponentBundle` from an empty TOML table, dropping any handle.
/// On empty-config build error, log at warn and skip — lint must degrade
/// gracefully (the skipped scheme surfaces as `unverified-scheme`), never fail
/// to construct its catalog.
macro_rules! register_bundle_empty {
    ($ctx:expr, $Bundle:ty) => {{
        let key = <$Bundle as camel_component_api::ComponentBundle>::config_key();
        match <$Bundle as camel_component_api::ComponentBundle>::from_toml(
            ::toml::Value::Table(::toml::map::Map::new()),
        ) {
            Ok(bundle) => {
                <$Bundle as camel_component_api::ComponentBundle>::register_all(
                    bundle,
                    &mut *$ctx,
                );
            }
            Err(err) => {
                tracing::warn!(
                    bundle = %key,
                    error = %err,
                    "lint catalog: empty-config build failed; skipping bundle (surfaces as unverified-scheme)"
                );
            }
        }
    }};
}

/// Like [`register_bundle_empty!`], but wires an empty datasource catalog
/// (sql/surrealdb bundles). An empty catalog is acceptable — metadata is
/// queryable regardless of configured datasources.
macro_rules! register_datasource_bundle_empty {
    ($ctx:expr, $Bundle:ty, $catalog:expr) => {{
        let key = <$Bundle as camel_component_api::ComponentBundle>::config_key();
        match <$Bundle as camel_component_api::ComponentBundle>::from_toml(
            ::toml::Value::Table(::toml::map::Map::new()),
        ) {
            Ok(bundle) => {
                let bundle = bundle.with_catalog(::std::sync::Arc::clone(&$catalog));
                <$Bundle as camel_component_api::ComponentBundle>::register_all(
                    bundle,
                    &mut *$ctx,
                );
            }
            Err(err) => {
                tracing::warn!(
                    bundle = %key,
                    error = %err,
                    "lint catalog: empty-config build failed; skipping datasource bundle (surfaces as unverified-scheme)"
                );
            }
        }
    }};
}

/// Register the built-in components into `ctx` for lint catalog population.
///
/// This mirrors `commands::run`'s registration list but is HANDLE-FREE: it
/// passes empty/default config to every bundle, registers bridge components
/// without their runtime handles (no `xsd_bridge_backend` / `bridge_runtime`,
/// no `BridgeCleanup`), and drops any pool returned by jms/cxf. It SKIPS the
/// path/route-coupled bundles: `wasm` (needs a config-relative `base_dir`) and
/// `exec` (route-conditional registration). Skipped schemes surface as
/// `unverified-scheme` notes in lint output, which is the accepted
/// graceful-degradation behaviour.
///
/// Because `Component::metadata()` has a trait default returning
/// `ComponentMetadata::minimal(scheme)` and `Registry::register()` harvests
/// it unconditionally, registering each builtin makes its scheme queryable by
/// the lint catalog (rich metadata where the component opts into it, a
/// minimal-but-present entry otherwise).
///
/// Drift tradeoff: this list and `run`'s list are kept separate on purpose —
/// `run`'s registration is lifecycle-entangled with bridge/pool/datasource/
/// path handles that lint has no use for. The drift is bounded and caught by
/// the corpus baseline (`tests/lint_corpus.rs`); unification is tracked by a
/// bd follow-up.
pub fn register_builtin_components_for_lint(ctx: &mut camel_core::CamelContext) {
    use camel_api::datasource::DatasourceCatalog;
    use camel_core::datasource::RuntimeDatasourceCatalog;

    // --- Config-independent components (handle-free) ---
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.register_component(camel_component_cron::CronComponent::new());
    ctx.register_component(camel_component_log::LogComponent::new());
    ctx.register_component(camel_component_direct::DirectComponent::new());
    ctx.register_component(camel_component_seda::SedaComponent::new());
    ctx.register_component(camel_component_mock::MockComponent::new());
    ctx.register_component(camel_component_controlbus::ControlBusComponent::new());

    // --- Bridge components WITHOUT their runtime handles ---
    // validator: xsd_bridge_backend() not captured, no backend stored.
    ctx.register_component(camel_component_validator::ValidatorComponent::new());
    // xslt / xj: bridge_runtime() not captured, no BridgeCleanup installed.
    // Lint is short-lived; no shutdown cleanup needed.
    ctx.register_component(camel_xslt::XsltComponent::default());
    ctx.register_component(camel_xj::XjComponent::default());

    // --- Empty datasource catalog for sql/surrealdb bundles ---
    let datasource_catalog: Arc<dyn DatasourceCatalog> = Arc::new(RuntimeDatasourceCatalog::new(
        std::collections::HashMap::new(),
    ));

    // --- Always-on bundles with empty config ---
    register_bundle_empty!(ctx, camel_component_http::HttpBundle);
    #[cfg(feature = "http-static")]
    register_bundle_empty!(ctx, camel_component_http::HttpStaticBundle);
    register_bundle_empty!(ctx, camel_component_ws::WsBundle);
    register_bundle_empty!(ctx, camel_component_file::FileBundle);
    register_bundle_empty!(ctx, camel_component_container::ContainerBundle);
    register_bundle_empty!(ctx, camel_template::TemplateBundle);
    register_bundle_empty!(ctx, camel_master::MasterBundle);
    register_bundle_empty!(ctx, camel_component_opensearch::OpenSearchBundle);
    register_bundle_empty!(ctx, camel_component_redis::RedisBundle);

    // jms / cxf: registered without capturing their pool handle (lint has no
    // runtime use for a bridge pool; metadata is harvested at register time).
    register_bundle_empty!(ctx, camel_component_jms::JmsBundle);
    register_bundle_empty!(ctx, camel_component_cxf::CxfBundle);

    // --- Datasource bundles (empty datasource catalog) ---
    register_datasource_bundle_empty!(ctx, camel_component_sql::SqlBundle, datasource_catalog);
    #[cfg(feature = "surrealdb")]
    register_datasource_bundle_empty!(
        ctx,
        camel_component_surrealdb::SurrealDbBundle,
        datasource_catalog
    );

    // --- Feature-gated bundles (empty config) ---
    #[cfg(feature = "kafka")]
    register_bundle_empty!(ctx, camel_component_kafka::KafkaBundle);
    #[cfg(feature = "mqtt")]
    register_bundle_empty!(ctx, camel_component_mqtt::MqttBundle);
    #[cfg(feature = "grpc")]
    register_bundle_empty!(ctx, camel_component_grpc::GrpcBundle);
    #[cfg(feature = "llm")]
    register_bundle_empty!(ctx, camel_component_llm::LlmBundle);

    // --- Skipped (path/route-coupled) ---
    // wasm: needs a config-relative `base_dir`; lint has no canonical config.
    // exec: route-conditional; lint has no discovered routes to gate on.
    // Both surface as `unverified-scheme` in lint output by design.
}
