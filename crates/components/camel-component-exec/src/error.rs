//! ExecError — typed errors for the exec component.
//!
//! Pre/during-spawn failures (no output to carry) are `Err`. Post-spawn
//! conditions with output (timeout, exit-code mismatch) are NON-error outcomes
//! (producer returns Ok with result + headers). See spec §Error handling.

/// Errors raised by the exec producer. All surface as
/// `CamelError::ProcessorErrorWithSource(msg, Arc<ExecError>)`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ExecError {
    #[error("executable/profile not allowlisted (fail-closed): {requested:?}")]
    NotAllowlisted { requested: String },

    #[error("arg policy denied {arg:?}: {reason}")]
    ArgPolicyDenied { arg: String, reason: String },

    #[error("shell executable rejected without allow_shell: {executable:?}")]
    ShellRejected { executable: String },

    #[error("working dir escapes workspace root: {path:?}")]
    InvalidWorkDir { path: String },

    #[error("stdin exceeds cap ({size} > {max} bytes)")]
    StdinTooLarge { size: usize, max: usize },

    #[error("malformed CamelExecArgs header: {0}")]
    InvalidArgs(String),

    #[error("spawn failed: {0}")]
    Spawn(#[from] std::io::Error),
}
