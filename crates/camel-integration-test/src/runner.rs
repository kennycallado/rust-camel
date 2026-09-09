//! The scenario action runner (ADR-0069 §5, §7).
//!
//! Executes a scenario's ordered actions against a
//! [`PartnerRouter`](crate::adapters::PartnerRouter): `send`
//! dispatches through the adapter, `receive` awaits with the action's
//! deadline and applies `extract` into [`ScenarioVars`], `sleep` uses
//! tokio time, and `validate` evaluates the matcher grammar against
//! the last received message or an extracted variable, or asserts a
//! partner's recorded-request count (feature `http`).
//!
//! Failure taxonomy (ADR-0069 §7), encoded by variant and named in
//! `Display`, never by message text alone:
//!
//! - Verdict class — the scenario ran and the system under test
//!   failed it: [`ScenarioFailure::ReceiveTimeout`],
//!   [`ScenarioFailure::ValidationMismatch`],
//!   [`ScenarioFailure::VarUnresolved`].
//! - Apparatus class — the scenario never got a meaningful answer:
//!   [`ScenarioFailure::ActionTransport`],
//!   [`ScenarioFailure::PartnerStartup`],
//!   [`ScenarioFailure::ShutdownFailure`],
//!   [`ScenarioFailure::LogCaptureUnavailable`].
//!
//! Verdict-class failures map to exit 1 at the CLI; apparatus-class
//! failures map to exit 2, as do doc-validation failures before the
//! runner ever runs.
//!
//! Every await is bounded: `receive` carries the action deadline, and
//! `send` is bounded by the document's `sendDeadline`, defaulting to
//! [`SEND_DEADLINE`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use camel_api::datasource::DatasourceCatalog;
use camel_api::{Body, Exchange, Value};
use camel_matchers::{expectation_matches, stringify};

use crate::adapters::redact_wire_path;
use crate::adapters::{
    IncomingMessage, OutgoingMessage, PartnerRouter, ReceiveError, TransportError, lanes_suffix,
};
use crate::document::{
    EndpointRef, Expectation, LogLevel, LogsAssertion, Provisioning, ScenarioAction,
    ScenarioDocument, ScenarioTarget, ValidateExpectation,
};

/// Partner verification for the `validate` action's `partner` target
/// (ADR-0069 §5): the filtered recorded-request count, the deadline
/// poll, and the mismatch-detail renderers.
mod partner_validate;

// Test-only re-exports: these primitives are exercised directly by
// `runner_test`, while the runner itself only calls
// `partner_validate_action`.
use partner_validate::partner_validate_action;
#[cfg(all(test, feature = "http"))]
pub(crate) use partner_validate::{
    matching_requests, partner_mismatch_detail, render_bound, render_filters,
};

/// SQL row-shape verification for the `validate` action's `sql`
/// target (bd rc-25lup.2): pool resolution, the fail-closed row
/// mapping, the by-name projection, the non-monotone poll lattice,
/// and the cell-free mismatch renderer.
mod sql_validate;

// The dispatch target of the runner's sql validate arm; the
// re-exports below are its direct unit tests (`sql_validate_test`,
// which compiles in BOTH feature configurations — hence the twin).
#[cfg(all(test, feature = "sql"))]
pub(crate) use sql_validate::any_row_to_tuple;
pub(crate) use sql_validate::sql_validate_action;

/// The default bounded deadline for every `send` action (ADR-0069
/// §7: every adapter operation carries a deadline). A document-level
/// `sendDeadline` overrides it (rc-tr4w).
const SEND_DEADLINE: Duration = Duration::from_secs(30);

/// The effective send bound of a document: its declared
/// `sendDeadline`, or the thirty-second [`SEND_DEADLINE`] default.
/// Real time only (ADR-0069 §6: no virtual time).
pub(crate) fn effective_send_deadline(doc: &ScenarioDocument) -> Duration {
    doc.send_deadline.unwrap_or(SEND_DEADLINE)
}

/// Mutable run state carried across actions: scenario variables set by
/// `extract`, and the last message received per endpoint for
/// `lastReceived` validation.
#[derive(Debug, Default)]
pub struct ScenarioVars {
    /// Variables extracted from received messages, by name.
    variables: BTreeMap<String, Value>,
    /// Last message received per endpoint URI.
    last_received: BTreeMap<String, IncomingMessage>,
}

impl ScenarioVars {
    /// Empty run state.
    pub fn new() -> Self {
        Self::default()
    }

    /// The variable set by an earlier `extract`, if any.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.variables.get(name)
    }

    /// Sets a variable, overwriting any earlier value.
    pub fn set(&mut self, name: impl Into<String>, value: Value) {
        self.variables.insert(name.into(), value);
    }

    /// The last message received on the endpoint URI, if any.
    pub fn last_received(&self, endpoint: &str) -> Option<&IncomingMessage> {
        self.last_received.get(endpoint)
    }

    /// Records the last message received on an endpoint URI.
    fn remember(&mut self, endpoint: String, message: IncomingMessage) {
        self.last_received.insert(endpoint, message);
    }
}

