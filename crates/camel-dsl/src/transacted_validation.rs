//! Static, fail-closed validation of `transacted:` blocks on the declarative model.
//!
//! Task 3.1 (sqltrans, slice 1): `validate_transacted_blocks` inspects the
//! declarative route AST — not the compiled `BuilderStep` tree — and rejects
//! every block shape that cannot honour a block-owned transaction. The walk is
//! syntactic: it reports static presence anywhere in the block body and never
//! reasons about reachability through filters, choices or expressions. That
//! keeps the rule fail-closed: a shape the validator cannot prove safe is
//! rejected.
//!
//! Per block, the checks run in this order:
//!
//! 1. nesting — a `transacted` block inside another one (`NestedBlock`);
//! 2. `outputType=StreamList` on any sql step anywhere in the body
//!    (`StreamListInBlock`, unconditional — no consumer step needs to exist);
//!    every step with a static URI (`to`, `enrich`, `poll_enrich`, `wire_tap`)
//!    feeds this rule and the datasource rules below;
//! 3. structural reachability — parallel split, multicast, wire tap, async
//!    producer scheme and `seda:` handoffs that could carry a joined sql step
//!    out of the transaction scope;
//! 4. datasource identity — foreign datasource steps (`ForeignDatasource`) and
//!    every sql step that names no datasource (`UnnamedDatasource`). The
//!    unnamed rule is unconditional, not `db_url`-specific: a bare
//!    `sql:select 1`, an inline `db_url` step and a mangled query whose
//!    parameters cannot be recovered all fail closed rather than fall through
//!    to an implicit datasource.
//!
//! Manager resolution is NOT a gate check: it runs as the tail of the ONE
//! lowering walk (`camel_core::transacted_walk::walk_transacted_block_deferred`,
//! which takes the resolver), after the per-arm SQL checks — the frozen walk
//! precedence (per-arm BEFORE manager). Keeping resolution in this AST gate
//! instead would let `ManagerMissing` (always-absent resolver in production)
//! mask a per-arm rejection of the same block.
//!
//! Scope of the walk: [`validate_transacted_blocks`] inspects the whole
//! declarative route, not only `route.steps`. Every
//! `error_handler.on_exceptions[].steps` clause and the
//! `circuit_breaker.fallback` body are walked with the same block rules.
//! Error-handler lowering keeps only `Processor` steps, so a block authored in
//! an `on_exceptions` clause would otherwise be dropped silently at load; the
//! load-time rejection keeps it typed.
//!
//! Step identity in the reported errors:
//!
//! - a sql step reports its full authored URI (`sql:select 1?datasource=pg`);
//! - a parallel split reports `split`, a multicast reports `multicast`;
//! - a wire tap reports the tap URI, and an async/seda handoff reports the
//!   `to:` URI.
//!
//! URI parsing: scheme detection is local (text before the first `:`), while
//! query parameters come from [`camel_endpoint::parse_uri`] — the same parser
//! the endpoint layer uses. If that parser rejects the query (for example a
//! bare `sql:...?datasource` key), the parameters are recovered by a literal
//! `&`/`=` scan so a malformed query can never hide a sql step from a
//! fail-closed check.

use camel_api::CamelError;
use camel_api::transaction::TransactionError;

use crate::model::{DeclarativeRoute, DeclarativeStep, TransactedStepDef};

/// Facts validation needs about one authored sql step URI.
///
/// Only URIs whose scheme is `sql:` produce facts; every other step kind is
/// invisible to the sql rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlStepFacts {
    /// Value of the `datasource` parameter, when present.
    pub datasource: Option<String>,
    /// Whether the URI carries an inline `db_url` parameter at all.
    pub uses_db_url: bool,
    /// Whether `outputType` is exactly `StreamList`.
    pub stream_list: bool,
}

