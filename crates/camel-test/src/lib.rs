//! Testing utilities for rust-camel.
//!
//! This crate provides helpers for writing integration tests against the
//! rust-camel framework. It re-exports commonly needed types and provides
//! test-specific utilities.

mod harness;
mod time;

pub mod security_fixture;

pub use camel_component_mock::MockComponent;
pub use camel_matchers::{CountBound, Expectation};
pub use harness::{CamelTestContext, CamelTestContextBuilder, NoTimeControl, WithTimeControl};
pub use security_fixture::SecurityConfigFixture;
pub use time::TimeController;

#[cfg(test)]
mod tests {
    #[test]
    fn kit_reexports_shared_types() {
        let _: Option<crate::Expectation> = None;
        let _: Option<crate::CountBound> = None;
    }
}