/// Resolves `${name}` placeholders in a scenario string against `vars`.
///
/// Grammar: `$${` escapes to a literal `${`; `${name}` substitutes the
/// variable when `name` matches `[A-Za-z0-9_]+` and is immediately
/// followed by `}`. Anything else — including `${env:FOO}`, where a
/// colon follows the name — stays literal, so `${env:}` never resolves
/// in scenarios. A non-string variable substitutes its JSON
/// representation (`Value::to_string`), so a number 42 yields `42`.
/// Substituted text is not re-scanned. An unset variable fails with
/// [`ScenarioFailure::VarUnresolved`].
pub(crate) fn resolve_placeholders(
    input: &str,
    vars: &ScenarioVars,
) -> Result<String, ScenarioFailure> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            // `$${` escapes to a literal `${`.
            if i + 2 < bytes.len() && bytes[i + 1] == b'$' && bytes[i + 2] == b'{' {
                out.extend_from_slice(b"${");
                i += 3;
                continue;
            }
            // `${name}` with name in [A-Za-z0-9_]+ immediately followed
            // by `}`; a colon or any other character after the name
            // keeps the whole span literal.
            if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                let name_start = i + 2;
                let mut j = name_start;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if j > name_start && j < bytes.len() && bytes[j] == b'}' {
                    let name = &input[name_start..j];
                    match vars.get(name) {
                        Some(value) => {
                            let replacement = stringify(value);
                            out.extend_from_slice(replacement.as_bytes());
                            i = j + 1;
                            continue;
                        }
                        None => {
                            return Err(ScenarioFailure::VarUnresolved {
                                name: name.to_string(),
                            });
                        }
                    }
                }
            }
            out.push(b'$');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    // The output is a byte-for-byte copy of the input except for
    // substituted spans, so it stays valid UTF-8.
    Ok(String::from_utf8(out).expect("placeholder output preserves input UTF-8")) // allow-unwrap
}

/// Recursively interpolates `${name}` placeholders in a value: maps
/// and arrays are rebuilt with interpolated values, string leaves go
/// through [`resolve_placeholders`], and every other leaf is cloned
/// untouched. An unset variable propagates
/// [`ScenarioFailure::VarUnresolved`] from any depth.
pub(crate) fn interpolate_value(
    value: &Value,
    vars: &ScenarioVars,
) -> Result<Value, ScenarioFailure> {
    match value {
        Value::String(text) => Ok(Value::String(resolve_placeholders(text, vars)?)),
        Value::Array(items) => items
            .iter()
            .map(|item| interpolate_value(item, vars))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => {
            let rebuilt = map
                .iter()
                .map(|(key, item)| Ok((key.clone(), interpolate_value(item, vars)?)))
                .collect::<Result<_, _>>()?;
            Ok(Value::Object(rebuilt))
        }
        other => Ok(other.clone()),
    }
}

/// The outcome of a scenario that ran to completion: every action
/// succeeded and every validation passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScenarioVerdict {
    /// All actions completed and all validations passed.
    Pass,
}

/// Why a scenario failed (ADR-0069 §7). Verdict-class variants mean
/// the system under test failed the scenario; apparatus-class
/// variants mean the scenario never got a meaningful answer. The
/// CLI maps verdict-class failures to exit 1 and apparatus-class
/// failures to exit 2; doc validation also maps to exit 2.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ScenarioFailure {
    /// Nothing reached the partner before the deadline (verdict
    /// class, `receive-timeout`).
    #[error("receive-timeout: {endpoint} delivered nothing within {deadline:?}{lanes}")]
    ReceiveTimeout {
        /// The endpoint URI that delivered nothing.
        endpoint: String,
        /// The deadline that elapsed.
        deadline: Duration,
        /// Rendered lane evidence (already redacted, ADR-0051): the
        /// `; no arrival matched; lanes recorded: [...]` suffix, or
        /// empty when the construction site saw no lanes.
        lanes: String,
    },
    /// A validation failed (verdict class, `validation-mismatch`).
    #[error("validation-mismatch: action {action}: {detail}")]
    ValidationMismatch {
        /// Index of the failing action, zero-based.
        action: usize,
        /// What was expected and what arrived.
        detail: String,
    },
    /// A referenced variable was never set (verdict class,
    /// `scenario-var-unresolved`).
    #[error("scenario-var-unresolved: {name}")]
    VarUnresolved {
        /// The variable name no `extract` ever set.
        name: String,
    },
    /// A send or receive failed at the transport before any assertion
    /// ran (apparatus class, `action-transport-failure`).
    #[error("action-transport-failure: action {action}: {source}")]
    ActionTransport {
        /// Index of the failing action, zero-based.
        action: usize,
        /// The transport failure.
        source: TransportError,
    },
    /// A partner listener bound but its handler failed to start
    /// (apparatus class, `partner-startup-failure`). Reserved in v1:
    /// no adapter separates bind from handler start, and the CLI maps
    /// bind failures to `partner-bind-failure` doc errors.
    #[error("partner-startup-failure: {message}")]
    PartnerStartup {
        /// Startup failure detail.
        message: String,
    },
    /// The partner's arrival lane dropped arrivals while the scenario
    /// was not receiving (apparatus class, `arrival-lane-overflow`):
    /// the harness lost them before the system under test could fail
    /// the scenario on substance.
    #[error("arrival-lane-overflow: {endpoint} dropped {dropped} arrivals")]
    ArrivalLaneOverflow {
        /// The endpoint URI whose lane dropped arrivals.
        endpoint: String,
        /// How many arrivals the lane dropped while full.
        dropped: usize,
    },
    /// Teardown of the boot or a partner timed out or erred after the
    /// verdict was recorded (apparatus class, `shutdown-failure`).
    #[error("shutdown-failure: {message}")]
    ShutdownFailure {
        /// Teardown failure detail.
        message: String,
    },
    /// The document declares a `logs:` block but the harness's capture
    /// subscriber does not own the process's tracing seat (apparatus
    /// class, `log-capture-unavailable`, rc-tdgh5): a foreign tracing
    /// subscriber won the first-wins `try_init`, so the events the
    /// block asserts against never reach the harness. The scenario
    /// never got a meaningful answer.
    #[error("log-capture-unavailable: {detail}")]
    LogCaptureUnavailable {
        /// Why capture cannot run (the foreign-subscriber condition).
        detail: String,
    },
}

