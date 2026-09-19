//! stream:out / tracer-stdout collision warning (openspec change
//! `stream-component`, task 1.4).
//!
//! [`warn_stream_stdout_collision`] is the single-source helper that owns
//! both facts — the discovered routes and the tracer config — and warns
//! when a route writes to `stream:out` while the tracer stdout sink is
//! enabled. `camel run` and `camel job` call it after route load, before
//! context start. Posture: warn only, never auto-mux.

use camel_config::config::CamelConfig;
use camel_core::BuilderStep;

/// Warn when a discovered route writes to `stream:out` while the tracer
/// stdout sink is enabled — both would interleave tracer events with route
/// body output on the process stdout, corrupting piped output.
///
/// Posture: warn only, never auto-mux (openspec change `stream-component`).
/// Returns whether the collision condition held; `camel run` calls this
/// fire-and-forget right after route discovery, before context start.
///
/// Tracer condition mirrors `camel-config/src/context_ext.rs:791`
/// (`config.enabled && config.outputs.stdout.enabled`). The real field path
/// is the RAW tracer config that feeds `init_tracing_subscriber`:
/// `CamelConfig.observability.tracer` (`camel_config::config::
/// ObservabilityConfig.tracer: TracerConfig`, cloned before the
/// `effective_tracer_config` assembly), down through `TracerConfig.outputs
/// .stdout` — domain shape `StdoutOutput` at
/// `camel-core/src/shared/observability/domain/config.rs` (`StdoutOutput`,
/// serde-default enabled).
///
/// The route check is intentionally small and local: it scans every
/// [`BuilderStep`] tree reachable from each route — top-level steps, steps
/// nested inside structural containers (filter, split, choice, doTry, …)
/// and the circuit-breaker fallback pipeline — and matches a URI whose
/// query part is stripped down exactly to `stream:out` (scheme `stream`,
/// path `out`). `stream:out?…` collides; `stream:err`, `stream:file?…`, and
/// `stream:outx` do not. Dynamic-URI steps (routing slip, recipient list,
/// dynamic router) resolve at runtime and cannot be statically checked.
pub fn warn_stream_stdout_collision(
    routes: &[camel_core::RouteDefinition],
    config: &CamelConfig,
) -> bool {
    let tracer = &config.observability.tracer;
    if !tracer.enabled || !tracer.outputs.stdout.enabled {
        return false;
    }
    let hits: Vec<&str> = routes
        .iter()
        .filter(|route| {
            steps_contain_stream_out(route.steps())
                || steps_contain_stream_out(route.circuit_breaker_fallback())
        })
        .map(|route| route.route_id())
        .collect();
    if hits.is_empty() {
        return false;
    }
    tracing::warn!(
        "routes {hits:?} write to stream:out while the tracer stdout sink is \
         enabled ([observability.tracer] enabled with outputs.stdout on); \
         tracer events will interleave with route output on stdout — disable \
         [observability.tracer.outputs.stdout] or route the tracer to \
         stderr/file",
    );
    true
}

/// `true` when `uri` names the stream stdout endpoint: exact
/// `stream:out` after the `?` query part is stripped.
fn is_stream_out_uri(uri: &str) -> bool {
    uri.split('?').next() == Some("stream:out")
}

