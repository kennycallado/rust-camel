//! Diagnostic types for the camel-lint engine.
//!
//! Defines spans, severity levels, diagnostic codes, and the `Diagnostic`
//! struct that rules produce.

// ---------------------------------------------------------------------------
// Span — byte-exact range into source text
// ---------------------------------------------------------------------------

/// A byte-exact range into the source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

// ---------------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl std::fmt::Display for Severity {
    /// Stable lowercase string used by the corpus baseline contract.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        })
    }
}

// ---------------------------------------------------------------------------
// DiagnosticCode
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UriKnownSubCode {
    UnverifiedScheme,
    UnknownOption,
    MissingRequiredOption,
    KindMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticCode {
    RSyn,
    RSchema,
    RUriKnown(UriKnownSubCode),
    RSecret,
    RDeprecated,
}

impl std::fmt::Display for DiagnosticCode {
    /// Canonical stable string for a diagnostic code.
    ///
    /// This is the baseline's stable contract: `R-SYN`, `R-SCHEMA`,
    /// `R-SECRET`, `R-DEPRECATED`, and `R-URI-known:<sub>` where `<sub>` is
    /// `unverified-scheme` / `unknown-option` / `kind-mismatch` /
    /// `missing-required-option`. Never rely on the `Debug` repr — it is not
    /// a stability boundary.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticCode::RSyn => f.write_str("R-SYN"),
            DiagnosticCode::RSchema => f.write_str("R-SCHEMA"),
            DiagnosticCode::RSecret => f.write_str("R-SECRET"),
            DiagnosticCode::RDeprecated => f.write_str("R-DEPRECATED"),
            DiagnosticCode::RUriKnown(sub) => {
                let s = match sub {
                    UriKnownSubCode::UnverifiedScheme => "unverified-scheme",
                    UriKnownSubCode::UnknownOption => "unknown-option",
                    UriKnownSubCode::KindMismatch => "kind-mismatch",
                    UriKnownSubCode::MissingRequiredOption => "missing-required-option",
                };
                write!(f, "R-URI-known:{s}")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Diagnostic
// ---------------------------------------------------------------------------

/// A diagnostic produced by a lint rule.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub span: Span,
    pub message: String,
    pub fix: Option<Fix>,
}

// ---------------------------------------------------------------------------
// Fix
// ---------------------------------------------------------------------------

/// A suggested fix: replace `span` in the source with `replacement`.
#[derive(Clone, Debug)]
pub struct Fix {
    pub span: Span,
    pub replacement: String,
}
