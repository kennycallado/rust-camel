//! Identifier validation and SurrealQL query builders for operations that
//! have no SDK fluent equivalent.
//!
//! # Identifier validation
//! SurrealDB identifiers (table names, field names, edge types) must be
//! ASCII alphanumeric and underscores only. Rejects SQL injection patterns
//! (semicolons, dashes, spaces), unicode, and empty strings.
//!
//! # When raw queries are needed
//! The SurrealDB SDK provides fluent methods (`create`, `select`, `update`,
//! `delete`) that should be used for standard CRUD operations. Raw query
//! builders are kept here only for:
//! - **RELATE raw fallback** — retained for direct SurrealQL construction tests.
//! - **Vector KNN search** — the `<|K,METRIC|>` syntax is SurrealQL-specific.
//! - **User-supplied SurrealQL** — the `query` operation passes through
//!   arbitrary SQL from the route.
//!
//! All identifiers that are interpolated into raw SQL are validated via
//! [`validate_identifier`] or [`validate_record_key`] first.

use crate::error::SurrealDbError;
use surrealdb::types::{RecordId, RecordIdKey, Uuid};

/// Validate a SurrealDB identifier (table, field, or edge name).
///
/// Rules:
/// - Non-empty
/// - ASCII alphanumeric + underscore only
/// - Must not start with a digit
/// - No spaces, dashes, semicolons, or unicode
pub fn validate_identifier(s: &str) -> Result<(), SurrealDbError> {
    if s.is_empty() {
        return Err(SurrealDbError::InvalidIdentifier(
            "identifier must not be empty".into(),
        ));
    }
    if !s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return Err(SurrealDbError::InvalidIdentifier(format!(
            "'{s}' must start with ASCII letter or underscore"
        )));
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(SurrealDbError::InvalidIdentifier(format!(
            "'{s}' contains invalid characters (only ASCII alphanumeric and underscore allowed)"
        )));
    }
    Ok(())
}

/// Validate a SurrealDB record key (the value part of a RecordId like `user:{key}`).
///
/// More permissive than [`validate_identifier`]: allows leading digits, hyphens
/// (for UUIDs and slugs), periods, and colons (for composite keys). Still
/// rejects SQL-injection patterns (semicolons, quotes, whitespace, backslash,
/// unicode).
///
/// This is defence-in-depth for raw builders and URI-to-SDK `RecordId`
/// conversion.
pub fn validate_record_key(s: &str) -> Result<(), SurrealDbError> {
    if s.is_empty() {
        return Err(SurrealDbError::InvalidIdentifier(
            "record key must not be empty".into(),
        ));
    }
    if s.contains([';', '\'', '"', '\n', '\r', '\t', ' ', '\\']) {
        return Err(SurrealDbError::InvalidIdentifier(format!(
            "'{s}' contains forbidden characters (semicolon, quote, whitespace, or backslash)"
        )));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == ':')
    {
        return Err(SurrealDbError::InvalidIdentifier(format!(
            "'{s}' contains invalid characters (only ASCII alphanumeric, underscore, hyphen, period, colon allowed)"
        )));
    }
    Ok(())
}

/// Build an SDK `RecordId` from URI text while preserving SurrealQL's common
/// unquoted key semantics: all-digit keys are numeric, UUID-shaped keys are
/// UUIDs, everything else is a string key.
pub(crate) fn record_id_from_uri(table: &str, key: &str) -> Result<RecordId, SurrealDbError> {
    validate_identifier(table)?;
    validate_record_key(key)?;

    if let Ok(number) = key.parse::<i64>() {
        return Ok(RecordId::new(table, RecordIdKey::Number(number)));
    }

    if let Ok(uuid) = key.parse::<Uuid>() {
        return Ok(RecordId::new(table, RecordIdKey::Uuid(uuid)));
    }

    Ok(RecordId::new(table, RecordIdKey::String(key.to_string())))
}

/// Build `RELATE {from_table}:{from_id}->\`{edge}\`->{to_table}:{to_id} CONTENT $body`.
///
/// Raw fallback for RELATE construction. The producer uses the SDK
/// `insert(edge).relation(data)` path; this builder remains for tests and any
/// future grammar-only paths. All 5 components are validated; `$body` is bound
/// by the caller.
pub fn build_relate_sql(
    from_table: &str,
    from_id: &str,
    edge: &str,
    to_table: &str,
    to_id: &str,
) -> Result<String, SurrealDbError> {
    validate_identifier(from_table)?;
    validate_record_key(from_id)?;
    validate_identifier(edge)?;
    validate_identifier(to_table)?;
    validate_record_key(to_id)?;
    Ok(format!(
        "RELATE {from_table}:{from_id}->`{edge}`->{to_table}:{to_id} CONTENT $body"
    ))
}

/// Build vector KNN search query.
///
/// SurrealDB v3 KNN syntax:
///   SELECT *, vector::distance::knn() AS distance
///   FROM {table}
///   WHERE {vector_field} <|{top_k},{metric}|> $vector
///   ORDER BY distance
///
/// `table` and `vector_field` are validated identifiers interpolated directly
/// (they appear in positions where bind variables are not supported by the
/// SurrealQL grammar). `$vector` is bound by the caller.
pub fn build_vector_search_sql(
    table: &str,
    vector_field: &str,
    top_k: usize,
    metric: &str,
) -> String {
    format!(
        "SELECT *, vector::distance::knn() AS distance FROM {} WHERE {} <|{},{}|> $vector ORDER BY distance",
        table, vector_field, top_k, metric
    )
}

