//! Integration-tier test support for rust-camel (ADR-0069).
//!
//! This crate owns the scenario model: the parsing of `.test.yaml`
//! documents that declare a `scenario:` section, the ordered action
//! vocabulary (`send`, `receive`, `sleep`, `validate`), endpoint
//! references with partner provisioning, and the document-level rules
//! that keep the scenario vocabulary separate from the unit-tier
//! vocabulary (`inputs`, `expects`, `intercepts`).
//!
//! The tier derivation, the layered environment source, the action
//! runner, and the partner adapters are built on the model types
//! defined here.
//!
//! The system under test is always an embedded boot. The harness never
//! drives a deployed `camel run` process.

pub mod adapters;
pub mod boot_scenario;
pub mod document;
pub mod env_layers;
/// Inbound listener provisioning (feature `http`, rc-5yon): binds
/// `127.0.0.1:0` and stages the listener on the HTTP component's
/// global registry (ADR-0070) before the scenario boot, so the route
/// consumer consumes the staged socket.
#[cfg(feature = "http")]
pub mod inbound;
/// Process-global log capture (rc-tdgh5): the capture subscriber, the
/// open windows, and the captured events behind the scenario tier's
/// document-level `logs:` assertions. The harness claims the process's
/// tracing seat before the composition root's boot (first-wins
/// `try_init`); when it owns the seat, [`log_capture::CaptureLayer`]
/// attributes every event to every open window whose
/// `[opened_at, now)` interval contains the event timestamp.
pub mod log_capture;
pub mod runner;
pub mod sql_action;
pub mod tier;

/// Partner-script grammar of the scenario document's `partners:` map;
/// its public types are re-exported through [`crate::document`].
mod partner_script;

#[cfg(feature = "http")]
pub use adapters::http::{HttpPartner, HttpRecorder, HttpWireRequest, ScriptedResponse};
pub use adapters::{
    DirectStimulus, FakeAdapter, FakeRecorder, IncomingMessage, OutgoingMessage, PartnerAdapter,
    PartnerRouter, ReceiveError, ReceiveTimeout, RecordedSend, TransportError,
};
pub use boot_scenario::{ScenarioRun, boot_scenario};
pub use camel_matchers::RequestExpectation as PartnerExpectation;
pub use camel_matchers::{CountBound, Expectation, PathFilter, RowsExpectation};
#[cfg(feature = "http")]
pub use document::partner_scripts_for;
pub use document::{
    DocError, EndpointRef, InboundListener, PartnerFault, PartnerScript, PartnerScriptResponse,
    Provisioning, RouteSource, ScenarioAction, ScenarioDocument, ScenarioTarget, SqlTarget,
    ValidateExpectation, parse_scenario_document,
};
pub use env_layers::{AmbientLookup, LayeredEnv, ambient_std};
#[cfg(feature = "http")]
pub use inbound::provision_inbound;
pub use log_capture::{LogEvent, WindowHandle, capture_installed, ensure_capture_subscriber};
pub use runner::{
    DocumentOutcome, ScenarioFailure, ScenarioVars, ScenarioVerdict, run_scenario,
    run_scenario_document,
};
#[cfg(feature = "sql")]
pub use sql_action::execute_sql_prepare;
pub use sql_action::{
    RawSqlAction, SQL_ACTION_KEY, SQL_MEMORY_NOT_SHARED, SqlAction, ensure_sqlite_memory_shared,
    is_read_statement, sanitize_db_error, validate_sql_action,
};
pub use tier::{DocumentInputs, Tier, derive_tier};

/// Scenario document parser contract tests (the six named tests from
/// the task brief plus the validate and regex-gate tests).
#[cfg(test)]
mod doc_parse_test;

/// Layered environment tests (the three named tests from task 2.3).
#[cfg(test)]
mod env_layers_test;

/// Scenario action runner tests (the four named tests from task 2.4).
#[cfg(test)]
mod runner_test;

/// boot_scenario delegation tests (the six named tests from task 3.1,
/// scenario-shared-boot).
#[cfg(test)]
mod boot_scenario_test;

/// Partner router address-math tests (the pure wire_target /
/// lane_key_for cases).
#[cfg(test)]
mod adapters_test;

/// HTTP partner adapter tests (the named tests from task 3.1).
#[cfg(all(test, feature = "http"))]
mod http_partner_test;

/// Scenario `sql:` action executor tests (bd rc-25lup.1).
#[cfg(all(test, feature = "sql"))]
mod sql_action_test;

/// `validate` sql-target executor tests (bd rc-25lup.2, task 3.1):
/// the poll lattice, projection, mismatch redaction, and the
/// feature-off twin. Compiles in BOTH feature configurations — the
/// test bodies split internally per config.
#[cfg(test)]
mod sql_validate_test;