/// Fills the harness bind variables into `vars` (ADR-0069 §9): every
/// wired reference with `provisioning: harness` and a `bindVar` gets
/// its partner's bound `host:port` authority from the router, so a
/// scenario string can address the partner as
/// `http://${NAME}/path`.
///
/// Two-layer split: the scenario variable carries `host:port` only;
/// the env-tier binding that route files interpolate keeps its
/// `http://host:port` form (owned by the CLI driver, unchanged here).
/// A reference with no registered adapter or no bound authority is
/// skipped: the variable stays unset, and a later use fails with the
/// verdict-class `VarUnresolved`.
pub fn fill_bind_vars(wired: &[EndpointRef], router: &PartnerRouter, vars: &mut ScenarioVars) {
    for reference in wired {
        if reference.provisioning != Some(Provisioning::Harness) {
            continue;
        }
        let Some(bind_var) = reference.bind_var.as_deref() else {
            continue;
        };
        let Some(authority) = router
            .adapter(&reference.endpoint)
            .and_then(|adapter| adapter.bound_authority())
        else {
            continue;
        };
        vars.set(bind_var, Value::String(authority));
    }
}

/// Runs a scenario's actions in order against the router.
///
/// On success every action completed; on failure the variant names
/// the ADR-0069 §7 class. `vars` carries extraction results and
/// last-received state both into and out of the run.
pub async fn run_scenario(
    doc: &ScenarioDocument,
    router: &PartnerRouter,
    vars: &mut ScenarioVars,
) -> Result<ScenarioVerdict, ScenarioFailure> {
    // The scenario-start anchor every `elapsedAtLeast` bound measures
    // against; taken once per run, before the first action.
    let started_at = Instant::now();
    let send_deadline = effective_send_deadline(doc);
    for (index, action) in doc.scenario.iter().enumerate() {
        // The single-action loop has no boot of its own, so a `sql:`
        // action here has no datasource catalog and fails closed.
        run_action(action, index, router, vars, started_at, send_deadline, None).await?;
    }
    Ok(ScenarioVerdict::Pass)
}

/// The outcome of executing a whole scenario document
/// (ADR-0069 sections 5 and 7).
///
/// [`run_scenario_document`](self::run_scenario_document) fills
/// `per_action` with one outcome per executed action and stops at the
/// first failure; `verdict` is `Some(Pass)` only when every action
/// passed. `final_failure` is the post-verdict slot: the caller that
/// owns the boot (the CLI, after `BootHandle::shutdown`) records a
/// `ShutdownFailure` there without masking the recorded verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentOutcome {
    /// One outcome per executed action, in action order; actions after
    /// the first failure never ran.
    pub per_action: Vec<Result<ScenarioVerdict, ScenarioFailure>>,
    /// `Some(Pass)` when every action completed; `None` after any
    /// failure.
    pub verdict: Option<ScenarioVerdict>,
    /// Post-verdict shutdown failure, recorded by the boot-owning
    /// caller; empty when teardown is clean or never ran.
    pub final_failure: Option<ScenarioFailure>,
    /// The bound address of the document's `inbound:` listener
    /// (rc-5yon, ADR-0070), filled by the boot-owning caller from
    /// [`crate::boot_scenario::ScenarioRun::inbound_bound`] after the
    /// boot, so tests target the ephemeral listener without re-deriving
    /// it. `None` when the document declares no `inbound:` listener or
    /// the caller never filled it; the post-boot slot, as
    /// `final_failure` is the post-verdict slot.
    pub inbound_bound: Option<std::net::SocketAddr>,
    /// Document-level `logs:` block violation (rc-tdgh5): a rendered
    /// diagnostic naming each violated clause — for `noLevelAbove`,
    /// each offending event's level, target, and message. `Some` only
    /// when every action passed and the logs evaluation then failed,
    /// so `verdict` is `None` alongside it. `None` when the document
    /// declares no `logs:` block, the block passed, an action failed
    /// first (the block never evaluated), or capture was unavailable
    /// (the apparatus failure lives in `per_action`).
    pub logs_failure: Option<String>,
}

