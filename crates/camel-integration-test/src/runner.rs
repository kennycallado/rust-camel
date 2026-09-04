//! The scenario action runner (ADR-0069 §5, §7).
//!
//! Executes a scenario's ordered actions against a
//! [`PartnerRouter`](crate::adapters::PartnerRouter): `send`
//! dispatches through the adapter, `receive` awaits with the action's
//! deadline and applies `extract` into [`ScenarioVars`], `sleep` uses
//! tokio time, and `validate` evaluates the matcher grammar against
//! the last received message or an extracted variable.
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
//!   [`ScenarioFailure::ShutdownFailure`].
//!
//! Verdict-class failures map to exit 1 at the CLI; apparatus-class
//! failures map to exit 2, as do doc-validation failures before the
//! runner ever runs.
//!
//! Every await is bounded: `receive` carries the action deadline, and
//! `send` is bounded by [`SEND_DEADLINE`].

use std::collections::BTreeMap;
use std::time::Duration;

use camel_api::Value;

use crate::adapters::{
    IncomingMessage, OutgoingMessage, PartnerAdapter, PartnerRouter, ReceiveError, TransportError,
};
use crate::document::{EndpointRef, Expectation, ScenarioAction, ScenarioDocument, ScenarioTarget};

/// The bounded deadline for every `send` action (ADR-0069 §7: every
/// adapter operation carries a deadline).
const SEND_DEADLINE: Duration = Duration::from_secs(30);

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
    #[error("receive-timeout: {endpoint} delivered nothing within {deadline:?}")]
    ReceiveTimeout {
        /// The endpoint URI that delivered nothing.
        endpoint: String,
        /// The deadline that elapsed.
        deadline: Duration,
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
    /// Teardown of the boot or a partner timed out or erred after the
    /// verdict was recorded (apparatus class, `shutdown-failure`).
    #[error("shutdown-failure: {message}")]
    ShutdownFailure {
        /// Teardown failure detail.
        message: String,
    },
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
    for (index, action) in doc.scenario.iter().enumerate() {
        run_action(action, index, router, vars).await?;
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
}

/// Executes a scenario document's actions in order against the
/// router, one recorded outcome per action, stopping at the first
/// failure (the whole-document contract, library-level).
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
) -> DocumentOutcome {
    let mut per_action = Vec::with_capacity(doc.scenario.len());
    let mut failed = false;
    for (index, action) in doc.scenario.iter().enumerate() {
        if failed {
            break;
        }
        match run_action(action, index, router, vars).await {
            Ok(()) => per_action.push(Ok(ScenarioVerdict::Pass)),
            Err(failure) => {
                per_action.push(Err(failure));
                failed = true;
            }
        }
    }
    let verdict = if failed {
        None
    } else {
        Some(ScenarioVerdict::Pass)
    };
    DocumentOutcome {
        per_action,
        verdict,
        final_failure: None,
    }
}

