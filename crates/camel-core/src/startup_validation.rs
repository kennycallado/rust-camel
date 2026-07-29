//! ADR-0033 startup-validation phase.
//!
//! `ConfigCheck` impls are registered for Require-Explicit-Choice security
//! defaults. The fail-closed `run_startup_validation()` walks the registry
//! and returns `Err(CamelError::Config)` if any check fails. See ADR-0033 in
//! `docs/adr/0033-…`.

use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinition};
use camel_api::{CamelError, ConfigValidationError};
use camel_endpoint::uri::parse_bool_param;

/// A single startup-time configuration check.
///
/// Each `ConfigCheck` is run as part of the fail-closed startup-validation phase
/// established by ADR-0033. The check owns a name (for error reporting), a human
/// description, and a synchronous `run` that returns `Ok(())` if the check passes
/// or `Err(CamelError::Config)` if the check fails and the operator config is
/// invalid.
pub trait ConfigCheck: Send + Sync {
    /// Stable identifier for this check (e.g. `"grpc-tls"`, `"sql-dynamic-query"`).
    /// Used in error messages and `camel doctor` output.
    fn name(&self) -> &'static str;

    /// One-line human description of what this check enforces.
    fn description(&self) -> &'static str;

    /// Run the check. Returns `Err(CamelError::Config(_))` to fail-closed.
    fn run(&self) -> Result<(), CamelError>;
}

/// Aggregated result of running every registered `ConfigCheck`.
///
/// Batch 1 ships the type so later batches can populate `failures`. Skeleton
/// phase always produces a `StartupValidationReport { failures: vec![] }`.
#[derive(Debug, Default, Clone)]
pub struct StartupValidationReport {
    /// Names of every `ConfigCheck` that returned `Err`, in registration order.
    pub failures: Vec<String>,
}

impl StartupValidationReport {
    /// True when no check failed. Caller MUST refuse to start the runtime when
    /// this is `false`.
    pub fn is_ok(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Entry point for the startup-validation phase.
///
/// Walks the supplied `ConfigCheck` registry, collects failures, and fails closed
/// when any check returns `Err`. Established by ADR-0033.
pub fn run_startup_validation(
    checks: Vec<Box<dyn ConfigCheck>>,
) -> Result<StartupValidationReport, CamelError> {
    let mut report = StartupValidationReport::default();
    for check in &checks {
        if let Err(e) = check.run() {
            report.failures.push(format!("{}: {}", check.name(), e));
        }
    }
    if !report.is_ok() {
        return Err(CamelError::Config(report.failures.join("; ")));
    }
    Ok(report)
}

/// ConfigCheck: SQL dynamic-query intent must match capability (H7).
///
/// If `use_message_body_for_sql=true` but `allow_dynamic_query=false`,
/// the operator intent (use body as query) is blocked by the capability
/// gate — this is a misconfiguration that would silently produce empty
/// queries. Fail closed at startup instead.
pub struct SqlDynamicQueryCheck {
    pub use_message_body_for_sql: bool,
    pub allow_dynamic_query: bool,
}

impl ConfigCheck for SqlDynamicQueryCheck {
    fn name(&self) -> &'static str {
        "sql-dynamic-query"
    }
    fn description(&self) -> &'static str {
        "SQL use_message_body_for_sql requires allow_dynamic_query=true"
    }
    fn run(&self) -> Result<(), CamelError> {
        if self.use_message_body_for_sql && !self.allow_dynamic_query {
            return Err(CamelError::from(
                ConfigValidationError::SqlDynamicQueryWithoutAllowDynamic,
            ));
        }
        Ok(())
    }
}