/// Executes a scenario document's actions in order against the
/// router, one recorded outcome per action, stopping at the first
/// failure (the whole-document contract, library-level).
///
/// `datasource_catalog` is the booted cascade's single datasource
/// catalog (bd rc-25lup.1): a `sql:` action resolves its pool through
/// it, so the seeds land in the same pools the routes use. Callers
/// without a boot pass `None`; a `sql:` action then fails closed
/// instead of silently seeding nothing.
///
/// When the document declares a `logs:` block (rc-tdgh5), a capture
/// window opens at document start (behind the harness's process-seat
/// ownership — a foreign subscriber fails the document through
/// [`ScenarioFailure::LogCaptureUnavailable`] first) and the block
/// evaluates after the action loop against the window's events: a
/// violation fills [`DocumentOutcome::logs_failure`] with the verdict
/// `None`.
///
/// Partners route through `router`; a `send` addressed to a context
/// component reaches the booted system under test through the
/// context-stimulus adapter the caller registered for that endpoint
/// (see [`crate::adapters`]). The single-action
/// [`run_scenario`] loop and this loop share [`run_action`].
pub async fn run_scenario_document(
    doc: &ScenarioDocument,
    router: &PartnerRouter,
    vars: &mut ScenarioVars,
    datasource_catalog: Option<&Arc<dyn DatasourceCatalog>>,
) -> DocumentOutcome {
    // Log-capture window (rc-tdgh5): open at document start when the
    // document declares a `logs:` block — and only when the harness's
    // capture subscriber owns the process's tracing seat. A foreign
    // subscriber won the first-wins race: the document fails through
    // the apparatus class before any action runs, because the events
    // the block asserts against would never reach the harness.
    let capture_window = match &doc.logs {
        None => None,
        Some(_) if crate::log_capture::capture_installed() => {
            Some(crate::log_capture::open_window())
        }
        Some(_) => {
            return DocumentOutcome {
                per_action: vec![Err(ScenarioFailure::LogCaptureUnavailable {
                    detail: "the `logs:` block needs the harness log-capture subscriber, but a foreign tracing subscriber owns this process (first-wins try_init); install nothing before the scenario harness".to_string(),
                })],
                verdict: None,
                final_failure: None,
                logs_failure: None,
                inbound_bound: None,
            };
        }
    };
    // The scenario-start anchor every `elapsedAtLeast` bound measures
    // against; taken once per run, before the first action.
    let started_at = Instant::now();
    let send_deadline = effective_send_deadline(doc);
    let mut per_action = Vec::with_capacity(doc.scenario.len());
    let mut failed = false;
    for (index, action) in doc.scenario.iter().enumerate() {
        if failed {
            break;
        }
        match run_action(
            action,
            index,
            router,
            vars,
            started_at,
            send_deadline,
            datasource_catalog,
        )
        .await
        {
            Ok(()) => per_action.push(Ok(ScenarioVerdict::Pass)),
            Err(failure) => {
                per_action.push(Err(failure));
                failed = true;
            }
        }
    }
    // Logs evaluation (rc-tdgh5): only when no action failed, against
    // the window that spanned the run. Closing unregisters the window
    // (conservative attribution for every later window); an action
    // failure skips evaluation and drops the handle, which unregisters
    // the window the same way.
    let logs_failure = match (&doc.logs, capture_window) {
        (Some(assertion), Some(window)) if !failed => {
            let events = window.close();
            evaluate_logs(assertion, &events)
        }
        _ => None,
    };
    let verdict = if failed || logs_failure.is_some() {
        None
    } else {
        Some(ScenarioVerdict::Pass)
    };
    DocumentOutcome {
        per_action,
        verdict,
        final_failure: None,
        logs_failure,
        inbound_bound: None,
    }
}

/// Evaluates the document-level `logs:` block against the closed
/// window's events (rc-tdgh5). Conjunction across clauses: every
/// `contains` marker must appear in at least one event message, every
/// `regex` entry must match at least one (unanchored), and no event
/// may carry a level above `noLevelAbove`. `Some(diagnostic)` names
/// every violated clause; for `noLevelAbove` it lists each offending
/// event's level, target, and message.
fn evaluate_logs(
    assertion: &LogsAssertion,
    events: &[crate::log_capture::LogEvent],
) -> Option<String> {
    let mut violations: Vec<String> = Vec::new();
    for marker in &assertion.contains {
        if !events
            .iter()
            .any(|event| event.message.contains(marker.as_str()))
        {
            violations.push(format!(
                "`logs.contains` entry `{marker}` matched no captured event"
            ));
        }
    }
    for pattern in &assertion.regex {
        // The load-time gate compiled every pattern; a compile failure
        // here is unreachable, reported rather than panicked (defense
        // in depth).
        match regex::Regex::new(pattern) {
            Ok(compiled) => {
                if !events.iter().any(|event| compiled.is_match(&event.message)) {
                    violations.push(format!(
                        "`logs.regex` entry `{pattern}` matched no captured event"
                    ));
                }
            }
            Err(error) => violations.push(format!(
                "`logs.regex` entry `{pattern}` does not compile: {error}"
            )),
        }
    }
    if let Some(cap) = assertion.no_level_above {
        let offenders: Vec<&crate::log_capture::LogEvent> = events
            .iter()
            .filter(|event| event.level < as_tracing_level(cap))
            .collect();
        if !offenders.is_empty() {
            let listed = offenders
                .iter()
                .map(|event| format!("{} {} {}", event.level, event.target, event.message))
                .collect::<Vec<_>>()
                .join("; ");
            violations.push(format!(
                "`logs.noLevelAbove` violated by {} event(s): {listed}",
                offenders.len()
            ));
        }
    }
    if violations.is_empty() {
        None
    } else {
        Some(violations.join("; "))
    }
}

/// Maps the document grammar's level onto `tracing`'s ordering.
/// `tracing` orders levels by verbosity — `TRACE` is the greatest,
/// `ERROR` the least — so an event more SEVERE than the cap compares
/// LESS than the cap's level (`event.level < cap`).
fn as_tracing_level(level: LogLevel) -> tracing::Level {
    match level {
        LogLevel::Trace => tracing::Level::TRACE,
        LogLevel::Debug => tracing::Level::DEBUG,
        LogLevel::Info => tracing::Level::INFO,
        LogLevel::Warn => tracing::Level::WARN,
        LogLevel::Error => tracing::Level::ERROR,
    }
}

