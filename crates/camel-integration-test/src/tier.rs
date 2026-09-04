//! Pure tier derivation (ADR-0069 section 1).
//!
//! A document's tier is a pure, total function of its content: no field
//! declares it. A `scenario:` section forces [`Tier::Full`] without
//! condition. Otherwise the function computes the endpoint-scheme closure
//! over every route source (each route's `from`, its error handler's
//! dead-letter URI, and every step URI, nested steps traversed
//! recursively), subtracts endpoints exactly replaced by a `skipTo`
//! intercept, adds the unit-tier schemes, and derives [`Tier::Lean`] only
//! when the whole closure stays within the lean scheme set. The function
//! does no I/O, reads no environment, and reads no clock.

use std::collections::BTreeSet;

use camel_core::intercept::InterceptAction;
use camel_core::{BuilderStep, RouteDefinition};

/// Schemes the lean boot registers (ADR-0064). The tier function never
/// grows this set; a scheme outside it forces the full boot.
const LEAN_SCHEMES: [&str; 5] = ["direct", "log", "mock", "seda", "timer"];

/// The derived execution profile of a test document (ADR-0069 section 1).
/// Content-derived: no field declares it, and the tier filters assert on
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Tier {
    /// The document's closure stays within the lean scheme set; it boots
    /// the lean boot, byte-identical registry.
    Lean,
    /// The document needs the full runtime boot.
    Full,
}

/// The document-level inputs to tier derivation.
///
/// `intercepts` comes from the unit-tier document model
/// (`parse_test_document` output); the scenario parser bans `intercepts`,
/// but `derive_tier` serves both tiers' documents. `unit_schemes` are the
/// schemes named by `inputs` and `expects`.
#[derive(Debug, Clone, Copy)]
pub struct DocumentInputs<'a> {
    /// The document declares a `scenario:` section; forces
    /// [`Tier::Full`] without condition.
    pub has_scenario: bool,
    /// Intercept map keyed by source URI, verbatim.
    pub intercepts: &'a [(String, InterceptAction)],
    /// Schemes named by `inputs`/`expects`.
    pub unit_schemes: &'a [String],
}

/// Derives a document's tier (ADR-0069 section 1). Pure and total: no
/// I/O, no environment reads, no clock.
///
/// A `scenario:` section forces [`Tier::Full`]. Otherwise the closure is
/// the schemes of every endpoint the routes touch (each route's `from`,
/// its error handler's dead-letter URI, and every step URI, nested steps
/// traversed recursively), minus endpoints exactly replaced by a
/// `skipTo` intercept, plus the unit schemes. Dynamic-dispatch steps
/// force [`Tier::Full`] wherever they appear: their target scheme is not
/// knowable before run time.
pub fn derive_tier(routes: &[RouteDefinition], doc: &DocumentInputs<'_>) -> Tier {
    if doc.has_scenario {
        return Tier::Full;
    }
    let mut uris: Vec<&str> = Vec::new();
    let mut dynamic_dispatch = false;
    for route in routes {
        uris.push(route.from_uri());
        // The dead-letter URI runs on the error path; the lean boot holds
        // no non-lean component, so it joins the closure like any other
        // endpoint URI.
        if let Some(dlc) = route
            .error_handler_config()
            .and_then(|handler| handler.dlc_uri.as_deref())
        {
            uris.push(dlc);
        }
        walk_steps(route.steps(), &mut uris, &mut dynamic_dispatch);
        // Circuit-breaker fallback sub-pipelines compile alongside the
        // main steps; walk them too.
        walk_steps(
            route.circuit_breaker_fallback(),
            &mut uris,
            &mut dynamic_dispatch,
        );
    }
    // Only an exact `skipTo` replacement subtracts: the original send is
    // skipped, so the intercepted endpoint never runs. A `divertCopyTo`
    // delivers a copy while the real send continues, so it subtracts
    // nothing. Matching is verbatim string equality; query parameters
    // are significant.
    let replaced_by_skip_to = |uri: &str| {
        doc.intercepts
            .iter()
            .any(|(key, action)| key == uri && matches!(action, InterceptAction::SkipTo { .. }))
    };
    let mut schemes: BTreeSet<&str> = doc.unit_schemes.iter().map(String::as_str).collect();
    // Explicit encoding of ADR-0069 section 1: "placeholder-in-scheme
    // forces FULL". The outcome is subsumed by the lean-literal check (a
    // placeholder scheme is never in the lean set); kept for spec
    // traceability and as a named regression site.
    let mut placeholder_in_scheme = false;
    for uri in uris.into_iter().filter(|uri| !replaced_by_skip_to(uri)) {
        let head = scheme_head(uri);
        if head.contains("${") || head.contains("{{") {
            placeholder_in_scheme = true;
        }
        schemes.insert(head);
    }
    if dynamic_dispatch || placeholder_in_scheme {
        return Tier::Full;
    }
    if schemes.iter().any(|scheme| !LEAN_SCHEMES.contains(scheme)) {
        return Tier::Full;
    }
    Tier::Lean
}