/// Extract the validation facts from one authored URI.
///
/// Returns `None` for any URI whose scheme is not `sql:` (case-insensitive).
pub fn sql_step_facts(uri: &str) -> Option<SqlStepFacts> {
    let (scheme, rest) = uri.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("sql") {
        return None;
    }

    let Some((_path, query)) = rest.split_once('?') else {
        return Some(SqlStepFacts {
            datasource: None,
            uses_db_url: false,
            stream_list: false,
        });
    };

    // Primary parse: the endpoint layer's own grammar (`scheme:path?params`).
    if let Ok(parsed) = camel_endpoint::parse_uri(uri) {
        return Some(SqlStepFacts {
            datasource: parsed.params.get("datasource").cloned(),
            uses_db_url: parsed.params.contains_key("db_url"),
            stream_list: parsed
                .params
                .get("outputType")
                .is_some_and(|value| value == "StreamList"),
        });
    }

    // Fallback: literal scan, no decoding. The three keys this validator reads
    // hold plain names/paths, so a malformed query elsewhere must not make the
    // sql step invisible to a fail-closed check.
    let mut facts = SqlStepFacts {
        datasource: None,
        uses_db_url: false,
        stream_list: false,
    };
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "datasource" => facts.datasource = Some(value.to_string()),
            "db_url" => facts.uses_db_url = true,
            "outputType" if value == "StreamList" => facts.stream_list = true,
            _ => {}
        }
    }
    Some(facts)
}

/// Validate every `transacted:` block in `route`.
///
/// Walks the declarative route in document order and validates EVERY block at
/// the point it is found — a route with several blocks validates all of them,
/// not just the first. The walk covers the route body, every
/// `error_handler.on_exceptions[].steps` clause and the
/// `circuit_breaker.fallback` body, so a block cannot hide in a child pipeline
/// of the handler machinery. Manager resolution is not part of this gate: it
/// runs as the lowering walk's tail (per-arm BEFORE manager — the frozen
/// precedence).
pub fn validate_transacted_blocks(route: &DeclarativeRoute) -> Result<(), CamelError> {
    validate_transacted_steps(&route.steps)?;
    if let Some(error_handler) = &route.error_handler {
        for clause in error_handler.on_exceptions.iter().flatten() {
            validate_transacted_steps(&clause.steps)?;
        }
    }
    if let Some(circuit_breaker) = &route.circuit_breaker {
        validate_transacted_steps(&circuit_breaker.fallback)?;
    }
    Ok(())
}

/// Validate every `transacted:` block reachable from one step sequence.
fn validate_transacted_steps(steps: &[DeclarativeStep]) -> Result<(), CamelError> {
    for step in steps {
        if let DeclarativeStep::Transacted(block) = step {
            validate_block(block)?;
        }
        for child in child_steps(step) {
            validate_transacted_steps(child)?;
        }
    }
    Ok(())
}

fn validate_block(block: &TransactedStepDef) -> Result<(), CamelError> {
    let datasource = block.datasource.as_str();

    // 1. Nesting: a block inside a block is rejected first, at the innermost
    // datasource name.
    walk_steps(&block.steps, &mut |step| {
        if let DeclarativeStep::Transacted(inner) = step {
            return Err(CamelError::Transaction(TransactionError::NestedBlock {
                inner: inner.datasource.clone(),
            }));
        }
        Ok(())
    })?;

    // 2. StreamList is rejected unconditionally: the streaming body cannot be
    // read inside a transaction that may still roll back.
    walk_steps(&block.steps, &mut |step| {
        if let Some(facts) = sql_step_facts_of(step)
            && facts.stream_list
        {
            return Err(CamelError::Transaction(
                TransactionError::StreamListInBlock {
                    step: sql_step_id(step),
                    datasource: datasource.to_string(),
                },
            ));
        }
        Ok(())
    })?;

    // Joined sql presence is syntactic: anywhere in the body, at any depth.
    let joined_sql_in_block = block_has_joined_sql(&block.steps, datasource);

    // 3. Structural reachability of the joined sql steps.
    walk_steps(&block.steps, &mut |step| {
        check_structural_step(step, datasource, joined_sql_in_block)
    })?;

    // 4. Datasource identity. A sql step with no named datasource is rejected
    // unconditionally: the block-owned transaction cannot adopt it, and a
    // mangled or bare query must never fall through to an implicit
    // datasource.
    walk_steps(&block.steps, &mut |step| match sql_step_facts_of(step) {
        Some(facts) => match facts.datasource.as_deref() {
            Some(step_datasource) if step_datasource != datasource => Err(CamelError::Transaction(
                TransactionError::ForeignDatasource {
                    step: sql_step_id(step),
                    block_datasource: datasource.to_string(),
                    step_datasource: step_datasource.to_string(),
                },
            )),
            Some(_) => Ok(()),
            None => Err(CamelError::Transaction(
                TransactionError::UnnamedDatasource {
                    step: sql_step_id(step),
                },
            )),
        },
        None => Ok(()),
    })
}

