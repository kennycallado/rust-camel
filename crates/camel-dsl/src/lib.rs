//! YAML/JSON DSL for defining Camel routes — declarative route parsing and compilation to canonical specs.
//!
//! Main types: `DeclarativeRoute`, `DeclarativeStep`, `RouteDslRoute`, `RouteDslStep`.
//! Main modules: `canonical`, `compile`, `contract`, `discovery`, `json`, `model`, `yaml`, `route_ast`.

pub mod canonical;
pub mod compile;
pub mod config_semantics;
pub mod contract;
pub mod discovery;
pub mod embedded_store;
mod env_int_probe;
pub mod env_interpolation;
mod input_format;
pub mod json;
pub mod mcp;
pub mod media;
pub mod model;
pub mod openapi;
#[cfg(test)]
mod parity_tests;
pub mod rest;
pub mod route_ast;
pub mod script_watch;
pub mod template;
pub mod transacted_validation;
mod util;
pub(crate) mod virtual_config;
pub mod yaml;

pub use canonical::{parse_canonical_json, parse_canonical_route};
pub use compile::{
    compile_declarative_route, compile_declarative_route_to_canonical,
    compile_declarative_route_with, compile_declarative_route_with_stream_cache_threshold,
    compile_declarative_step,
};
pub use contract::{DeclarativeStepKind, is_rust_only_kind, mandatory_declarative_step_kinds};
pub use discovery::{
    DiscoveryError, EmbeddedDocumentKind, MaterializationFailure, VirtualStoreDiscovery,
    discover_embedded_text, discover_routes, discover_routes_with_threshold,
    discover_routes_with_threshold_and_security, discover_routes_with_threshold_security_and_env,
    discover_virtual_store,
};
pub use embedded_store::{
    STORE_SCHEMA, SourcePlan, StoreAsset, StoreDocument, StoreEntry, StoreEntryKind, StoreError,
    StoreIndex, SubstitutionContext, SubstitutionEntry, SubstitutionSpan, VirtualDocumentStore,
};
pub use env_interpolation::{interpolate_with_args, interpolate_yaml_source};
pub use json::{
    parse_json, parse_json_routes_with_env, parse_json_to_canonical, parse_json_to_declarative,
    parse_json_with_threshold, parse_json_with_threshold_and_security,
};
pub use model::{
    AggregateStepDef, AggregateStrategyDef, BodyTypeDef, ChoiceStepDef, DeclarativeCircuitBreaker,
    DeclarativeConcurrency, DeclarativeErrorHandler, DeclarativeRedeliveryPolicy, DeclarativeRoute,
    DeclarativeSecurityPolicy, DeclarativeStep, EnrichStepDef, FilterStepDef,
    LanguageExpressionDef, LogLevelDef, LogStepDef, MulticastAggregationDef, MulticastStepDef,
    ScriptStepDef, SecurityCompileContext, SetBodyStepDef, SetHeaderStepDef, SetPropertyStepDef,
    SplitAggregationDef, SplitExpressionDef, SplitStepDef, StreamCacheStepDef, ThrowStepDef,
    ToStepDef, TransactedStepDef, ValueSourceDef, WhenStepDef, WireTapStepDef,
};
pub use openapi::generate_openapi;
pub use script_watch::ScriptWatchRegistry;
pub use transacted_validation::{SqlStepFacts, sql_step_facts, validate_transacted_blocks};
pub use util::MAX_ROUTE_FILE_SIZE;
pub use yaml::{
    RouteDslRoute, RouteDslRoutes, RouteDslStep, RoutesEnvError,
    extract_rest_blocks_from_file_with_env, load_from_file, load_from_file_with_env,
    parse_routes_with_env, parse_yaml, parse_yaml_to_canonical, parse_yaml_to_declarative,
    parse_yaml_with_threshold, parse_yaml_with_threshold_and_security,
};

