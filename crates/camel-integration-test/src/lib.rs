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
pub mod runner;
pub mod tier;

#[cfg(feature = "http")]
pub use adapters::http::{HttpPartner, HttpRecorder, HttpWireRequest, ScriptedResponse};
pub use adapters::{
    DirectStimulus, FakeAdapter, FakeRecorder, IncomingMessage, OutgoingMessage, PartnerAdapter,
    PartnerRouter, ReceiveError, ReceiveTimeout, RecordedSend, TransportError,
};
pub use boot_scenario::{ScenarioRun, boot_scenario};
#[cfg(feature = "http")]
pub use document::partner_scripts_for;
pub use document::{
    DocError, EndpointRef, Expectation, PartnerScript, PartnerScriptResponse, Provisioning,
    RouteSource, ScenarioAction, ScenarioDocument, ScenarioTarget, parse_scenario_document,
};
pub use env_layers::{AmbientLookup, LayeredEnv, ambient_std};
pub use runner::{
    DocumentOutcome, ScenarioFailure, ScenarioVars, ScenarioVerdict, run_scenario,
    run_scenario_document,
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

/// Partner router address-math tests (the pure wire_target /
/// lane_key_for cases).
#[cfg(test)]
mod adapters_test;

/// HTTP partner adapter tests (the named tests from task 3.1).
#[cfg(all(test, feature = "http"))]
mod http_partner_test;