/// Build `LIVE SELECT * FROM {table}`.
///
/// `table` is a validated identifier interpolated directly. The consumer
/// uses the SDK's `db.select().live()` fluent API instead of this builder,
/// but it is kept for documentation and potential future use.
pub fn build_live_sql(table: &str) -> String {
    format!("LIVE SELECT * FROM {}", table)
}

#[cfg(test)]
mod tests {
    // =========================================================================
    // Identifier validation tests (11)
    // =========================================================================

    #[test]
    fn valid_plain_table() {
        assert!(super::validate_identifier("users").is_ok());
    }

    #[test]
    fn valid_with_underscores() {
        assert!(super::validate_identifier("user_profiles").is_ok());
    }

    #[test]
    fn valid_with_numbers() {
        assert!(super::validate_identifier("tbl_42").is_ok());
    }

    #[test]
    fn valid_single_letter() {
        assert!(super::validate_identifier("x").is_ok());
    }

    #[test]
    fn valid_uppercase() {
        assert!(super::validate_identifier("USERS").is_ok());
    }

    #[test]
    fn validate_starts_with_underscore() {
        assert!(super::validate_identifier("_internal").is_ok());
    }

    #[test]
    fn reject_identifier_with_colon() {
        // user:42 is a RecordId, not an identifier
        assert!(super::validate_identifier("user:42").is_err());
    }

    #[test]
    fn invalid_empty_string() {
        assert!(super::validate_identifier("").is_err());
    }

    #[test]
    fn invalid_contains_dash() {
        let err = super::validate_identifier("user-profiles").unwrap_err();
        assert!(err.to_string().contains("identifier"));
    }

    #[test]
    fn invalid_contains_space() {
        let err = super::validate_identifier("user profiles").unwrap_err();
        assert!(err.to_string().contains("identifier"));
    }

    #[test]
    fn invalid_contains_semicolon() {
        let err = super::validate_identifier("users;DROP TABLE").unwrap_err();
        assert!(err.to_string().contains("identifier"));
    }

    #[test]
    fn invalid_unicode() {
        let err = super::validate_identifier("usuários").unwrap_err();
        assert!(err.to_string().contains("identifier"));
    }

    #[test]
    fn invalid_starts_with_number() {
        let err = super::validate_identifier("1table").unwrap_err();
        assert!(err.to_string().contains("identifier"));
    }

    // =========================================================================
    // Raw SQL builder tests (RELATE, vector KNN, LIVE)
    // =========================================================================

    #[test]
    fn build_relate_query() {
        let sql = super::build_relate_sql("user", "1", "knows", "topic", "42").unwrap();
        assert_eq!(sql, "RELATE user:1->`knows`->topic:42 CONTENT $body");
    }

    #[test]
    fn build_relate_rejects_invalid_edge() {
        assert!(super::build_relate_sql("user", "1", "bad edge", "topic", "42").is_err());
    }

    #[test]
    fn build_relate_rejects_invalid_from_table() {
        assert!(super::build_relate_sql("user table", "1", "knows", "topic", "42").is_err());
    }

    #[test]
    fn build_relate_rejects_invalid_to_id() {
        assert!(super::build_relate_sql("user", "1", "knows", "topic", "bad id").is_err());
    }

    #[test]
    fn build_relate_accepts_uuid_record_key() {
        // UUIDs contain hyphens — validate_record_key must accept them
        assert!(
            super::build_relate_sql(
                "user",
                "550e8400-e29b-41d4-a716-446655440000",
                "knows",
                "topic",
                "42"
            )
            .is_ok()
        );
    }

    #[test]
    fn sdk_record_id_uses_numeric_key_for_numeric_uri_segment() {
        let rid = super::record_id_from_uri("user", "1").unwrap();
        assert!(matches!(rid.key, surrealdb::types::RecordIdKey::Number(1)));
    }

    #[test]
    fn sdk_record_id_uses_string_key_for_alpha_uri_segment() {
        let rid = super::record_id_from_uri("user", "alice").unwrap();
        assert!(matches!(rid.key, surrealdb::types::RecordIdKey::String(ref s) if s == "alice"));
    }

    #[test]
    fn sdk_record_id_uses_uuid_key_for_uuid_uri_segment() {
        let rid =
            super::record_id_from_uri("user", "550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert!(matches!(rid.key, surrealdb::types::RecordIdKey::Uuid(_)));
    }

    #[test]
    fn validate_record_key_rejects_semicolon() {
        assert!(super::validate_record_key("1;DROP TABLE").is_err());
    }

    #[test]
    fn validate_record_key_rejects_space() {
        assert!(super::validate_record_key("bad id").is_err());
    }

    #[test]
    fn validate_record_key_rejects_quote() {
        assert!(super::validate_record_key("1'OR'1").is_err());
    }

    #[test]
    fn build_vector_search_query() {
        let sql = super::build_vector_search_sql("embeddings", "embedding", 5, "COSINE");
        assert_eq!(
            sql,
            "SELECT *, vector::distance::knn() AS distance FROM embeddings WHERE embedding <|5,COSINE|> $vector ORDER BY distance"
        );
    }

    #[test]
    fn build_live_query() {
        let sql = super::build_live_sql("events");
        assert_eq!(sql, "LIVE SELECT * FROM events");
    }
}