/// Executes one action at its scenario index. The shared primitive of
/// [`run_scenario`] and [`run_scenario_document`]; every failure
/// carries the action index. A `sql:` action seeds through
/// `datasource_catalog` (`None` fails closed — see
/// [`run_scenario_document`]).
async fn run_action(
    action: &ScenarioAction,
    index: usize,
    router: &PartnerRouter,
    vars: &mut ScenarioVars,
    started_at: Instant,
    send_deadline: Duration,
    datasource_catalog: Option<&Arc<dyn DatasourceCatalog>>,
) -> Result<(), ScenarioFailure> {
    match action {
        ScenarioAction::Send {
            to,
            body,
            headers,
            method,
            expect_reply,
        } => {
            send_action(
                index,
                to,
                body.as_ref(),
                headers.as_ref(),
                method,
                expect_reply.as_ref(),
                router,
                vars,
                send_deadline,
            )
            .await?;
        }
        ScenarioAction::Receive {
            from,
            deadline,
            extract,
        } => {
            receive_action(index, from, *deadline, extract.as_ref(), router, vars).await?;
        }
        ScenarioAction::Sleep { duration } => {
            tokio::time::sleep(*duration).await;
        }
        ScenarioAction::Validate { .. } => {
            validate_action(action, index, started_at, router, vars, datasource_catalog).await?;
        }
        ScenarioAction::Sql {
            datasource,
            prepare,
        } => {
            // Apparatus class (exit 2): seeding is harness-side state
            // preparation — a failure here means the scenario never
            // got its declared preconditions, never that the system
            // under test misbehaved.
            #[cfg(feature = "sql")]
            {
                let Some(catalog) = datasource_catalog else {
                    return Err(ScenarioFailure::ActionTransport {
                        action: index,
                        source: TransportError::Other {
                            message: "sql action: no datasource catalog is available; the \
                                      boot-owning caller must pass the cascade's catalog"
                                .to_string(),
                        },
                    });
                };
                let sql = crate::sql_action::SqlAction {
                    datasource: datasource.clone(),
                    prepare: prepare.clone(),
                };
                crate::sql_action::execute_sql_prepare(catalog, &sql)
                    .await
                    .map_err(|message| ScenarioFailure::ActionTransport {
                        action: index,
                        source: TransportError::Other { message },
                    })?;
            }
            // Defense-in-depth: the document parser rejects `sql:`
            // without the feature, so only a directly-constructed
            // document reaches this arm (the boot-level inbound
            // precedent).
            #[cfg(not(feature = "sql"))]
            {
                let _ = (datasource, prepare, datasource_catalog);
                return Err(ScenarioFailure::ActionTransport {
                    action: index,
                    source: TransportError::Other {
                        message: "the `sql:` action requires the `sql` feature, which this \
                                  harness build does not enable: rebuild with \
                                  `--features sql` (demand-gated activation)"
                            .to_string(),
                    },
                });
            }
        }
    }
    Ok(())
}

/// Dispatches a `send` action, bounded by the document's effective
/// send deadline ([`effective_send_deadline`]: the declared
/// `sendDeadline`, or the thirty-second default).
///
/// The endpoint reference, the body's string leaves, and the header
/// values are the complete interpolation surface: each resolves its
/// `${name}` placeholders against `vars` before dispatch, and an
/// unresolved variable fails with the verdict-class `VarUnresolved`.
/// The dial target comes from the router's address math: a
/// harness-declared `:0` reference (or a dynamic reference resolving
/// to a partner authority) dials the partner's bound address with the
/// interpolated path preserved; anything else dials the interpolated
/// URI literally.
///
/// A declared `expectReply` (rc-qvz6, `direct:` sends only — the
/// grammar rejected every other scheme at load) asserts the
/// synchronous reply the adapter returned: a non-matching reply is a
/// verdict-class [`ScenarioFailure::ValidationMismatch`] naming the
/// rendered expectation and the actual body, and a missing reply is
/// an apparatus-class [`ScenarioFailure::ActionTransport`] — the
/// scenario never got an answer to assert against.
// The action's flat decomposition (index, endpoint, body, headers,
// method, reply expectation, router, vars) plus the document send
// bound threaded from run_action (rc-tr4w).
#[allow(clippy::too_many_arguments)]
async fn send_action(
    index: usize,
    to: &EndpointRef,
    body: Option<&Value>,
    headers: Option<&BTreeMap<String, Value>>,
    method: &str,
    expect_reply: Option<&Expectation>,
    router: &PartnerRouter,
    vars: &ScenarioVars,
    send_deadline: Duration,
) -> Result<(), ScenarioFailure> {
    let declared = to.endpoint.as_str();
    let interpolated = resolve_placeholders(declared, vars)?;
    let body = body
        .map(|value| interpolate_value(value, vars))
        .transpose()?;
    let headers = headers
        .map(|map| -> Result<BTreeMap<String, Value>, ScenarioFailure> {
            map.iter()
                .map(|(name, value)| Ok((name.clone(), interpolate_value(value, vars)?)))
                .collect()
        })
        .transpose()?;
    let msg = OutgoingMessage {
        body: body.unwrap_or(Value::Null),
        headers: headers.unwrap_or_default(),
        method: method.to_string(),
    };
    let bounded =
        tokio::time::timeout(send_deadline, router.send(declared, &interpolated, msg)).await;
    let sent = bounded.map_err(|_| ScenarioFailure::ActionTransport {
        action: index,
        source: TransportError::Deadline {
            after: send_deadline,
        },
    })?;
    let reply = sent.map_err(|source| {
        // Render-site defense: the http lane pre-renders its
        // composite "key path" overflow form with each half
        // redacted, but a third-party adapter may hand the overflow
        // over RAW; the runner holds the secret set. Only the exact
        // pre-rendered shape (one space, path half leading `/`)
        // redacts per half — one pass over that shape would merge
        // two query-bearing halves into a single pair and swallow
        // the path half. Any other string, including a raw
        // third-party key, redacts as ONE value: splitting an
        // arbitrary string could sever a secret value across halves
        // and print its tail (fail-safe, idempotent, ADR-0051).
        let source = match source {
            TransportError::LaneFifoOverflow { lane_key, bound } => {
                let rendered = match lane_key.split_once(' ') {
                    Some((key_half, path_half))
                        if !key_half.contains(' ') && path_half.starts_with('/') =>
                    {
                        format!(
                            "{} {}",
                            redact_wire_path(key_half, &router.secret_query_keys()),
                            redact_wire_path(path_half, &router.secret_query_keys())
                        )
                    }
                    _ => redact_wire_path(&lane_key, &router.secret_query_keys()),
                };
                TransportError::LaneFifoOverflow {
                    lane_key: rendered,
                    bound,
                }
            }
            other => other,
        };
        ScenarioFailure::ActionTransport {
            action: index,
            source,
        }
    })?;
    if let Some(expectation) = expect_reply {
        let Some(reply) = reply else {
            // Fail closed: the grammar promised a direct reply, but
            // the adapter produced none — an apparatus defect, never
            // a silently-skipped assertion.
            return Err(ScenarioFailure::ActionTransport {
                action: index,
                source: TransportError::Other {
                    message: "direct send produced no reply".to_string(),
                },
            });
        };
        let value = reply_body_value(&reply);
        if !expectation_matches(expectation, &value) {
            return Err(ScenarioFailure::ValidationMismatch {
                action: index,
                detail: format!(
                    "direct reply on {}: expected {}, got {}",
                    to.endpoint,
                    render_expectation(expectation),
                    stringify(&value)
                ),
            });
        }
    }
    Ok(())
}