pub use template::json::{parse_json_templated_routes, parse_json_templates};
pub use template::materializer::{
    CompiledMaterializationResult, materialize_and_compile, materialize_template, resolve_params,
    substitute_strings_in_json,
};
pub use template::yaml::{parse_yaml_templated_routes, parse_yaml_templates};

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;

    /// Install a bare registry as the process-global tracing default, once
    /// per test binary. Guards capture tests against callsite-interest
    /// poisoning: `tracing` caches each callsite's `Interest` process-wide
    /// from its FIRST macro execution, evaluated against the executing
    /// thread's dispatcher. Subscriber-less sibling tests in this binary
    /// hit the shared warn/error callsites first (e.g. the discovery
    /// "recursive includes" warn, the yaml parse-failed `error!` replay at
    /// `yaml.rs:178/196`) and cache `Interest::never`, so a later
    /// thread-local `set_default` capture silently drops events. The
    /// global registry heals prior poison and floors future rebuilds at
    /// `sometimes` (fix pattern: c3853198; bd rc-img5).
    pub(crate) fn ensure_global_tracing_default() {
        static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if INIT.set(()).is_ok() {
            let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
        }
    }

    /// WARN/ERROR counters for one capture scope. `warn()` and `error()`
    /// stay valid after the [`tracing::subscriber::DefaultGuard`] that
    /// produced them drops (the counters are shared via `Arc`).
    pub(crate) struct EventCounts {
        warn: Arc<std::sync::atomic::AtomicUsize>,
        error: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl EventCounts {
        /// Events at WARN level observed since the subscriber was installed.
        pub(crate) fn warn(&self) -> usize {
            self.warn.load(std::sync::atomic::Ordering::SeqCst)
        }

        /// Events at ERROR level observed since the subscriber was installed.
        pub(crate) fn error(&self) -> usize {
            self.error.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Level-counting layer over a thread-local default subscriber — same
    /// mechanism as camel-core's `capture_warns` precedent. Counts WARN and
    /// ERROR separately so quietness pins can assert exact counts (the
    /// logging contract distinguishes "no error!" from "no repeated warn!").
    struct LevelCountLayer {
        warn: Arc<std::sync::atomic::AtomicUsize>,
        error: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl<C> tracing_subscriber::Layer<C> for LevelCountLayer
    where
        C: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, C>,
        ) {
            match *event.metadata().level() {
                tracing::Level::WARN => {
                    self.warn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                tracing::Level::ERROR => {
                    self.error.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                _ => {}
            }
        }
    }

    /// Install a thread-local WARN/ERROR counting subscriber.
    pub(crate) fn event_counting_subscriber() -> (EventCounts, tracing::subscriber::DefaultGuard) {
        use tracing_subscriber::prelude::*;
        ensure_global_tracing_default();
        let counts = EventCounts {
            warn: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            error: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let layer = LevelCountLayer {
            warn: Arc::clone(&counts.warn),
            error: Arc::clone(&counts.error),
        };
        let guard = tracing_subscriber::registry().with(layer).set_default();
        (counts, guard)
    }

    /// Test double for `TokenAuthenticator`, shared by unit tests across this
    /// crate. `subject` lets tests distinguish instances by resolved identity.
    pub(crate) fn test_authenticator(subject: &str) -> Arc<dyn camel_auth::TokenAuthenticator> {
        Arc::new(TestAuthenticator {
            subject: subject.to_string(),
        })
    }

    struct TestAuthenticator {
        subject: String,
    }

    #[async_trait::async_trait]
    impl camel_auth::TokenAuthenticator for TestAuthenticator {
        async fn authenticate_bearer(
            &self,
            _token: &str,
        ) -> Result<camel_api::security_policy::Principal, camel_api::CamelError> {
            Ok(camel_api::security_policy::Principal {
                subject: self.subject.clone(),
                issuer: "test-issuer".into(),
                audience: vec![],
                scopes: vec!["read:api".into()],
                roles: vec!["admin".into()],
                claims: serde_json::Value::Null,
            })
        }
    }
}