/// Structural checks for one step, using syntactic joined-sql presence.
fn check_structural_step(
    step: &DeclarativeStep,
    datasource: &str,
    joined_sql_in_block: bool,
) -> Result<(), CamelError> {
    match step {
        DeclarativeStep::Split(split)
            if split.parallel && steps_contain_joined_sql(&split.steps, datasource) =>
        {
            Err(CamelError::Transaction(TransactionError::ParallelInBlock {
                step: "split".to_string(),
                datasource: datasource.to_string(),
            }))
        }
        DeclarativeStep::Multicast(multicast)
            if steps_contain_joined_sql(&multicast.steps, datasource) =>
        {
            Err(CamelError::Transaction(
                TransactionError::MulticastInBlock {
                    step: "multicast".to_string(),
                    datasource: datasource.to_string(),
                },
            ))
        }
        DeclarativeStep::WireTap(tap) if is_joined_sql_uri(&tap.uri, datasource) => {
            Err(CamelError::Transaction(TransactionError::WireTapInBlock {
                step: tap.uri.clone(),
                datasource: datasource.to_string(),
            }))
        }
        DeclarativeStep::To(to) if joined_sql_in_block => {
            let Some((scheme, _)) = to.uri.split_once(':') else {
                return Ok(());
            };
            // `seda` has its own rule: an in-process queue handoff is not an
            // external broker send, so it is deliberately not routed through
            // `is_async_producer_scheme`.
            if scheme.eq_ignore_ascii_case("seda") {
                return Err(CamelError::Transaction(TransactionError::SedaInBlock {
                    step: to.uri.clone(),
                    datasource: datasource.to_string(),
                }));
            }
            if camel_core::route::is_async_producer_scheme(scheme) {
                return Err(CamelError::Transaction(TransactionError::AsyncInBlock {
                    step: to.uri.clone(),
                    datasource: datasource.to_string(),
                }));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn steps_contain_joined_sql(steps: &[DeclarativeStep], datasource: &str) -> bool {
    steps.iter().any(|step| {
        joined_sql_step(step, datasource)
            || child_steps(step)
                .iter()
                .any(|child| steps_contain_joined_sql(child, datasource))
    })
}

fn block_has_joined_sql(steps: &[DeclarativeStep], datasource: &str) -> bool {
    steps_contain_joined_sql(steps, datasource)
}

fn joined_sql_step(step: &DeclarativeStep, datasource: &str) -> bool {
    match sql_step_facts_of(step) {
        Some(facts) => facts.datasource.as_deref() == Some(datasource),
        None => false,
    }
}

fn is_joined_sql_uri(uri: &str, datasource: &str) -> bool {
    match sql_step_facts(uri) {
        Some(facts) => facts.datasource.as_deref() == Some(datasource),
        None => false,
    }
}

/// Facts for a step that addresses a statically authored endpoint URI.
///
/// Every variant that carries a static `uri` field is covered — `to`,
/// `enrich`, `poll_enrich` and `wire_tap` — so the StreamList, foreign
/// datasource and unnamed datasource rules see all of them. The `wire_tap`
/// URI is checked here IN ADDITION to the structural `WireTapInBlock` rule,
/// which inspects the tap side pipeline rather than the tap target.
///
/// Dynamic-URI steps (recipient list, routing slip, dynamic router) are
/// DEFERRED: their targets are computed from the exchange at run time, and
/// req 5's fail-closed dynamic-target rule lands in slice 2 (bd rc-c3mjp).
fn sql_step_facts_of(step: &DeclarativeStep) -> Option<SqlStepFacts> {
    sql_step_facts(static_step_uri(step)?)
}

/// The authored static endpoint URI a declarative step carries, if any.
///
/// Variants without a static `uri` field (dynamic-URI EIPs, leaf EIPs) yield
/// `None`.
fn static_step_uri(step: &DeclarativeStep) -> Option<&str> {
    match step {
        DeclarativeStep::To(to) => Some(&to.uri),
        DeclarativeStep::Enrich(enrich) | DeclarativeStep::PollEnrich(enrich) => Some(&enrich.uri),
        DeclarativeStep::WireTap(tap) => Some(&tap.uri),
        _ => None,
    }
}

/// Stable identity of a sql step in validation errors: its authored URI.
///
/// Only called for steps that produced [`SqlStepFacts`], i.e. steps with a
/// static URI.
fn sql_step_id(step: &DeclarativeStep) -> String {
    static_step_uri(step).unwrap_or_default().to_string()
}

/// Direct child step slices of one step, in document order.
///
/// A nested `transacted` block is not a generic child: it is validated as its
/// own unit and rejected by the nesting rule, so its body is never traversed
/// as part of the enclosing block.
fn child_steps(step: &DeclarativeStep) -> Vec<&[DeclarativeStep]> {
    match step {
        DeclarativeStep::Filter(filter) => vec![filter.steps.as_slice()],
        DeclarativeStep::Split(split) => vec![split.steps.as_slice()],
        DeclarativeStep::Multicast(multicast) => vec![multicast.steps.as_slice()],
        DeclarativeStep::Throttle(throttle) => vec![throttle.steps.as_slice()],
        DeclarativeStep::LoadBalance(load_balance) => vec![load_balance.steps.as_slice()],
        DeclarativeStep::Loop(loop_eip) => vec![loop_eip.steps.as_slice()],
        DeclarativeStep::IdempotentConsumer(idempotent) => vec![idempotent.steps.as_slice()],
        DeclarativeStep::Cache(cache) => vec![cache.on_miss.as_slice()],
        DeclarativeStep::Choice(choice) => {
            let mut children: Vec<&[DeclarativeStep]> = Vec::new();
            for when in &choice.whens {
                children.push(when.steps.as_slice());
            }
            if let Some(otherwise) = &choice.otherwise {
                children.push(otherwise.as_slice());
            }
            children
        }
        DeclarativeStep::DoTry {
            steps,
            catch,
            finally,
        } => {
            let mut children: Vec<&[DeclarativeStep]> = vec![steps.as_slice()];
            for clause in catch {
                children.push(clause.steps.as_slice());
            }
            if let Some(finally) = finally {
                children.push(finally.steps.as_slice());
            }
            children
        }
        _ => Vec::new(),
    }
}

/// Pre-order, document-order traversal of a step sequence and all descendants.
fn walk_steps<'a>(
    steps: &'a [DeclarativeStep],
    visit: &mut dyn FnMut(&'a DeclarativeStep) -> Result<(), CamelError>,
) -> Result<(), CamelError> {
    for step in steps {
        visit(step)?;
        for child in child_steps(step) {
            walk_steps(child, visit)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        DeclarativeCircuitBreaker, DeclarativeErrorHandler, DeclarativeOnException,
        MulticastAggregationDef, MulticastStepDef, SplitAggregationDef, SplitExpressionDef,
        SplitStepDef, ToStepDef, WireTapStepDef,
    };

    fn sql(uri: &str) -> DeclarativeStep {
        DeclarativeStep::To(ToStepDef::new(uri))
    }

    fn block(datasource: &str, steps: Vec<DeclarativeStep>) -> DeclarativeStep {
        DeclarativeStep::Transacted(TransactedStepDef {
            datasource: datasource.to_string(),
            steps,
        })
    }

    fn split(parallel: bool, steps: Vec<DeclarativeStep>) -> DeclarativeStep {
        DeclarativeStep::Split(SplitStepDef {
            expression: SplitExpressionDef::BodyLines,
            aggregation: SplitAggregationDef::Original,
            parallel,
            parallel_limit: None,
            trace_item_threshold: None,
            stop_on_exception: false,
            steps,
        })
    }

    fn multicast(steps: Vec<DeclarativeStep>) -> DeclarativeStep {
        DeclarativeStep::Multicast(MulticastStepDef {
            steps,
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            timeout_ms: None,
            aggregation: MulticastAggregationDef::LastWins,
        })
    }

    fn route_with_steps(steps: Vec<DeclarativeStep>) -> DeclarativeRoute {
        DeclarativeRoute {
            from: "direct:start".into(),
            route_id: "test-route".into(),
            auto_startup: true,
            startup_order: 0,
            concurrency: None,
            error_handler: None,
            circuit_breaker: None,
            security_policy: None,
            unit_of_work: None,
            steps,
            http_error_mapping: None,
        }
    }

    fn validate(steps: &[DeclarativeStep]) -> Result<(), CamelError> {
        validate_transacted_blocks(&route_with_steps(steps.to_vec()))
    }

    /// Unwraps the typed transaction error so tests can assert the exact
    /// variant instead of a rendered message.
    fn transaction_error(steps: &[DeclarativeStep]) -> TransactionError {
        match validate(steps) {
            Ok(()) => panic!("expected a typed transaction error"),
            Err(CamelError::Transaction(error)) => error,
            Err(other) => panic!("expected CamelError::Transaction, got {other:?}"),
        }
    }

    /// Same as [`transaction_error`], for route-level tests that attach a block
    /// outside `route.steps`.
    fn route_transaction_error(route: &DeclarativeRoute) -> TransactionError {
        match validate_transacted_blocks(route) {
            Ok(()) => panic!("expected a typed transaction error"),
            Err(CamelError::Transaction(error)) => error,
            Err(other) => panic!("expected CamelError::Transaction, got {other:?}"),
        }
    }

    #[test]
    fn sql_step_facts_parses_sql_uris() {
        assert_eq!(
            sql_step_facts("sql:select 1?datasource=pg"),
            Some(SqlStepFacts {
                datasource: Some("pg".to_string()),
                uses_db_url: false,
                stream_list: false,
            })
        );
        assert_eq!(
            sql_step_facts("sql:select 1?db_url=postgres://x&outputType=StreamList"),
            Some(SqlStepFacts {
                datasource: None,
                uses_db_url: true,
                stream_list: true,
            })
        );
        assert_eq!(sql_step_facts("kafka:topic"), None);
    }

    #[test]
    fn validation_passes_clean_block_regardless_of_manager_presence() {
        // Manager resolution is NOT a gate check: a structurally clean block
        // passes the gate with or without a registered manager — the
        // lowering walk's tail owns `ManagerMissing` (per-arm BEFORE
        // manager, one visitor).
        let steps = vec![block(
            "pg",
            vec![
                sql("sql:select 1?datasource=pg"),
                sql("sql:insert into t values (1)?datasource=pg"),
            ],
        )];

        assert!(
            validate(&steps).is_ok(),
            "the gate judges structure only; manager resolution happens at the walk tail"
        );
    }

    #[test]
    fn validation_rejects_nested_block() {
        let steps = vec![block(
            "pg",
            vec![block("audit", vec![sql("sql:select 1?datasource=audit")])],
        )];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::NestedBlock {
                inner: "audit".to_string(),
            }
        );
    }

    #[test]
    fn validation_rejects_stream_list_unconditionally() {
        let steps = vec![block(
            "pg",
            vec![sql("sql:select 1?datasource=pg&outputType=StreamList")],
        )];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::StreamListInBlock {
                step: "sql:select 1?datasource=pg&outputType=StreamList".to_string(),
                datasource: "pg".to_string(),
            }
        );
    }

    #[test]
    fn validation_rejects_parallel_split_over_joined_sql() {
        let steps = vec![block(
            "pg",
            vec![
                sql("sql:select 1?datasource=pg"),
                split(
                    true,
                    vec![sql("sql:insert into t values (1)?datasource=pg")],
                ),
            ],
        )];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::ParallelInBlock {
                step: "split".to_string(),
                datasource: "pg".to_string(),
            }
        );
    }

    #[test]
    fn validation_accepts_sequential_split_over_joined_sql() {
        let steps = vec![block(
            "pg",
            vec![
                sql("sql:select 1?datasource=pg"),
                split(
                    false,
                    vec![sql("sql:insert into t values (1)?datasource=pg")],
                ),
            ],
        )];

        assert!(
            validate(&steps).is_ok(),
            "a sequential split keeps the joined sql step in the block transaction"
        );
    }

    #[test]
    fn validation_rejects_multicast_over_joined_sql() {
        let steps = vec![block(
            "pg",
            vec![
                sql("sql:select 1?datasource=pg"),
                multicast(vec![sql("sql:insert into t values (1)?datasource=pg")]),
            ],
        )];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::MulticastInBlock {
                step: "multicast".to_string(),
                datasource: "pg".to_string(),
            }
        );
    }

    #[test]
    fn validation_rejects_wire_tap_over_joined_sql() {
        let steps = vec![block(
            "pg",
            vec![
                sql("sql:select 1?datasource=pg"),
                DeclarativeStep::WireTap(WireTapStepDef {
                    uri: "sql:insert into audit values (1)?datasource=pg".to_string(),
                }),
            ],
        )];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::WireTapInBlock {
                step: "sql:insert into audit values (1)?datasource=pg".to_string(),
                datasource: "pg".to_string(),
            }
        );
    }

    #[test]
    fn validation_rejects_async_producer_handoff() {
        let steps = vec![block(
            "pg",
            vec![sql("sql:select 1?datasource=pg"), sql("kafka:orders")],
        )];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::AsyncInBlock {
                step: "kafka:orders".to_string(),
                datasource: "pg".to_string(),
            }
        );
    }

    #[test]
    fn validation_rejects_seda_handoff() {
        let steps = vec![block(
            "pg",
            vec![sql("sql:select 1?datasource=pg"), sql("seda:orders")],
        )];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::SedaInBlock {
                step: "seda:orders".to_string(),
                datasource: "pg".to_string(),
            }
        );
    }

    #[test]
    fn validation_accepts_async_handoff_without_joined_sql() {
        let steps = vec![block("pg", vec![sql("kafka:orders")])];

        assert!(
            validate(&steps).is_ok(),
            "no joined sql step means the async handoff cannot escape a transaction"
        );
    }

    #[test]
    fn validation_rejects_foreign_datasource_step() {
        let steps = vec![block(
            "pg",
            vec![
                sql("sql:select 1?datasource=pg"),
                sql("sql:insert into audit values (1)?datasource=audit"),
            ],
        )];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::ForeignDatasource {
                step: "sql:insert into audit values (1)?datasource=audit".to_string(),
                block_datasource: "pg".to_string(),
                step_datasource: "audit".to_string(),
            }
        );
    }

    #[test]
    fn validation_rejects_unnamed_datasource_step() {
        let steps = vec![block(
            "pg",
            vec![sql("sql:select 1?db_url=postgres://localhost/x")],
        )];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::UnnamedDatasource {
                step: "sql:select 1?db_url=postgres://localhost/x".to_string(),
            }
        );
    }

    #[test]
    fn validation_rejects_bare_sql_step_without_datasource() {
        // Fail-closed: a sql step naming no datasource cannot join the block
        // transaction, whether or not it carries an inline `db_url`.
        let steps = vec![block("pg", vec![sql("sql:select 1")])];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::UnnamedDatasource {
                step: "sql:select 1".to_string(),
            }
        );
    }

    #[test]
    fn validation_covers_blocks_in_on_exception_clauses() {
        // Error-handler lowering keeps only `Processor` steps, so a block
        // authored in a clause would be dropped silently if the validator
        // stopped at `route.steps`.
        let mut route = route_with_steps(vec![]);
        route.error_handler = Some(DeclarativeErrorHandler {
            on_exceptions: Some(vec![DeclarativeOnException {
                code: None,
                kind: None,
                message_contains: Some("boom".to_string()),
                retry: None,
                steps: vec![block(
                    "pg",
                    vec![sql("sql:select 1?datasource=pg&outputType=StreamList")],
                )],
                handled: Some(false),
                continued: None,
                handled_by: None,
            }]),
            ..DeclarativeErrorHandler::default()
        });

        assert_eq!(
            route_transaction_error(&route),
            TransactionError::StreamListInBlock {
                step: "sql:select 1?datasource=pg&outputType=StreamList".to_string(),
                datasource: "pg".to_string(),
            }
        );
    }

    #[test]
    fn validation_covers_blocks_in_circuit_breaker_fallback() {
        let mut route = route_with_steps(vec![]);
        route.circuit_breaker = Some(DeclarativeCircuitBreaker {
            failure_threshold: 3,
            open_duration_ms: 1_000,
            fallback: vec![block("pg", vec![sql("sql:select 1?datasource=audit")])],
        });

        assert_eq!(
            route_transaction_error(&route),
            TransactionError::ForeignDatasource {
                step: "sql:select 1?datasource=audit".to_string(),
                block_datasource: "pg".to_string(),
                step_datasource: "audit".to_string(),
            }
        );
    }

    #[test]
    fn validation_covers_every_block_in_route() {
        // Two top-level blocks: the first is clean, the second violates. A walk
        // that stopped after the first block would wrongly report `Ok`.
        let steps = vec![
            block("pg", vec![sql("sql:select 1?datasource=pg")]),
            block(
                "audit",
                vec![sql("sql:select 1?datasource=audit&outputType=StreamList")],
            ),
        ];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::StreamListInBlock {
                step: "sql:select 1?datasource=audit&outputType=StreamList".to_string(),
                datasource: "audit".to_string(),
            }
        );
    }

    #[test]
    fn validation_walks_blocks_nested_in_other_eips() {
        let steps = vec![DeclarativeStep::Filter(crate::model::FilterStepDef {
            predicate: crate::model::LanguageExpressionDef {
                language: "simple".to_string(),
                source: "${body}".to_string(),
            },
            steps: vec![block(
                "pg",
                vec![split(
                    true,
                    vec![sql("sql:insert into t values (1)?datasource=pg")],
                )],
            )],
        })];

        assert_eq!(
            transaction_error(&steps),
            TransactionError::ParallelInBlock {
                step: "split".to_string(),
                datasource: "pg".to_string(),
            }
        );
    }
}