/// Converts a synchronous `direct:` reply exchange's body into the
/// matcher value an `expectReply` assertion reads (rc-qvz6): the
/// reply message is the exchange's output when the route produced
/// one, the (route-mutated — `set_body` writes it) input otherwise.
/// Feature-free by design: the partner-body extractors stay
/// `http`-gated; this path never touches the wire. Crate-visible for
/// the runner's unit tests, like the interpolation primitives.
pub(crate) fn reply_body_value(exchange: &Exchange) -> Value {
    let message = exchange.output.as_ref().unwrap_or(&exchange.input);
    match &message.body {
        Body::Json(value) => value.clone(),
        Body::Text(text) => reply_bytes_value(text.as_bytes()),
        Body::Xml(text) => reply_bytes_value(text.as_bytes()),
        Body::Bytes(bytes) => reply_bytes_value(bytes),
        // Empty and consumed-stream bodies carry no reply bytes, and
        // foreign `#[non_exhaustive]` body kinds (none today) expose
        // none either; the value reads as the empty string.
        _ => Value::String(String::new()),
    }
}

/// Parses reply bytes as JSON, falling back to a lossy-UTF-8 string
/// when they are not JSON text: a text body holding JSON is observed
/// as the structured value the matcher verbs expect, and any other
/// text stays textual. Shared with the sql validate executor, whose
/// blob cells obey the same law.
pub(crate) fn reply_bytes_value(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(bytes).into_owned()))
}

/// Renders an expectation for an `expectReply` mismatch detail: the
/// verb and its payload in the document grammar's own terms.
fn render_expectation(expectation: &Expectation) -> String {
    match expectation {
        Expectation::Equals(expected) => format!("equals {expected}"),
        Expectation::Regex(pattern) => format!("matches regex `{pattern}`"),
        Expectation::Contains(needle) => format!("contains `{needle}`"),
        Expectation::StartsWith(prefix) => format!("startsWith `{prefix}`"),
        Expectation::EndsWith(suffix) => format!("endsWith `{suffix}`"),
        Expectation::Exists => "exists".to_string(),
        Expectation::JsonSubset(pattern) => format!("is a superset of {pattern}"),
        // Foreign `#[non_exhaustive]` variants (none today): no verb
        // renders, but the matcher already failed closed.
        _ => "the expected value".to_string(),
    }
}

/// Awaits a `receive` action until the deadline, records the message,
/// and applies `extract` into `vars`.
async fn receive_action(
    index: usize,
    from: &EndpointRef,
    deadline: Duration,
    extract: Option<&BTreeMap<String, String>>,
    router: &PartnerRouter,
    vars: &mut ScenarioVars,
) -> Result<(), ScenarioFailure> {
    // The lane is read under the two-key contract: the declared
    // string names the registered lane when it can, and the
    // interpolated URI resolves a dynamic reference's lane by
    // authority (`lane_key_for`).
    let declared = from.endpoint.as_str();
    let interpolated = resolve_placeholders(declared, vars)?;
    let message = router
        .receive(declared, &interpolated, deadline)
        .await
        .map_err(|source| {
            // Render-site defense: a third-party adapter may hand
            // over RAW endpoint and lane evidence; the runner holds
            // the secret set, and redaction is idempotent on
            // already-masked output (ADR-0051).
            let keys = router.secret_query_keys();
            match source {
                ReceiveError::Timeout(timeout) => ScenarioFailure::ReceiveTimeout {
                    endpoint: redact_wire_path(&timeout.endpoint, &keys),
                    deadline,
                    lanes: lanes_suffix(
                        &timeout
                            .lanes_recorded
                            .iter()
                            .map(|lane| redact_wire_path(lane, &keys))
                            .collect::<Vec<_>>(),
                    ),
                },
                ReceiveError::Overflow(overflow) => ScenarioFailure::ArrivalLaneOverflow {
                    endpoint: redact_wire_path(&overflow.endpoint, &keys),
                    dropped: overflow.dropped,
                },
                ReceiveError::Transport(source) => ScenarioFailure::ActionTransport {
                    action: index,
                    source,
                },
            }
        })?;
    if let Some(extract) = extract {
        for (name, selector) in extract {
            let value = select_from(&message, selector).ok_or_else(|| {
                ScenarioFailure::ValidationMismatch {
                    action: index,
                    detail: format!(
                        "extract of `{selector}` into variable `{name}` resolved to nothing"
                    ),
                }
            })?;
            vars.set(name.clone(), value);
        }
    }
    vars.remember(from.endpoint.clone(), message);
    Ok(())
}

