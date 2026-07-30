use camel_api::CamelError;

/// Errors raised by the external template component during acquisition,
/// compilation, reload, and the resolve-time validation of operator-supplied
/// resource limits.
///
/// All variants carry enough context to drive operator-visible logging while
/// staying `Clone` so the error can be moved into a `BoxProcessor` response
/// (`CamelError: Clone`) without consuming the original.
#[derive(Debug, Clone, thiserror::Error)]
pub enum TemplateReloadError {
    #[error("template acquisition failed: {0}")]
    Acquire(String),

    #[error("template compilation failed: {0}")]
    Compile(String),

    #[error("template path escapes configured root: {0}")]
    PathEscape(String),

    #[error("template dependency cycle detected: {0}")]
    Cycle(String),

    #[error("duplicate template identity in dependency closure: {0}")]
    DuplicateIdentity(String),

    /// A limit field was set to zero at resolve time. Zero is rejected because
    /// the limits enforce fail-closed bounding; allowing zero would silently
    /// disable the bound.
    #[error("template bound exceeded: {0}")]
    BoundExceeded(&'static str),

    /// The reload exceeded `reload_timeout_ms`; the late build must not swap.
    #[error("template reload exceeded reload_timeout_ms")]
    Timeout,

    /// A build tagged at a generation that has already been superseded
    /// reached the commit step and must be rejected.
    #[error("template build tagged with a stale generation")]
    StaleGeneration,
}

impl From<TemplateReloadError> for CamelError {
    fn from(err: TemplateReloadError) -> Self {
        CamelError::TemplateReload(err.to_string())
    }
}
