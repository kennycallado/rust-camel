//! Client-visible error projections for the ADR-0097 business error envelope.
//!
//! ADR-0097 splits error fields into two trust classes: `code` and `kind` are
//! operator-authored constants and safe to disclose, while `message` and
//! `data` are exchange-derived (untrusted) and log-only. The helpers here are
//! the single source of truth for both reply bodies, so camel-http and the MCP
//! component cannot drift apart.

/// Generic non-business error body.
///
/// Reuses camel-http's catch-all wording and discloses nothing about the
/// failure beyond "internal error". Business errors use
/// [`business_envelope`] instead (ADR-0097 trust classes).
pub const INTERNAL_ERROR_BODY_JSON: &str = r#"{"error":"Internal Server Error"}"#;

/// Build the business-error reply body for a `code`/`kind` pair.
///
/// The result has exactly two keys, per ADR-0097: `message` and `data` are
/// log-only and never serialized into a client-visible body, because they are
/// exchange-derived and untrusted.
#[must_use]
pub fn business_envelope(code: &str, kind: &str) -> serde_json::Value {
    serde_json::json!({ "error": code, "kind": kind })
}

#[cfg(test)]
mod tests {
    use super::{INTERNAL_ERROR_BODY_JSON, business_envelope};

    #[test]
    fn business_envelope_has_exactly_two_keys() {
        let code = "ORDER_NOT_FOUND";
        let kind = "not-found";

        let value = business_envelope(code, kind);

        let serialized =
            serde_json::to_string(&value).expect("business envelope must always serialize");
        assert_eq!(
            serialized,
            r#"{"error":"ORDER_NOT_FOUND","kind":"not-found"}"#
        );
        let object = value.as_object().expect("business envelope is an object");
        assert_eq!(
            object.len(),
            2,
            "business envelope must have exactly two keys"
        );
        assert!(!object.contains_key("message"), "message is log-only");
        assert!(!object.contains_key("data"), "data is log-only");
    }

    #[test]
    fn internal_error_body_is_exact_constant() {
        assert_eq!(
            INTERNAL_ERROR_BODY_JSON,
            r#"{"error":"Internal Server Error"}"#
        );

        let parsed: serde_json::Value =
            serde_json::from_str(INTERNAL_ERROR_BODY_JSON).expect("constant must be valid JSON");
        let object = parsed.as_object().expect("constant parses to an object");
        assert_eq!(object.len(), 1, "constant must carry exactly one key");
        assert!(object.contains_key("error"), "constant must carry `error`");
    }
}
