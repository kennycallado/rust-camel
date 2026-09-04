//! Layered hermetic environment source (ADR-0069 §4).
//!
//! Resolution order for [`LayeredEnv::lookup`]: harness-provisioned
//! bindings (the `bindVar` keys of `provisioning: harness` endpoints,
//! guaranteed absent from the document `env` map by the parser's
//! reserved-key rule), then the document `env` map, then the injected
//! ambient lookup — but only for keys listed in the passthrough
//! allowlist. Everything else resolves to `None`.
//!
//! Hermeticity contract: nothing in this crate writes the process
//! environment, and the only ambient reads go through the closure
//! injected at construction. Production callers pass [`ambient_std`];
//! tests inject map closures.

use std::collections::BTreeMap;
use std::sync::Arc;

/// Ambient lookup closure: maps a variable name to its value.
pub type AmbientLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct LayeredEnv {
    doc: BTreeMap<String, String>,
    harness_provisioned: BTreeMap<String, String>,
    passthrough: Vec<String>,
    ambient: AmbientLookup,
}

impl LayeredEnv {
    pub fn new(
        doc: BTreeMap<String, String>,
        harness_provisioned: BTreeMap<String, String>,
        passthrough: Vec<String>,
        ambient: AmbientLookup,
    ) -> Self {
        Self {
            doc,
            harness_provisioned,
            passthrough,
            ambient,
        }
    }

    /// Resolves `key` through the layer precedence: harness-provisioned
    /// bindings first, then the document `env` map, then the injected
    /// ambient lookup iff `key` is listed in the passthrough allowlist.
    pub fn lookup(&self, key: &str) -> Option<String> {
        if let Some(value) = self.harness_provisioned.get(key) {
            return Some(value.clone());
        }
        if let Some(value) = self.doc.get(key) {
            return Some(value.clone());
        }
        if self.passthrough.iter().any(|allowed| allowed == key) {
            return (self.ambient)(key);
        }
        None
    }
}

/// Wires `std::env::var` as the ambient lookup for production callers.
pub fn ambient_std() -> AmbientLookup {
    Arc::new(|key| std::env::var(key).ok())
}