/// `true` when any step in `steps`, at any nesting depth, declares a
/// `to: stream:out` endpoint. Container variants descend into every
/// nested pipeline (filter/split sub-pipelines, choice `whens` and
/// `otherwise`, doTry clauses, cache `on_miss`, …), mirroring the
/// structural recursion of `camel-core/src/startup_validation.rs`
/// (`for_each_step_uri`). Exhaustive by design — no catch-all — so a
/// future `BuilderStep` variant forces a classification here.
fn steps_contain_stream_out(steps: &[BuilderStep]) -> bool {
    steps.iter().any(|step| match step {
        BuilderStep::To(uri) => is_stream_out_uri(uri),

        // Structural containers with a single child pipeline.
        BuilderStep::DeclarativeFilter { steps, .. }
        | BuilderStep::DeclarativeSplit { steps, .. }
        | BuilderStep::DeclarativeStreamSplit { steps, .. }
        | BuilderStep::Split { steps, .. }
        | BuilderStep::Filter { steps, .. }
        | BuilderStep::Multicast { steps, .. }
        | BuilderStep::Throttle { steps, .. }
        | BuilderStep::LoadBalance { steps, .. }
        | BuilderStep::Loop { steps, .. }
        | BuilderStep::DeclarativeLoop { steps, .. }
        | BuilderStep::IdempotentConsumer { steps, .. }
        | BuilderStep::Cache { on_miss: steps, .. } => steps_contain_stream_out(steps),

        // Choice variants: every `when` branch plus the `otherwise` branch.
        BuilderStep::Choice { whens, otherwise } => {
            whens
                .iter()
                .any(|when| steps_contain_stream_out(&when.steps))
                || otherwise.as_deref().is_some_and(steps_contain_stream_out)
        }
        BuilderStep::DeclarativeChoice { whens, otherwise } => {
            whens
                .iter()
                .any(|when| steps_contain_stream_out(&when.steps))
                || otherwise.as_deref().is_some_and(steps_contain_stream_out)
        }

        // doTry: the try body, every doCatch clause, and the doFinally body.
        BuilderStep::DeclarativeDoTry {
            try_steps,
            catch,
            finally,
        } => {
            steps_contain_stream_out(try_steps)
                || catch
                    .iter()
                    .any(|clause| steps_contain_stream_out(&clause.steps))
                || finally
                    .as_ref()
                    .is_some_and(|fin| steps_contain_stream_out(&fin.steps))
        }

        // Static-URI leaves: WireTap, Enrich, and PollEnrich carry a
        // fixed endpoint URI (no nested pipeline), visited by
        // `for_each_step_uri` in camel-core's startup_validation.rs.
        BuilderStep::WireTap { uri }
        | BuilderStep::Enrich { uri, .. }
        | BuilderStep::PollEnrich { uri, .. } => is_stream_out_uri(uri),

        // Leaves: no static `to` URI and no nested pipeline. Dynamic-URI
        // variants (routing slip, recipient list, dynamic router) resolve
        // at runtime and are intentionally not scanned.
        BuilderStep::Processor(_)
        | BuilderStep::Stop
        | BuilderStep::Log { .. }
        | BuilderStep::DeclarativeSetHeader { .. }
        | BuilderStep::DeclarativeSetHeaderIfAbsent { .. }
        | BuilderStep::DeclarativeRemoveHeader { .. }
        | BuilderStep::DeclarativeSetProperty { .. }
        | BuilderStep::DeclarativeSetBody { .. }
        | BuilderStep::DeclarativeScript { .. }
        | BuilderStep::DeclarativeFunction { .. }
        | BuilderStep::DeclarativeDynamicRouter { .. }
        | BuilderStep::DeclarativeRoutingSlip { .. }
        | BuilderStep::Aggregate { .. }
        | BuilderStep::DeclarativeLog { .. }
        | BuilderStep::Bean { .. }
        | BuilderStep::Script { .. }
        | BuilderStep::DynamicRouter { .. }
        | BuilderStep::RoutingSlip { .. }
        | BuilderStep::RecipientList { .. }
        | BuilderStep::DeclarativeRecipientList { .. }
        | BuilderStep::Delay { .. }
        | BuilderStep::Validate { .. }
        | BuilderStep::ClaimCheck { .. }
        | BuilderStep::Sampling { .. }
        | BuilderStep::Sort { .. }
        | BuilderStep::CacheInvalidate { .. }
        | BuilderStep::CacheClear { .. }
        | BuilderStep::CacheStats { .. }
        | BuilderStep::CachePeekStale { .. }
        | BuilderStep::Resequence { .. } => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- stream:out / tracer-stdout collision (stream-component task 1.4) ---

    /// Minimal route fixture: one consumer + one top-level `to` step.
    fn route_with_to_step(uri: &str) -> camel_core::RouteDefinition {
        camel_core::RouteDefinition::new("direct:start", vec![BuilderStep::To(uri.to_string())])
            .with_route_id("stream-out-probe")
    }

    /// Tracer configs parse through the serde boundary (the custom
    /// `TracerConfig` Deserialize detects explicit `enabled`), mirroring
    /// the `context_ext` test fixtures.
    fn config_from_toml(toml: &str) -> CamelConfig {
        toml::from_str(toml).expect("collision-test config must parse")
    }

    #[test]
    fn collision_warns_when_stream_out_and_tracer_stdout() {
        let config = config_from_toml(
            "[observability.tracer]\nenabled = true\n\
             [observability.tracer.outputs.stdout]\nenabled = true\n",
        );
        let routes = vec![route_with_to_step("stream:out")];
        assert!(warn_stream_stdout_collision(&routes, &config));
    }

    #[test]
    fn collision_absent_when_tracer_stdout_disabled() {
        let config = config_from_toml(
            "[observability.tracer]\nenabled = true\n\
             [observability.tracer.outputs.stdout]\nenabled = false\n",
        );
        let routes = vec![route_with_to_step("stream:out")];
        assert!(!warn_stream_stdout_collision(&routes, &config));
    }

    #[test]
    fn collision_absent_without_stream_out_route() {
        let config = config_from_toml(
            "[observability.tracer]\nenabled = true\n\
             [observability.tracer.outputs.stdout]\nenabled = true\n",
        );
        let routes = vec![route_with_to_step("log:x")];
        assert!(!warn_stream_stdout_collision(&routes, &config));
    }

    #[test]
    fn collision_warns_for_nested_stream_out() {
        let config = config_from_toml(
            "[observability.tracer]\nenabled = true\n\
             [observability.tracer.outputs.stdout]\nenabled = true\n",
        );
        // `to: stream:out` buried inside a DeclarativeFilter sub-pipeline:
        // the top-level steps list holds no `To` step at all.
        let route = camel_core::RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DeclarativeFilter {
                predicate: camel_api::declarative::LanguageExpressionDef {
                    language: "simple".to_string(),
                    source: "${body} != null".to_string(),
                },
                steps: vec![BuilderStep::To("stream:out".to_string())],
            }],
        )
        .with_route_id("nested-stream-out-probe");
        assert!(warn_stream_stdout_collision(&[route], &config));
    }

    #[test]
    fn collision_warns_for_wiretap_stream_out() {
        let config = config_from_toml(
            "[observability.tracer]\nenabled = true\n\
             [observability.tracer.outputs.stdout]\nenabled = true\n",
        );
        // WireTap carries a static URI (visited by `for_each_step_uri`),
        // so a `wireTap: stream:out` tee must trip the collision warn even
        // though the top-level steps list holds no `To` step.
        let route = camel_core::RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::WireTap {
                uri: "stream:out".to_string(),
            }],
        )
        .with_route_id("wiretap-stream-out-probe");
        assert!(warn_stream_stdout_collision(&[route], &config));
    }
}
