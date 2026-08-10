//! Rule trait — the contract that every lint rule implements.

use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::document::Document;
use camel_api::component_metadata::ComponentMetadataCatalog;

/// A lint rule that analyzes a `Document` against a component metadata
/// catalog and produces zero or more diagnostics.
pub trait Rule: Send + Sync {
    /// Analyze the document and return any diagnostics.
    fn analyze(&self, doc: &Document, catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic>;

    /// Return the diagnostic code for this rule.
    fn code(&self) -> DiagnosticCode;
}