/// Executes one action at its scenario index. The shared primitive of
/// [`run_scenario`] and [`run_scenario_document`]; every failure
/// carries the action index.
async fn run_action(
    action: &ScenarioAction,
    index: usize,
    router: &PartnerRouter,
    vars: &mut ScenarioVars,
) -> Result<(), ScenarioFailure> {
    match action {
        ScenarioAction::Send { to, body, headers } => {
            send_action(index, to, body.as_ref(), headers.as_ref(), router).await?;
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
        ScenarioAction::Validate {
            target,
            expectation,
        } => {
            validate_action(index, target, expectation, vars)?;
        }
    }
    Ok(())
}

/// Dispatches a `send` action, bounded by [`SEND_DEADLINE`].
async fn send_action(
    index: usize,
    to: &EndpointRef,
    body: Option<&Value>,
    headers: Option<&BTreeMap<String, Value>>,
    router: &PartnerRouter,
) -> Result<(), ScenarioFailure> {
    let msg = OutgoingMessage {
        body: body.cloned().unwrap_or(Value::Null),
        headers: headers.cloned().unwrap_or_default(),
    };
    let bounded = tokio::time::timeout(SEND_DEADLINE, router.send(to, msg)).await;
    let sent = bounded.map_err(|_| ScenarioFailure::ActionTransport {
        action: index,
        source: TransportError::Deadline {
            after: SEND_DEADLINE,
        },
    })?;
    sent.map_err(|source| ScenarioFailure::ActionTransport {
        action: index,
        source,
    })
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
    let message = router
        .receive(from, deadline)
        .await
        .map_err(|source| match source {
            ReceiveError::Timeout(_) => ScenarioFailure::ReceiveTimeout {
                endpoint: from.endpoint.clone(),
                deadline,
            },
            ReceiveError::Transport(source) => ScenarioFailure::ActionTransport {
                action: index,
                source,
            },
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

/// Evaluates a `validate` action against the target value in `vars`.
/// Mismatch details name the validation subject — the variable's
/// name, or the receiving endpoint — so a corrupted-header regression
/// is diagnosable from the failure text.
fn validate_action(
    index: usize,
    target: &ScenarioTarget,
    expectation: &Expectation,
    vars: &ScenarioVars,
) -> Result<(), ScenarioFailure> {
    let (value, subject) = match target {
        ScenarioTarget::LastReceived(endpoint) => (
            vars.last_received(&endpoint.endpoint)
                .map(|message| message.body.clone())
                .ok_or_else(|| ScenarioFailure::ValidationMismatch {
                    action: index,
                    detail: format!(
                        "no message has been received on {} to validate",
                        endpoint.endpoint
                    ),
                })?,
            format!("body last received on {}", endpoint.endpoint),
        ),
        ScenarioTarget::Variable(name) => (
            vars.get(name)
                .cloned()
                .ok_or_else(|| ScenarioFailure::VarUnresolved { name: name.clone() })?,
            format!("variable `{name}`"),
        ),
    };
    match expectation {
        Expectation::Equals(expected) => {
            if &value == expected {
                Ok(())
            } else {
                Err(ScenarioFailure::ValidationMismatch {
                    action: index,
                    detail: format!("{subject}: expected {expected}, got {value}"),
                })
            }
        }
        Expectation::Regex(pattern) => {
            let regex = regex::Regex::new(pattern).map_err(|error| {
                ScenarioFailure::ValidationMismatch {
                    action: index,
                    detail: format!("invalid regex `{pattern}`: {error}"),
                }
            })?;
            if regex.is_match(&stringify(&value)) {
                Ok(())
            } else {
                Err(ScenarioFailure::ValidationMismatch {
                    action: index,
                    detail: format!("{subject}: `{pattern}` did not match {value}"),
                })
            }
        }
        Expectation::Contains(needle) => check(
            index,
            stringify(&value).contains(needle),
            format!("{subject}: did not contain `{needle}`: {value}"),
        ),
        Expectation::StartsWith(prefix) => check(
            index,
            stringify(&value).starts_with(prefix),
            format!("{subject}: did not start with `{prefix}`: {value}"),
        ),
        Expectation::EndsWith(suffix) => check(
            index,
            stringify(&value).ends_with(suffix),
            format!("{subject}: did not end with `{suffix}`: {value}"),
        ),
        Expectation::Exists => {
            if value == Value::Null {
                Err(ScenarioFailure::ValidationMismatch {
                    action: index,
                    detail: format!("{subject}: expected a value, got null"),
                })
            } else {
                Ok(())
            }
        }
        Expectation::JsonSubset(pattern) => check(
            index,
            json_subset(pattern, &value),
            format!("{subject}: not a superset of {pattern}: {value}"),
        ),
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

/// Renders a value for string matchers: strings as-is, anything else
/// as its JSON form.
fn stringify(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Recursive-subset match: every key in `pattern` must exist in
/// `actual` with a recursively subset-matching value; values outside
/// `pattern` are ignored. Non-object patterns compare by equality.
fn json_subset(pattern: &Value, actual: &Value) -> bool {
    match (pattern, actual) {
        (Value::Object(pattern_object), Value::Object(actual_object)) => {
            pattern_object.iter().all(|(key, pattern_value)| {
                actual_object
                    .get(key)
                    .is_some_and(|actual_value| json_subset(pattern_value, actual_value))
            })
        }
        _ => pattern == actual,
    }
}