/// Evaluates a `validate` action (ADR-0069 §5).
///
/// The `partner` target asserts the exact filtered count of the
/// requests the harness partner recorded, read as the router's
/// snapshot — one immediate read without a deadline, a polled one
/// with it ([`partner_validate_action`]). The `sql` target asserts
/// the doc-authored read's row shape against the named datasource's
/// pool through [`sql_validate_action`], which owns the deadline
/// poll (SQL state is non-monotone, so its lattice differs from the
/// partner's). Every other target applies the message grammar
/// against `vars`; the deadline is partner/sql-only (the grammar
/// rejected it on these targets at parse time, so the message arm
/// ignores it). An `elapsedAtLeast` bound on a `lastReceived` target
/// checks the message's wire arrival against the scenario-start
/// anchor before the grammar runs; the grammar rejected it on every
/// other target at parse time. Mismatch details name the validation
/// subject — the variable's name, the receiving endpoint, or the
/// partner URI — so a corrupted-header regression is diagnosable
/// from the failure text.
async fn validate_action(
    action: &ScenarioAction,
    index: usize,
    started_at: Instant,
    router: &PartnerRouter,
    vars: &ScenarioVars,
    datasource_catalog: Option<&Arc<dyn DatasourceCatalog>>,
) -> Result<(), ScenarioFailure> {
    // run_action dispatches only the Validate variant here; the
    // fallback mirrors the impossible pairing arms below.
    let ScenarioAction::Validate {
        target,
        expectation,
        deadline,
        elapsed_at_least,
    } = action
    else {
        return Err(unpaired_validate(index));
    };
    match (target, expectation) {
        // The parser pairs a `partner` target with the partner count
        // grammar; this arm reads the router's snapshot and owns the
        // deadline.
        (ScenarioTarget::Partner(endpoint), ValidateExpectation::Partner(expected)) => {
            partner_validate_action(index, &endpoint.endpoint, expected, *deadline, router).await
        }
        // The parser pairs a `sql` target with the row-shape grammar;
        // this arm reads the datasource's live state and owns the
        // deadline poll. A `None` catalog fails closed inside (the
        // sql-action precedent), so a scenario never silently skips
        // its assertion.
        (ScenarioTarget::Sql(target), ValidateExpectation::Rows(expected)) => {
            sql_validate_action(index, target, expected, *deadline, datasource_catalog).await
        }
        (_, ValidateExpectation::Message(expectation)) => {
            let (value, subject) = match target {
                ScenarioTarget::LastReceived(endpoint) => {
                    // The declared endpoint may carry query bytes: the
                    // subject renders redacted like every diagnostic
                    // that quotes a wire path (ADR-0051).
                    let redacted =
                        redact_wire_path(&endpoint.endpoint, &router.secret_query_keys());
                    let message = vars.last_received(&endpoint.endpoint).ok_or_else(|| {
                        ScenarioFailure::ValidationMismatch {
                            action: index,
                            detail: format!(
                                "no message has been received on {redacted} to validate"
                            ),
                        }
                    })?;
                    // The elapsed bound anchors to the message's wire
                    // arrival, never the consumption time: a message
                    // consumed late can still have arrived early (the
                    // wire is the proof, ADR-0069 §5).
                    if let Some(bound) = elapsed_at_least {
                        let actual = message
                            .arrival
                            .checked_duration_since(started_at)
                            .unwrap_or_default();
                        if actual < *bound {
                            return Err(ScenarioFailure::ValidationMismatch {
                                action: index,
                                detail: format!(
                                    "{redacted}: arrived {} after the scenario started; `elapsedAtLeast` requires {}",
                                    humantime::format_duration(actual),
                                    humantime::format_duration(*bound)
                                ),
                            });
                        }
                    }
                    (
                        message.body.clone(),
                        format!("body last received on {redacted}"),
                    )
                }
                ScenarioTarget::Variable(name) => (
                    vars.get(name)
                        .cloned()
                        .ok_or_else(|| ScenarioFailure::VarUnresolved { name: name.clone() })?,
                    format!("variable `{name}`"),
                ),
                // Taken by the arm above: the grammar never pairs a
                // `partner` target with the message expectation.
                ScenarioTarget::Partner(_) => return Err(unpaired_validate(index)),
                // Taken by the arm above: the grammar pairs a `sql`
                // target with the rows grammar only; a message
                // expectation here means a caller bypassed the
                // parser.
                ScenarioTarget::Sql(_) => return Err(unpaired_validate(index)),
            };
            // The per-form booleans delegate to the shared core
            // (`camel_matchers::expectation_matches`); the detail
            // strings stay here, where subject rendering and
            // redaction live.
            match expectation {
                Expectation::Equals(expected) => check(
                    index,
                    expectation_matches(expectation, &value),
                    format!("{subject}: expected {expected}, got {value}"),
                ),
                // The parser pre-verifies regex patterns at load time,
                // so the invalid-regex arm is unreachable through the
                // harness; it stays for the byte-identical verdicts,
                // short-circuiting before the core delegation (core's
                // Regex arm returns false on compile-fail).
                Expectation::Regex(pattern) => {
                    if let Err(error) = regex::Regex::new(pattern) {
                        return Err(ScenarioFailure::ValidationMismatch {
                            action: index,
                            detail: format!("invalid regex `{pattern}`: {error}"),
                        });
                    }
                    check(
                        index,
                        expectation_matches(expectation, &value),
                        format!("{subject}: `{pattern}` did not match {value}"),
                    )
                }
                Expectation::Contains(needle) => check(
                    index,
                    expectation_matches(expectation, &value),
                    format!("{subject}: did not contain `{needle}`: {value}"),
                ),
                Expectation::StartsWith(prefix) => check(
                    index,
                    expectation_matches(expectation, &value),
                    format!("{subject}: did not start with `{prefix}`: {value}"),
                ),
                Expectation::EndsWith(suffix) => check(
                    index,
                    expectation_matches(expectation, &value),
                    format!("{subject}: did not end with `{suffix}`: {value}"),
                ),
                Expectation::Exists => check(
                    index,
                    expectation_matches(expectation, &value),
                    format!("{subject}: expected a value, got null"),
                ),
                Expectation::JsonSubset(pattern) => check(
                    index,
                    expectation_matches(expectation, &value),
                    format!("{subject}: not a superset of {pattern}: {value}"),
                ),
                // Foreign `#[non_exhaustive]` variants (none today):
                // the harness has no matcher for them, so they fail
                // closed.
                _ => Err(ScenarioFailure::ValidationMismatch {
                    action: index,
                    detail: "validate expectation kind is not supported by the message grammar"
                        .to_string(),
                }),
            }
        }
        // The parser never pairs a partner or sql target with another
        // kind's grammar, and never a rows expectation with a
        // non-sql target: a `partner` target pairs with the partner
        // count grammar, a `sql` target with the rows grammar
        // (`rows` or a count bound), every other target with the
        // message grammar.
        _ => Err(unpaired_validate(index)),
    }
}