/// Scan a list of `RouteDefinition`s and emit a `SqlDynamicQueryCheck` for
/// every `sql:` URI whose parameters declare the dynamic-query intent
/// (`useMessageBodyForSql` / `allowDynamicQuery`).
///
/// Walked URIs: the route's `from_uri` plus every `BuilderStep::To` URI
/// reachable through structural step variants (filter, choice, split,
/// multicast, throttle, etc.). Dynamic-URI steps (routing slip, recipient
/// list, dynamic router) are skipped — their URIs are runtime-resolved and
/// cannot be statically validated.
///
/// Invalid URIs are silently ignored: this scanner is best-effort defense in
/// depth, not a primary security gate. The authoritative runtime check
/// (`SqlProducer::resolve_query_source`) closes the SQLi vector regardless
/// of what the static scanner finds here.
pub fn scan_route_definitions_for_sql_checks(
    routes: &[RouteDefinition],
) -> Vec<Box<dyn ConfigCheck>> {
    let mut out: Vec<Box<dyn ConfigCheck>> = Vec::new();
    for route in routes {
        collect_sql_checks_for_uri(route.from_uri(), &mut out);
        for step in route.steps() {
            for_each_step_uri(step, &mut |uri| collect_sql_checks_for_uri(uri, &mut out));
        }
    }
    out
}

/// Walk every statically declared URI reachable through `step` and invoke
/// `f` for each one. Variant coverage mirrors the prior
/// `walk_step_uris`: `To`, `WireTap`, `Enrich`, `PollEnrich`, plus all
/// structural sub-pipelines whose `steps` are known at parse time
/// (`Filter` / `DeclarativeFilter`, `Split` / `DeclarativeSplit` /
/// `DeclarativeStreamSplit`, `Multicast`, `Throttle`, `LoadBalance`,
/// `Loop` / `DeclarativeLoop`, `IdempotentConsumer`), the `Choice` and
/// `DeclarativeChoice` `whens` (and `otherwise`), and the
/// `DeclarativeDoTry` `try_steps` / `catch` / `finally` blocks.
///
/// Dynamic-URI steps (RoutingSlip, RecipientList, DynamicRouter, and the
/// declarative equivalents) are intentionally skipped — their URIs are
/// resolved at runtime and cannot be statically validated.
fn for_each_step_uri<F: FnMut(&str)>(step: &BuilderStep, f: &mut F) {
    match step {
        BuilderStep::To(uri) => f(uri),
        BuilderStep::WireTap { uri } | BuilderStep::Enrich { uri, .. } => {
            f(uri);
        }
        BuilderStep::PollEnrich { uri, .. } => {
            f(uri);
        }
        BuilderStep::Filter { steps, .. }
        | BuilderStep::DeclarativeFilter { steps, .. }
        | BuilderStep::Split { steps, .. }
        | BuilderStep::DeclarativeSplit { steps, .. }
        | BuilderStep::DeclarativeStreamSplit { steps, .. }
        | BuilderStep::Multicast { steps, .. }
        | BuilderStep::Throttle { steps, .. }
        | BuilderStep::LoadBalance { steps, .. }
        | BuilderStep::Loop { steps, .. }
        | BuilderStep::DeclarativeLoop { steps, .. }
        | BuilderStep::IdempotentConsumer { steps, .. } => {
            for s in steps {
                for_each_step_uri(s, f);
            }
        }
        BuilderStep::Choice { whens, otherwise } => {
            for when in whens {
                for s in &when.steps {
                    for_each_step_uri(s, f);
                }
            }
            if let Some(ow) = otherwise {
                for s in ow {
                    for_each_step_uri(s, f);
                }
            }
        }
        BuilderStep::DeclarativeChoice { whens, otherwise } => {
            for when in whens {
                for s in &when.steps {
                    for_each_step_uri(s, f);
                }
            }
            if let Some(ow) = otherwise {
                for s in ow {
                    for_each_step_uri(s, f);
                }
            }
        }
        BuilderStep::DeclarativeDoTry {
            try_steps,
            catch,
            finally,
        } => {
            for s in try_steps {
                for_each_step_uri(s, f);
            }
            for clause in catch {
                for s in &clause.steps {
                    for_each_step_uri(s, f);
                }
            }
            if let Some(fin) = finally {
                for s in &fin.steps {
                    for_each_step_uri(s, f);
                }
            }
        }
        // All remaining variants are process-mode (no static URI) or
        // runtime-resolved dynamic URIs (routing slip, recipient list,
        // dynamic router) — skip.
        _ => {}
    }
}

