//! Engine-free public surface test.
//!
//! Runs under `--no-default-features` to prove the config/error re-exports
//! resolve at type position without the minijinja engine present.

use camel_template::ExternalTemplateLimitsConfig;
use camel_template::ResolvedExternalTemplateLimits;
use camel_template::TemplateReloadError;

/// Type-check the engine-free re-exports: each must resolve at type position
/// with the engine absent. No engine state is constructed.
#[test]
fn engine_free_public_surface_resolves() {
    fn assert_limits_config_takes_ref(_: &ExternalTemplateLimitsConfig) {}
    fn assert_resolved_limits_takes_ref(_: &ResolvedExternalTemplateLimits) {}
    fn assert_reload_error_takes_ref(_: &TemplateReloadError) {}

    // Type-position bindings only — no values, no engine state.
    let limits: Option<ExternalTemplateLimitsConfig> = None;
    let resolved: Option<ResolvedExternalTemplateLimits> = None;
    let error: Option<TemplateReloadError> = None;

    // The parameterized functions above pin the types into call positions,
    // exercising name resolution beyond the `Option` bindings.
    if let (Some(l), Some(r), Some(e)) = (limits, resolved, error) {
        assert_limits_config_takes_ref(&l);
        assert_resolved_limits_takes_ref(&r);
        assert_reload_error_takes_ref(&e);
    }
}
