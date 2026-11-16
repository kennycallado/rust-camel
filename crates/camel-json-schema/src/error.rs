//! Errors raised while reading, parsing, or compiling a JSON Schema.

/// Failure raised while opening, parsing, or compiling a JSON Schema.
///
/// The `Display` text of every variant is stable: consumers surface it
/// verbatim, and the variants that existed before ADR-0100 keep their
/// pre-confinement literals byte-identical.
#[must_use]
#[derive(Debug, Clone, thiserror::Error)]
pub enum SchemaError {
    /// An external reference resolved outside the schema root and was refused.
    #[error("external $ref '{uri}' rejected: outside schema root '{root}'")]
    ExternalRefRejected {
        /// Offending reference: the decoded candidate path for path-based
        /// rejections, the raw string for broken `file:` syntax or invalid
        /// UTF-8, and the resolved URI for inline sources.
        uri: String,
        /// Schema root the reference had to stay inside.
        root: String,
    },
    /// A reference resolved to a remote scheme, which this crate never fetches.
    #[error(
        "remote $ref '{uri}' rejected: schemas resolve from local files only (ADR-0100); vendor the file"
    )]
    RemoteRefRejected {
        /// Resolved reference URI.
        uri: String,
    },
    /// A referenced schema file exceeded the per-file size cap.
    #[error("schema file '{path}' exceeds {limit} bytes")]
    FileTooLarge {
        /// Referenced file path.
        path: String,
        /// Per-file byte cap.
        limit: u64,
    },
    /// The root schema file exceeded the per-file size cap.
    #[error("schema file '{path}' exceeds {limit} bytes")]
    RootTooLarge {
        /// Root schema path.
        path: String,
        /// Per-file byte cap.
        limit: u64,
    },
    /// Compiling the root schema would exceed the resource-count cap.
    #[error("schema resource limit exceeded: {limit} resources")]
    TooManyResources {
        /// Resource-count cap.
        limit: usize,
    },
    /// Compiling the root schema would exceed the total-byte cap.
    #[error("schema total size budget exceeded: {limit} bytes")]
    TotalBudgetExceeded {
        /// Total-byte cap.
        limit: u64,
    },
    /// A referenced path exists but is not a regular file.
    #[error("schema path '{path}' is not a regular file")]
    NotARegularFile {
        /// Referenced path.
        path: String,
    },
    /// A schema file could not be opened.
    #[error("failed to open schema file '{path}': {message}")]
    Open {
        /// Schema path.
        path: String,
        /// Underlying I/O failure text.
        message: String,
    },
    /// A schema file could not be read.
    #[error("failed to read schema file '{path}': {message}")]
    Read {
        /// Schema path.
        path: String,
        /// Underlying I/O failure text.
        message: String,
    },
    /// A YAML schema file was not valid UTF-8.
    #[error("YAML schema '{path}' is not valid UTF-8: {message}")]
    Utf8 {
        /// Schema path.
        path: String,
        /// Underlying UTF-8 failure text.
        message: String,
    },
    /// The root document could not be parsed.
    ///
    /// `message` already carries the full final text, for example
    /// `invalid JSON schema '<path>': <cause>` or
    /// `failed to convert YAML schema to JSON '<path>': <cause>`, so a
    /// consumer can surface it verbatim.
    #[error("{message}")]
    Parse {
        /// Schema path the message names.
        path: String,
        /// Full final parse-failure text.
        message: String,
    },
    /// The parsed schema could not be compiled into a validator.
    #[error("{message}")]
    Compile {
        /// Compile-failure text.
        message: String,
    },
}