/// Return `true` if any route statically declares the given URI scheme.
///
/// Scans each route's `from_uri` plus every URI reachable through the
/// structural `BuilderStep` tree (see `for_each_step_uri`). Dynamic-URI
/// steps (RoutingSlip, RecipientList, DynamicRouter) do not contribute —
/// their URIs are runtime-resolved and cannot be statically validated.
///
/// Invalid URIs (those that fail `camel_endpoint::parse_uri`) are silently
/// skipped, matching the best-effort semantics of the SQL scanner. An
/// invalid `from` URI does NOT cause the route to be reported as
/// referencing the scheme; an invalid step URI is just ignored.
///
/// Used by the `camel run` startup guard to decide whether a route
/// references `exec` (a feature-gated component) so it can register the
/// `ExecBundle` only when needed — fixing the fail-closed
/// `ExecBundle::validate()` that previously aborted startup for any
/// route, even ones that never used `exec:`.
pub fn route_definitions_reference_scheme(routes: &[RouteDefinition], scheme: &str) -> bool {
    use std::cell::Cell;
    let found = Cell::new(false);
    let mut check = |uri: &str| {
        if found.get() {
            return;
        }
        if let Ok(parts) = camel_endpoint::parse_uri(uri)
            && parts.scheme == scheme
        {
            found.set(true);
        }
    };
    for route in routes {
        if found.get() {
            break;
        }
        check(route.from_uri());
        if found.get() {
            break;
        }
        for step in route.steps() {
            for_each_step_uri(step, &mut check);
            if found.get() {
                break;
            }
        }
    }
    found.get()
}

