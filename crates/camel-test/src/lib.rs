//! Testing utilities for rust-camel.
//!
//! This crate provides helpers for writing integration tests against the
//! rust-camel framework. It re-exports commonly needed types and provides
//! test-specific utilities.

mod harness;
mod time;

pub mod security_fixture;

pub use camel_component_mock::MockComponent;
pub use harness::{CamelTestContext, CamelTestContextBuilder, NoTimeControl, WithTimeControl};
pub use security_fixture::SecurityConfigFixture;
pub use time::TimeController;