/// The failure for a target/expectation pairing the grammar never
/// produces: the parser pairs `partner` targets with the partner
/// count grammar, `sql` targets with the rows grammar, and every
/// other target with the message grammar, so only a caller bypassing
/// the parser reaches these arms.
fn unpaired_validate(index: usize) -> ScenarioFailure {
    ScenarioFailure::ValidationMismatch {
        action: index,
        detail: "validate target kind does not pair with the expectation kind: `partner` pairs \
                 with the partner count grammar, `sql` with the rows grammar, and every other \
                 target with the message grammar"
            .to_string(),
    }
}

/// Turns a validation predicate into a [`ScenarioFailure`] on `false`.
fn check(index: usize, passed: bool, detail: String) -> Result<(), ScenarioFailure> {
    if passed {
        Ok(())
    } else {
        Err(ScenarioFailure::ValidationMismatch {
            action: index,
            detail,
        })
    }
}

/// Reads a value out of a received message by dotted selector.
///
/// Grammar: the first segment selects `body`, `headers`, `status`,
/// `method`, or `path`; the rest is a literal header name
/// (`headers.X-Id`, dots allowed in the name) or a dotted object path
/// under the body (`body.user.id`). A bare `body` or `headers` selects
/// the whole part.
///
/// Header lookup is ASCII-case-insensitive: adapters differ in header
/// casing (hyper lowercases wire names; the fake preserves author
/// casing), and the same selector must behave identically per adapter.
/// Wire recording stays lowercase.
fn select_from(message: &IncomingMessage, selector: &str) -> Option<Value> {
    let (head, rest) = match selector.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (selector, None),
    };
    match head {
        "body" => match rest {
            None => Some(message.body.clone()),
            Some(path) => walk_path(&message.body, path),
        },
        "headers" => match rest {
            None => Some(Value::Object(
                message
                    .headers
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            )),
            Some(name) => lookup_header(&message.headers, name),
        },
        // The transport-scalar heads carry no sub-path: `status.why` is
        // not part of the grammar and resolves to nothing.
        "status" if rest.is_none() => Some(
            message
                .status
                .map_or(Value::Null, |code| Value::Number(code.into())),
        ),
        "method" if rest.is_none() => {
            Some(message.method.clone().map_or(Value::Null, Value::String))
        }
        "path" if rest.is_none() => Some(message.path.clone().map_or(Value::Null, Value::String)),
        _ => None,
    }
}

/// Case-insensitive header lookup: the first header whose name matches
/// the selector ASCII-case-insensitively wins; header maps are
/// case-unique per adapter, so the fold is deterministic.
fn lookup_header(headers: &BTreeMap<String, Value>, name: &str) -> Option<Value> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// Walks a dotted object path under a body value; arrays and scalars
/// resolve to nothing.
fn walk_path(value: &Value, path: &str) -> Option<Value> {
    let mut current = value;
    for key in path.split('.') {
        current = current.as_object()?.get(key)?;
    }
    Some(current.clone())
}