/// The scheme position of a URI: the text before the first `:`, or the
/// whole text when the URI carries no colon (which never matches the
/// lean set, so a scheme-less URI is conservatively full).
fn scheme_head(uri: &str) -> &str {
    match uri.split_once(':') {
        Some((head, _)) => head,
        None => uri,
    }
}

/// Walks one step list, recursing into every nested step list.
///
/// The match is exhaustive with NO `_` catch-all arm, mirroring the
/// bean-call walk: a future [`BuilderStep`] variant that holds a URI or
/// nested steps becomes a compile error here instead of a silently
/// un-walked location. A new dynamic-dispatch step must join the
/// dispatch arm so it keeps forcing [`Tier::Full`] (ADR-0069
/// consequences).
fn walk_steps<'a>(steps: &'a [BuilderStep], uris: &mut Vec<&'a str>, dynamic_dispatch: &mut bool) {
    for step in steps {
        match step {
            // Steps carrying a URI: the endpoint joins the closure.
            BuilderStep::To(uri)
            | BuilderStep::WireTap { uri }
            | BuilderStep::Enrich { uri, .. }
            | BuilderStep::PollEnrich { uri, .. } => uris.push(uri),

            // Dynamic dispatch: the target scheme is computed from the
            // exchange at run time and forces the full boot.
            BuilderStep::RecipientList { .. }
            | BuilderStep::DeclarativeRecipientList { .. }
            | BuilderStep::RoutingSlip { .. }
            | BuilderStep::DeclarativeRoutingSlip { .. }
            | BuilderStep::DynamicRouter { .. }
            | BuilderStep::DeclarativeDynamicRouter { .. } => *dynamic_dispatch = true,

            // Single nested `steps` child list.
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
            | BuilderStep::IdempotentConsumer { steps, .. } => {
                walk_steps(steps, uris, dynamic_dispatch);
            }

            // Choice shapes: when-clause sub-pipelines plus optional
            // otherwise branch (declarative and programmatic forms).
            BuilderStep::DeclarativeChoice { whens, otherwise } => {
                for when in whens {
                    walk_steps(&when.steps, uris, dynamic_dispatch);
                }
                if let Some(steps) = otherwise {
                    walk_steps(steps, uris, dynamic_dispatch);
                }
            }
            BuilderStep::Choice { whens, otherwise } => {
                for when in whens {
                    walk_steps(&when.steps, uris, dynamic_dispatch);
                }
                if let Some(steps) = otherwise {
                    walk_steps(steps, uris, dynamic_dispatch);
                }
            }

            BuilderStep::Cache { on_miss, .. } => {
                walk_steps(on_miss, uris, dynamic_dispatch);
            }

            BuilderStep::DeclarativeDoTry {
                try_steps,
                catch,
                finally,
            } => {
                walk_steps(try_steps, uris, dynamic_dispatch);
                for clause in catch {
                    walk_steps(&clause.steps, uris, dynamic_dispatch);
                }
                if let Some(finally) = finally {
                    walk_steps(&finally.steps, uris, dynamic_dispatch);
                }
            }

            // Leaf variants: hold no URI and no nested step list.
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
            | BuilderStep::Aggregate { .. }
            | BuilderStep::DeclarativeLog { .. }
            | BuilderStep::Bean { .. }
            | BuilderStep::Script { .. }
            | BuilderStep::Delay { .. }
            | BuilderStep::Validate { .. }
            | BuilderStep::ClaimCheck { .. }
            | BuilderStep::Sampling { .. }
            | BuilderStep::Sort { .. }
            | BuilderStep::CacheInvalidate { .. }
            | BuilderStep::CacheClear { .. }
            | BuilderStep::CacheStats { .. }
            | BuilderStep::CachePeekStale { .. }
            | BuilderStep::Resequence { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{DocumentInputs, Tier, derive_tier};
    use camel_api::error_handler::ErrorHandlerConfig;
    use camel_api::recipient_list::RecipientListConfig;
    use camel_api::{DynamicRouterConfig, RoutingSlipConfig};
    use camel_core::intercept::InterceptAction;
    use camel_core::{BuilderStep, RouteDefinition};

    /// Unit-tier defaults: `inputs` name `direct:`, `expects` name
    /// `mock:`. Both schemes are in the lean set.
    fn unit_schemes() -> Vec<String> {
        vec!["direct".to_string(), "mock".to_string()]
    }

    fn route(from: &str, steps: Vec<BuilderStep>) -> RouteDefinition {
        RouteDefinition::new(from, steps)
    }

    fn lean_route() -> RouteDefinition {
        route("direct:start", vec![BuilderStep::To("mock:out".into())])
    }

    fn inputs<'a>(
        has_scenario: bool,
        intercepts: &'a [(String, InterceptAction)],
        schemes: &'a [String],
    ) -> DocumentInputs<'a> {
        DocumentInputs {
            has_scenario,
            intercepts,
            unit_schemes: schemes,
        }
    }

    #[test]
    fn tier_lean_document_stays_lean() {
        let routes = [lean_route()];
        let schemes = unit_schemes();
        let intercepts = Vec::new();
        let input = inputs(false, &intercepts, &schemes);
        assert_eq!(derive_tier(&routes, &input), Tier::Lean);
    }

    #[test]
    fn tier_skipto_subtracts_from_closure() {
        let routes = [route(
            "direct:start",
            vec![BuilderStep::To("kafka:orders".into())],
        )];
        let schemes = unit_schemes();
        let intercepts = vec![(
            "kafka:orders".to_string(),
            InterceptAction::SkipTo {
                uri: "mock:orders".into(),
            },
        )];
        let input = inputs(false, &intercepts, &schemes);
        assert_eq!(derive_tier(&routes, &input), Tier::Lean);

        // Verbatim key match, query parameters significant: an intercept
        // keyed with a query string does not match a URI without one, so
        // `kafka` stays in the closure.
        let mismatched = vec![(
            "kafka:orders?option=1".to_string(),
            InterceptAction::SkipTo {
                uri: "mock:orders".into(),
            },
        )];
        let input = inputs(false, &mismatched, &schemes);
        assert_eq!(derive_tier(&routes, &input), Tier::Full);
    }

    #[test]
    fn tier_dlc_uri_counts_in_closure() {
        let schemes = unit_schemes();
        let intercepts = Vec::new();
        // The dead-letter URI runs on the error path; the lean boot holds
        // no non-lean component, so under-derivation here would fail at
        // error time. The URI joins the closure like any other endpoint.
        let kafka_dlq = [
            route("direct:start", vec![BuilderStep::To("mock:out".into())])
                .with_error_handler(ErrorHandlerConfig::dead_letter_channel("kafka:dlq")),
        ];
        let input = inputs(false, &intercepts, &schemes);
        assert_eq!(derive_tier(&kafka_dlq, &input), Tier::Full);

        // Closure inclusion is observable through the scheme rules: a
        // placeholder in the dead-letter URI's scheme position forces the
        // full tier exactly like a placeholder step URI.
        let placeholder_dlq = [
            route("direct:start", vec![BuilderStep::To("mock:out".into())])
                .with_error_handler(ErrorHandlerConfig::dead_letter_channel("${env:DLQ}:dead")),
        ];
        let input = inputs(false, &intercepts, &schemes);
        assert_eq!(derive_tier(&placeholder_dlq, &input), Tier::Full);

        // A `mock:` dead-letter URI mirrors how from/step URIs treat
        // `mock:`: it contributes to the closure and stays within the
        // lean set.
        let mock_dlq = [
            route("direct:start", vec![BuilderStep::To("mock:out".into())])
                .with_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc")),
        ];
        let input = inputs(false, &intercepts, &schemes);
        assert_eq!(derive_tier(&mock_dlq, &input), Tier::Lean);
    }

    #[test]
    fn tier_divertcopyto_does_not_subtract() {
        let routes = [route(
            "direct:start",
            vec![BuilderStep::To("kafka:orders".into())],
        )];
        let schemes = unit_schemes();
        let intercepts = vec![(
            "kafka:orders".to_string(),
            InterceptAction::DivertCopyTo {
                uri: "mock:mirror".into(),
            },
        )];
        let input = inputs(false, &intercepts, &schemes);
        assert_eq!(derive_tier(&routes, &input), Tier::Full);
    }

    #[test]
    fn tier_placeholder_in_scheme_forces_full() {
        let routes = [route(
            "direct:start",
            vec![BuilderStep::To("${env:TARGET_SCHEME}:host".into())],
        )];
        let schemes = unit_schemes();
        let intercepts = Vec::new();
        let input = inputs(false, &intercepts, &schemes);
        assert_eq!(derive_tier(&routes, &input), Tier::Full);
    }

    #[test]
    fn tier_dynamic_dispatch_forces_full() {
        let schemes = unit_schemes();
        let intercepts = Vec::new();
        // The DSL has no dedicated `toD` step yet; a toD-style target
        // computed from the exchange at run time is a URI whose scheme is
        // resolved at run time, represented here by a scheme placeholder.
        let cases: [(&str, BuilderStep); 4] = [
            (
                "recipient_list",
                BuilderStep::RecipientList {
                    config: RecipientListConfig::new(Arc::new(|_| "mock:one".to_string())),
                },
            ),
            (
                "routing_slip",
                BuilderStep::RoutingSlip {
                    config: RoutingSlipConfig::new(Arc::new(|_| Some("mock:one".to_string()))),
                },
            ),
            (
                "dynamic_router",
                BuilderStep::DynamicRouter {
                    config: DynamicRouterConfig::new(Arc::new(|_| Some("mock:one".to_string()))),
                },
            ),
            ("to_d", BuilderStep::To("${env:SCHEME}:orders".into())),
        ];
        for (name, step) in cases {
            let routes = [route("direct:start", vec![step])];
            let input = inputs(false, &intercepts, &schemes);
            assert_eq!(derive_tier(&routes, &input), Tier::Full, "case {name}");
        }
    }

    #[test]
    fn tier_scenario_section_forces_full() {
        let routes = [lean_route()];
        let schemes = unit_schemes();
        let intercepts = Vec::new();
        let input = inputs(true, &intercepts, &schemes);
        assert_eq!(derive_tier(&routes, &input), Tier::Full);
    }

    #[test]
    fn tier_all_route_sources_count() {
        let schemes = unit_schemes();
        let intercepts = Vec::new();
        // Every route source collapses to the same `[RouteDefinition]`
        // slice before tier derivation runs; both halves assert that every
        // route in the slice participates in the closure, identically to
        // the `routeFiles` source.
        for source in ["inline", "routeFilesFromRoot"] {
            let all_lean = [
                lean_route(),
                route("direct:poll", vec![BuilderStep::To("seda:pool".into())]),
            ];
            let input = inputs(false, &intercepts, &schemes);
            assert_eq!(
                derive_tier(&all_lean, &input),
                Tier::Lean,
                "source {source}"
            );

            let one_full = [
                lean_route(),
                route("direct:ship", vec![BuilderStep::To("kafka:orders".into())]),
            ];
            let input = inputs(false, &intercepts, &schemes);
            assert_eq!(
                derive_tier(&one_full, &input),
                Tier::Full,
                "source {source}"
            );
        }
    }
}
