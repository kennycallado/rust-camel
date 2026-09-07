//! Hermeticity witness for R-SCHEMA env interpolation (rc-93wct).
//!
//! Lives OUTSIDE `src/` on purpose: the crate purity gate forbids any
//! `std::env` use under `src/`, and the witness must poison the AMBIENT
//! process environment to prove the lint engine never reads it. Runs the
//! real `LintEngine` (all default rules) and asserts R-SCHEMA diagnostics
//! are a pure function of document text: the validated value derives from
//! the literal `:-default`, never from the environment.

use std::sync::Arc;

use camel_api::component_metadata::{ComponentMetadata, ComponentMetadataCatalog};
use camel_lint::{DiagnosticCode, LintEngine, Severity};

/// Catalog with no registered components — R-SCHEMA does not consult it.
struct EmptyCatalog;

impl ComponentMetadataCatalog for EmptyCatalog {
    fn get_metadata(&self, _scheme: &str) -> Option<ComponentMetadata> {
        None
    }
    fn schemes(&self) -> Vec<String> {
        Vec::new()
    }
    fn all_metadata(&self) -> Vec<ComponentMetadata> {
        Vec::new()
    }
}

#[test]
fn rschema_ignores_ambient_process_env() {
    let engine = LintEngine::new(Arc::new(EmptyCatalog)).with_default_rules();
    // String-typed field (`id` is strict "string" in ROUTE_SCHEMA): the
    // string-position placeholder is the rev-2 happy path.
    let source = "id: ${env:RC93WCT_AMBIENT:-hello}\nfrom: direct:start\nsteps:\n  - to: log:out\n";

    let rschema_keys = |diags: Vec<camel_lint::Diagnostic>| -> Vec<(Severity, String)> {
        diags
            .into_iter()
            .filter(|d| d.code == DiagnosticCode::RSchema)
            .map(|d| (d.severity, d.message))
            .collect()
    };

    // Control run: ambient variable absent.
    let control = rschema_keys(engine.lint(source));
    // Ambient run: variable poisoned to a value that would change every
    // observable (message, validation) if the engine read the environment.
    unsafe { std::env::set_var("RC93WCT_AMBIENT", "evil-ambient-value") };
    let with_ambient = rschema_keys(engine.lint(source));
    unsafe { std::env::remove_var("RC93WCT_AMBIENT") };

    assert_eq!(
        control, with_ambient,
        "ambient env must not change R-SCHEMA diagnostics"
    );
    assert!(
        control.iter().all(|(sev, _)| *sev != Severity::Error),
        "string-position default must be Error-free; got: {control:?}"
    );
    let infos: Vec<&(Severity, String)> = control
        .iter()
        .filter(|(sev, _)| *sev == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        1,
        "expected exactly one Info note for the substituted default; got: {control:?}"
    );
    assert!(
        infos[0].1.contains(":-hello"),
        "Info must report the literal `:-hello` default, not an ambient value; got: {}",
        infos[0].1
    );
}