fn collect_sql_checks_for_uri(uri: &str, out: &mut Vec<Box<dyn ConfigCheck>>) {
    let Ok(parts) = camel_endpoint::parse_uri(uri) else {
        return;
    };
    if parts.scheme != "sql" {
        return;
    }
    // Match the camel-sql parser: keys are case-preserved by parse_uri (it
    // only decodes percent-encoding), and the SQL config looks them up by
    // their camelCase name (`useMessageBodyForSql`, `allowDynamicQuery`).
    let use_body = parts
        .params
        .get("useMessageBodyForSql")
        .and_then(|v| parse_bool_param(v).ok())
        .unwrap_or(false);
    let allow_dynamic = parts
        .params
        .get("allowDynamicQuery")
        .and_then(|v| parse_bool_param(v).ok())
        .unwrap_or(false);
    if use_body || allow_dynamic {
        // Only emit a check when the operator declared an intent that
        // participates in the matrix. Endpoints with neither flag set
        // cannot be misconfigured in the H7 sense — skip the noise.
        out.push(Box::new(SqlDynamicQueryCheck {
            use_message_body_for_sql: use_body,
            allow_dynamic_query: allow_dynamic,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: the empty registry returns `Ok` with an empty failure list.
    #[test]
    fn empty_registry_returns_empty_ok_report() {
        let report = run_startup_validation(vec![]).expect("empty registry must return Ok");
        assert!(report.is_ok());
        assert!(report.failures.is_empty());
    }

    /// Trait smoke test: a trivial `ConfigCheck` impl can be defined by callers
    /// and dispatched dynamically. This guards the trait surface against
    /// accidental breaking changes between batches.
    #[test]
    fn trait_is_object_safe_and_dispatchable() {
        struct AlwaysOk;
        impl ConfigCheck for AlwaysOk {
            fn name(&self) -> &'static str {
                "always-ok"
            }
            fn description(&self) -> &'static str {
                "always-ok skeleton check"
            }
            fn run(&self) -> Result<(), CamelError> {
                Ok(())
            }
        }

        let check: Box<dyn ConfigCheck> = Box::new(AlwaysOk);
        assert_eq!(check.name(), "always-ok");
        assert!(check.run().is_ok());
    }

    /// H7: SQL endpoint with `use_message_body_for_sql=true` and
    /// `allow_dynamic_query=false` is a misconfiguration that would silently
    /// produce empty queries — fail closed at startup.
    #[test]
    fn sql_dynamic_query_check_refuses_startup() {
        let check = SqlDynamicQueryCheck {
            use_message_body_for_sql: true,
            allow_dynamic_query: false,
        };
        let result = run_startup_validation(vec![Box::new(check)]);
        match result {
            Err(CamelError::Config(msg)) => {
                assert!(msg.contains("sql-dynamic-query"));
                assert!(msg.contains("allow_dynamic_query"));
            }
            other => panic!("expected CamelError::Config, got {other:?}"),
        }
    }

    /// H7: the inner `SqlDynamicQueryCheck::run()` returns the typed
    /// `ConfigValidationError::SqlDynamicQueryWithoutAllowDynamic` so
    /// operators can match on the variant directly. The outer
    /// `run_startup_validation` wraps it into a `Config(String)` joined
    /// report (asserted above); this test guards the typed inner path
    /// so the promotion to typed errors (rc-r8fd) doesn't regress.
    #[test]
    fn sql_dynamic_query_check_run_returns_typed_error() {
        let check = SqlDynamicQueryCheck {
            use_message_body_for_sql: true,
            allow_dynamic_query: false,
        };
        let result = check.run();
        assert!(
            matches!(
                result,
                Err(CamelError::ConfigValidation(
                    ConfigValidationError::SqlDynamicQueryWithoutAllowDynamic,
                ))
            ),
            "expected ConfigValidation(SqlDynamicQueryWithoutAllowDynamic), got: {result:?}"
        );
    }

    /// H7: explicit opt-in satisfies the check — startup proceeds.
    #[test]
    fn sql_dynamic_query_check_passes_with_opt_in() {
        let check = SqlDynamicQueryCheck {
            use_message_body_for_sql: true,
            allow_dynamic_query: true,
        };
        let report =
            run_startup_validation(vec![Box::new(check)]).expect("opt-in must satisfy the check");
        assert!(report.is_ok());
    }

    /// H7: when `use_message_body_for_sql=false`, the dynamic-query gate
    /// is irrelevant and the check passes regardless of the opt-in flag.
    #[test]
    fn sql_dynamic_query_check_passes_without_body_sql() {
        let check = SqlDynamicQueryCheck {
            use_message_body_for_sql: false,
            allow_dynamic_query: false,
        };
        let report = run_startup_validation(vec![Box::new(check)])
            .expect("no body-sourced queries → no fail-closed condition");
        assert!(report.is_ok());
    }

    /// Scanner: a route with a `from: sql:...` URI whose params declare
    /// `useMessageBodyForSql=true` but no `allowDynamicQuery` must produce
    /// a failing `SqlDynamicQueryCheck`.
    #[test]
    fn scanner_flags_from_sql_endpoint_with_body_but_no_allow() {
        let route = RouteDefinition::new(
            "sql:select * from t?db_url=postgres://x/y&useMessageBodyForSql=true",
            vec![],
        )
        .with_route_id("r".to_string());
        let checks = scan_route_definitions_for_sql_checks(&[route]);
        assert_eq!(checks.len(), 1);
        assert!(run_startup_validation(checks).is_err());
    }

    /// Scanner: a route with a `to: sql:...` step in the top-level step
    /// vector is found.
    #[test]
    fn scanner_walks_top_level_to_sql_step() {
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::To(
                "sql:select 1?db_url=postgres://x/y&useMessageBodyForSql=true".to_string(),
            )],
        )
        .with_route_id("r".to_string());
        let checks = scan_route_definitions_for_sql_checks(&[route]);
        assert_eq!(checks.len(), 1);
        assert!(run_startup_validation(checks).is_err());
    }

    /// Scanner: a route with no SQL URIs produces no checks.
    #[test]
    fn scanner_emits_nothing_for_non_sql_route() {
        let route = RouteDefinition::new("timer:tick?period=1000", vec![]);
        let checks = scan_route_definitions_for_sql_checks(&[route]);
        assert!(checks.is_empty());
    }

    /// Scanner: a SQL endpoint without the body-mode flag is not flagged
    /// (the operator has not opted into dynamic queries, so there is no
    /// intent to validate against the capability gate).
    #[test]
    fn scanner_skips_sql_endpoint_without_dynamic_intent() {
        let route = RouteDefinition::new("sql:select 1?db_url=postgres://x/y", vec![]);
        let checks = scan_route_definitions_for_sql_checks(&[route]);
        assert!(checks.is_empty());
    }

    // -- exec-scheme scanner tests (Task 1.1) ---------------------------------
    //
    // These tests exercise `route_definitions_reference_scheme`, the reusable
    // scanner that the `camel run` startup guard calls to decide whether a
    // route statically references the `exec` scheme. The scanner walks every
    // structural `BuilderStep` variant whose URI is known at parse time, so
    // each variant needs a coverage test. Dynamic-URI steps
    // (RoutingSlip/RecipientList/DynamicRouter) must report `false` because
    // their URIs are resolved at runtime and cannot be statically validated.

    /// From-URI match: a route whose `from` is `exec:...` must be detected.
    #[test]
    fn scheme_scanner_detects_exec_from_uri() {
        let route = RouteDefinition::new("exec:echo", vec![]).with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// Top-level `To` step: a non-exec source with a `To("exec:...")` step.
    #[test]
    fn scheme_scanner_detects_exec_in_to_step() {
        let route = RouteDefinition::new(
            "timer:tick?period=500",
            vec![BuilderStep::To("exec:echo".to_string())],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `WireTap` variant — static URI in named field.
    #[test]
    fn scheme_scanner_detects_exec_in_wiretap() {
        let route = RouteDefinition::new(
            "timer:tick",
            vec![BuilderStep::WireTap {
                uri: "exec:audit".to_string(),
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `Enrich` variant — static URI in named field alongside strategy/timeout.
    #[test]
    fn scheme_scanner_detects_exec_in_enrich() {
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::Enrich {
                uri: "exec:enricher".to_string(),
                strategy: Some("agg".to_string()),
                timeout_ms: Some(1000),
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `PollEnrich` variant — static URI in named field.
    #[test]
    fn scheme_scanner_detects_exec_in_pollenrich() {
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::PollEnrich {
                uri: "exec:poller".to_string(),
                strategy: None,
                timeout_ms: Some(500),
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `Filter` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_filter() {
        use camel_api::FilterPredicate;
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::Filter {
                predicate: FilterPredicate::new(|_| true),
                steps: vec![BuilderStep::To("exec:echo".to_string())],
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `Split` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_split() {
        use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_lines};
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::Split {
                config: SplitterConfig::new(split_body_lines())
                    .aggregation(AggregationStrategy::Original),
                steps: vec![BuilderStep::To("exec:echo".to_string())],
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `Multicast` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_multicast() {
        use camel_api::MulticastConfig;
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::Multicast {
                steps: vec![BuilderStep::To("exec:echo".to_string())],
                config: MulticastConfig::new(),
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `Loop` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_loop() {
        use camel_api::loop_eip::{LoopConfig, LoopMode};
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::Loop {
                config: LoopConfig::new(LoopMode::Count(3)),
                steps: vec![BuilderStep::To("exec:echo".to_string())],
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `IdempotentConsumer` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_idempotent_consumer() {
        use crate::lifecycle::application::route_definition::LanguageExpressionDef;
        let expr = LanguageExpressionDef {
            language: "simple".into(),
            source: "${header.id}".into(),
        };
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::IdempotentConsumer {
                repository: "myRepo".to_string(),
                expression: expr,
                steps: vec![BuilderStep::To("exec:echo".to_string())],
                eager: false,
                remove_on_failure: false,
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `Throttle` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_throttle() {
        use camel_api::ThrottlerConfig;
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::Throttle {
                config: ThrottlerConfig::new(10, std::time::Duration::from_millis(10)),
                steps: vec![BuilderStep::To("exec:echo".to_string())],
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `LoadBalance` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_loadbalance() {
        use camel_api::LoadBalancerConfig;
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::LoadBalance {
                config: LoadBalancerConfig::round_robin(),
                steps: vec![BuilderStep::To("exec:echo".to_string())],
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `DeclarativeFilter` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_declarative_filter() {
        use crate::lifecycle::application::route_definition::LanguageExpressionDef;
        let expr = LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        };
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DeclarativeFilter {
                predicate: expr,
                steps: vec![BuilderStep::To("exec:echo".to_string())],
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `DeclarativeSplit` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_declarative_split() {
        use crate::lifecycle::application::route_definition::LanguageExpressionDef;
        use camel_api::splitter::AggregationStrategy;
        let expr = LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        };
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DeclarativeSplit {
                expression: expr,
                aggregation: AggregationStrategy::Original,
                parallel: false,
                parallel_limit: None,
                stop_on_exception: true,
                steps: vec![BuilderStep::To("exec:echo".to_string())],
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `DeclarativeStreamSplit` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_declarative_stream_split() {
        use camel_api::splitter::{AggregationStrategy, StreamSplitConfig, StreamSplitFormat};
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DeclarativeStreamSplit {
                stream_config: StreamSplitConfig {
                    format: StreamSplitFormat::Ndjson,
                    max_record_bytes: 1024,
                    batch_size: 1,
                    chunk_size: None,
                    include_origin: true,
                },
                aggregation: AggregationStrategy::Original,
                stop_on_exception: true,
                steps: vec![BuilderStep::To("exec:echo".to_string())],
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `DeclarativeLoop` sub-pipeline — recurse into `steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_declarative_loop() {
        use crate::lifecycle::application::route_definition::LanguageExpressionDef;
        let expr = LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        };
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DeclarativeLoop {
                count: Some(5),
                while_predicate: Some(expr),
                steps: vec![BuilderStep::To("exec:echo".to_string())],
                max_iterations: Some(100),
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `Choice` with a `when` branch that contains exec.
    #[test]
    fn scheme_scanner_detects_exec_in_choice_branch() {
        use crate::lifecycle::application::route_definition::WhenStep;
        use camel_api::FilterPredicate;
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::Choice {
                whens: vec![WhenStep {
                    predicate: FilterPredicate::new(|_| true),
                    steps: vec![BuilderStep::To("exec:echo".to_string())],
                }],
                otherwise: None,
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `Choice` with exec in the `otherwise` branch only.
    #[test]
    fn scheme_scanner_detects_exec_in_choice_otherwise() {
        use crate::lifecycle::application::route_definition::WhenStep;
        use camel_api::FilterPredicate;
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::Choice {
                whens: vec![WhenStep {
                    predicate: FilterPredicate::new(|_| false),
                    steps: vec![BuilderStep::To("log:info".to_string())],
                }],
                otherwise: Some(vec![BuilderStep::To("exec:echo".to_string())]),
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `DeclarativeChoice` with exec in a `when` branch.
    #[test]
    fn scheme_scanner_detects_exec_in_declarative_choice_when() {
        use crate::lifecycle::application::route_definition::{
            DeclarativeWhenStep, LanguageExpressionDef,
        };
        let expr = LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        };
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DeclarativeChoice {
                whens: vec![DeclarativeWhenStep {
                    predicate: expr,
                    steps: vec![BuilderStep::To("exec:echo".to_string())],
                }],
                otherwise: None,
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `DeclarativeChoice` with exec in `otherwise` only.
    #[test]
    fn scheme_scanner_detects_exec_in_declarative_choice_otherwise() {
        use crate::lifecycle::application::route_definition::{
            DeclarativeWhenStep, LanguageExpressionDef,
        };
        let expr = LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        };
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DeclarativeChoice {
                whens: vec![DeclarativeWhenStep {
                    predicate: expr,
                    steps: vec![BuilderStep::To("log:info".to_string())],
                }],
                otherwise: Some(vec![BuilderStep::To("exec:echo".to_string())]),
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `DeclarativeDoTry` with exec in `try_steps`.
    #[test]
    fn scheme_scanner_detects_exec_in_dotry() {
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DeclarativeDoTry {
                try_steps: vec![BuilderStep::To("exec:echo".to_string())],
                catch: vec![],
                finally: None,
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `DeclarativeDoTry` with exec in a `catch` clause.
    #[test]
    fn scheme_scanner_detects_exec_in_dotry_catch() {
        use crate::lifecycle::application::route_definition::DoTryCatchClauseBuilder;
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DeclarativeDoTry {
                try_steps: vec![BuilderStep::To("log:info".to_string())],
                catch: vec![DoTryCatchClauseBuilder {
                    exception: None,
                    when: None,
                    on_when: None,
                    disposition: camel_api::error_handler::ExceptionDisposition::Propagate,
                    steps: vec![BuilderStep::To("exec:echo".to_string())],
                }],
                finally: None,
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// `DeclarativeDoTry` with exec in `finally`.
    #[test]
    fn scheme_scanner_detects_exec_in_dotry_finally() {
        use crate::lifecycle::application::route_definition::DoTryFinallyBuilder;
        let route = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::DeclarativeDoTry {
                try_steps: vec![BuilderStep::To("log:info".to_string())],
                catch: vec![],
                finally: Some(DoTryFinallyBuilder {
                    on_when: None,
                    steps: vec![BuilderStep::To("exec:echo".to_string())],
                }),
            }],
        )
        .with_route_id("r".to_string());
        assert!(route_definitions_reference_scheme(&[route], "exec"));
    }

    /// Multi-route scan: only the second route references exec.
    #[test]
    fn scheme_scanner_detects_exec_across_multiple_routes() {
        let r1 = RouteDefinition::new("timer:tick", vec![]).with_route_id("r1".to_string());
        let r2 = RouteDefinition::new(
            "direct:start",
            vec![BuilderStep::To("exec:echo".to_string())],
        )
        .with_route_id("r2".to_string());
        assert!(route_definitions_reference_scheme(&[r1, r2], "exec"));
    }

    /// Negative: a non-exec route produces `false`.
    #[test]
    fn scheme_scanner_false_for_non_exec_route() {
        let route =
            RouteDefinition::new("timer:tick", vec![BuilderStep::To("log:info".to_string())])
                .with_route_id("r".to_string());
        assert!(!route_definitions_reference_scheme(&[route], "exec"));
    }

    /// Negative: dynamic-URI step (RoutingSlip) with a closure that returns
    /// `exec:echo` at runtime — the scanner cannot see through the closure.
    #[test]
    fn scheme_scanner_false_for_dynamic_uri_only() {
        use camel_api::RoutingSlipConfig;
        use std::sync::Arc;
        let route = RouteDefinition::new(
            "timer:tick",
            vec![BuilderStep::RoutingSlip {
                config: RoutingSlipConfig::new(Arc::new(|_| Some("exec:echo".to_string()))),
            }],
        )
        .with_route_id("r".to_string());
        assert!(!route_definitions_reference_scheme(&[route], "exec"));
    }
}
