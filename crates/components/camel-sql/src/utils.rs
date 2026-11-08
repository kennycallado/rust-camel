use camel_component_api::CamelError;
use sqlx::any::AnyRow;
use sqlx::{Column, Row};
use tracing::warn;

/// Converts a database row to a JSON object.
///
/// Iterates columns and extracts values by trying types in order:
/// Option<i64>, Option<i32>, Option<f64>, Option<f32>, Option<bool>,
/// Option<String>, timestamp strings.
/// SQL NULLs are properly represented as JSON null.
///
/// A value the server sent is never substituted with null: anything outside the
/// decode matrix (including non-UTF-8 blobs and non-finite floats) fails with
/// `undecodable_column_error`. See the README type matrix.
pub(crate) fn row_to_json(row: &AnyRow) -> Result<serde_json::Value, CamelError> {
    let mut map = serde_json::Map::new();

    for (i, column) in row.columns().iter().enumerate() {
        let name = column.name().to_string();
        let type_name = column.type_info().to_string();

        let value = if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(i) {
            serde_json::Value::Number(v.into())
        } else if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(i) {
            serde_json::Value::Number(v.into())
        } else if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(i) {
            match serde_json::Number::from_f64(v) {
                Some(n) => serde_json::Value::Number(n),
                // NaN/±Infinity are real server-sent values; JSON cannot hold
                // them, so fail loudly instead of substituting null.
                None => return Err(undecodable_column_error(&name, &type_name)),
            }
        } else if let Ok(v) = row.try_get::<Option<f32>, _>(i) {
            f32_column_value(&name, &type_name, v)?
        } else if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(i) {
            serde_json::Value::Bool(v)
        } else if let Ok(Some(v)) = row.try_get::<Option<String>, _>(i) {
            serde_json::Value::String(v)
        } else if let Ok(Some(v)) = row.try_get::<Option<Vec<u8>>, _>(i) {
            match std::str::from_utf8(&v) {
                Ok(text) => decode_blob_text(text),
                // Binary data is a real server-sent value; JSON cannot hold
                // raw bytes, so fail loudly instead of substituting null.
                Err(_) => return Err(undecodable_column_error(&name, &type_name)),
            }
        } else if matches!(row.try_get::<Option<String>, _>(i), Ok(None)) {
            // Confirmed SQL NULL — no warning needed
            serde_json::Value::Null
        } else if row
            .try_get_raw(i)
            .is_ok_and(|raw| sqlx::ValueRef::is_null(&raw))
        {
            // SQL NULL of a kind the string decode cannot confirm (e.g. a
            // non-text column type) — stays null, the error below must not fire.
            serde_json::Value::Null
        } else {
            // Truly undecodable, non-null value — fail the route loudly.
            return Err(undecodable_column_error(&name, &type_name));
        };

        map.insert(name, value);
    }

    Ok(serde_json::Value::Object(map))
}

/// Maps a UTF-8 blob payload to its JSON value, normalizing date-like text to ISO 8601.
///
/// Mirrors the historical bytea handling: string content keeps date parsing;
/// the JSON value never substitutes null for the payload itself.
fn decode_blob_text(text: &str) -> serde_json::Value {
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        serde_json::Value::String(date.format("%Y-%m-%d").to_string())
    } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f") {
        serde_json::Value::String(dt.format("%Y-%m-%dT%H:%M:%S%.f").to_string())
    } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S") {
        serde_json::Value::String(dt.format("%Y-%m-%dT%H:%M:%S").to_string())
    } else if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(text) {
        serde_json::Value::String(
            dt.with_timezone(&chrono::Utc)
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
        )
    } else {
        serde_json::Value::String(text.to_string())
    }
}

/// Maps a decoded REAL (`f32`) column to its JSON value.
///
/// The sqlx `Any` driver decodes Postgres `REAL`/`FLOAT4` only as `f32` (strict
/// type compatibility), so `row_to_json` needs its own branch after `f64`.
/// Finite values widen exactly to `f64`; JSON cannot hold NaN/±Infinity, so the
/// `None` arm of `from_f64` (its non-finite rejection) fails loudly with the
/// typed error instead of substituting null.
fn f32_column_value(
    name: &str,
    type_name: &str,
    v: Option<f32>,
) -> Result<serde_json::Value, CamelError> {
    match v {
        // SQL NULL REAL stays null.
        None => Ok(serde_json::Value::Null),
        // Finite values widen exactly to f64; JSON cannot hold NaN/±Infinity,
        // so from_f64's None arm fails loudly instead of substituting null.
        Some(v) => match serde_json::Number::from_f64(f64::from(v)) {
            Some(n) => Ok(serde_json::Value::Number(n)),
            None => Err(undecodable_column_error(name, type_name)),
        },
    }
}

/// Builds the actionable error for a column the sqlx `Any` driver cannot decode.
///
/// Names the column, the driver-reported type, and the CAST workaround so the
/// route author can fix the query (see the README type matrix). SQL NULL is
/// handled before this is called: it stays JSON null and never reaches here.
pub(crate) fn undecodable_column_error(column: &str, type_name: &str) -> CamelError {
    let hint = if matches!(type_name.to_uppercase().as_str(), "NUMERIC" | "DECIMAL") {
        "CAST(col AS double precision) for an approximate value, or CAST(col AS text) to preserve the exact value"
    } else {
        "CAST(col AS text) to fetch the value as text"
    };
    CamelError::TypeConversionFailed(format!(
        "column '{column}' has type {type_name} which the SQL Any driver cannot decode; \
         rewrite the query, e.g. {}. See the camel-sql README type matrix",
        hint.replace("col", column)
    ))
}

/// Rewrites sqlx's unsupported-type fetch error into the actionable decode error.
///
/// The `Any` bridge raises `ColumnDecode` (source `AnyDriverError`) when a row
/// carries a column type outside the `Any` type set (e.g. Postgres NUMERIC or
/// TIMESTAMPTZ). The raw message names the column and type but no workaround;
/// this restores the CAST hint. Returns `None` for every other error so callers
/// keep their existing shape.
pub(crate) fn actionable_any_type_error(e: &sqlx::Error) -> Option<CamelError> {
    let (column, driver_message) = match e {
        sqlx::Error::ColumnDecode { index, source } => (index.as_str(), source.to_string()),
        sqlx::Error::AnyDriverError(source) => ("", source.to_string()),
        _ => return None,
    };
    const MARKER: &str = "Any driver does not support the Postgres type ";
    let raw = driver_message.split(MARKER).nth(1)?;
    let type_name = raw
        .trim()
        .strip_prefix("PgTypeInfo(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(raw.trim());
    if column.is_empty() {
        Some(undecodable_column_error("unknown", type_name))
    } else {
        Some(undecodable_column_error(column, type_name))
    }
}

/// Maps a sqlx fetch error, upgrading unsupported-column-type failures to the
/// actionable decode error (`actionable_any_type_error`); other errors keep the
/// existing `ProcessorError` shape.
pub(crate) fn fetch_error(context: &'static str, e: sqlx::Error) -> CamelError {
    if let Some(typed) = actionable_any_type_error(&e) {
        warn!(error = %typed, "{context}");
        return typed;
    }
    warn!(error = %e, "{context}");
    CamelError::ProcessorError(format!("Query execution failed: {}", e))
}
// ── Retry classification ─────────────────────────────────────────────

/// Classifies a `sqlx::Error` into retryable (transient network/connection)
/// vs permanent (configuration, TLS, pool shutdown).
///
/// Used by consumer and producer pool-connect retry loops.
///
/// # Retryable
/// - `sqlx::Error::Io` — TCP connection refused, DNS failure, etc.
/// - `sqlx::Error::Database` — the underlying driver may report "connection refused"
///   or "too many connections"; we check the error message for transient keywords.
/// - `sqlx::Error::PoolTimedOut` — pool exhausted, may recover under lower load.
///
/// # Permanent (not retried)
/// - `sqlx::Error::Configuration` — bad URL, unknown driver → cannot be fixed by retry.
/// - `sqlx::Error::Tls` — certificate problems → permanent.
/// - `sqlx::Error::PoolClosed` — pool shutdown is intentional → no retry.
/// - Other variants → default false (safety).
pub(crate) fn is_retryable_sqlx_error(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Io(_) => true,
        sqlx::Error::PoolTimedOut => true,
        sqlx::Error::PoolClosed => false,
        sqlx::Error::Configuration(_) => false,
        sqlx::Error::Tls(_) => false,
        sqlx::Error::Database(db_err) => {
            let msg = db_err.message().to_lowercase();
            msg.contains("connection refused")
                || msg.contains("cannot connect")
                || msg.contains("too many connections")
                || msg.contains("no connection to the server")
                || msg.contains("connection reset")
                || msg.contains("broken pipe")
                || msg.contains("server closed the connection")
                || msg.contains("timed out")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_retryable_io_error() {
        let err = sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "refused",
        ));
        assert!(is_retryable_sqlx_error(&err));
    }

    #[test]
    fn is_retryable_pool_timed_out() {
        let err = sqlx::Error::PoolTimedOut;
        assert!(is_retryable_sqlx_error(&err));
    }

    #[test]
    fn is_not_retryable_pool_closed() {
        let err = sqlx::Error::PoolClosed;
        assert!(!is_retryable_sqlx_error(&err));
    }

    #[test]
    fn is_not_retryable_configuration() {
        // Configuration errors are permanent (bad URL, unknown driver, etc.)
        let err = sqlx::Error::Configuration("unknown driver".into());
        assert!(!is_retryable_sqlx_error(&err));
    }

    #[test]
    fn is_not_retryable_tls() {
        // TLS errors are permanent (certificate issues)
        let err = sqlx::Error::Tls(Box::new(std::io::Error::other("certificate error")));
        assert!(!is_retryable_sqlx_error(&err));
    }

    #[test]
    fn is_retryable_database_connection_refused() {
        // Database errors with "connection refused" are retryable
        let err = sqlx::Error::Database(Box::new(MockDbError {
            message: "connection refused".into(),
            code: None,
        }));
        assert!(is_retryable_sqlx_error(&err));
    }

    #[test]
    fn is_retryable_database_too_many_connections() {
        let err = sqlx::Error::Database(Box::new(MockDbError {
            message: "too many connections".into(),
            code: None,
        }));
        assert!(is_retryable_sqlx_error(&err));
    }

    #[test]
    fn is_not_retryable_database_syntax_error() {
        // Permanent: syntax error won't fix itself
        let err = sqlx::Error::Database(Box::new(MockDbError {
            message: "syntax error at or near 'SELECT'".into(),
            code: None,
        }));
        assert!(!is_retryable_sqlx_error(&err));
    }

    #[test]
    fn is_not_retryable_database_unknown_host() {
        // DNS failures are permanent: hostname typo, missing record, etc.
        // Should surface via sqlx::Error::Io if it should be retried.
        let err = sqlx::Error::Database(Box::new(MockDbError {
            message: "could not translate host name to address: unknown host".into(),
            code: None,
        }));
        assert!(!is_retryable_sqlx_error(&err));
    }

    /// Minimal mock of sqlx's DatabaseError trait for unit-testing classification.
    struct MockDbError {
        message: String,
        #[allow(dead_code)]
        code: Option<Box<dyn std::fmt::Display + Send + Sync>>,
    }

    impl std::fmt::Debug for MockDbError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockDbError")
                .field("message", &self.message)
                .finish()
        }
    }

    impl std::fmt::Display for MockDbError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl std::error::Error for MockDbError {}

    impl sqlx::error::DatabaseError for MockDbError {
        fn message(&self) -> &str {
            &self.message
        }

        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }

        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
    }

    #[test]
    fn undecodable_numeric_error_names_column_type_and_cast() {
        let err = undecodable_column_error("price", "Numeric");
        let msg = err.to_string();
        assert!(msg.contains("price"), "column name missing: {msg}");
        assert!(msg.contains("Numeric"), "driver type missing: {msg}");
        assert!(
            msg.contains("CAST(price AS double precision)"),
            "numeric hint missing: {msg}"
        );
        assert!(
            msg.contains("CAST(price AS text)"),
            "text hint missing: {msg}"
        );
        assert!(
            matches!(err, CamelError::TypeConversionFailed(_)),
            "must be the typed conversion error"
        );
    }

    #[test]
    fn undecodable_timestamptz_error_suggests_text_cast() {
        let err = undecodable_column_error("created_at", "Timestamptz");
        let msg = err.to_string();
        assert!(msg.contains("created_at"), "column name missing: {msg}");
        assert!(msg.contains("Timestamptz"), "driver type missing: {msg}");
        assert!(
            msg.contains("CAST(created_at AS text)"),
            "text hint missing: {msg}"
        );
        assert!(
            !msg.contains("double precision"),
            "timestamps have no numeric hint: {msg}"
        );
    }

    #[test]
    fn unsupported_type_fetch_error_is_rewritten() {
        let e = sqlx::Error::ColumnDecode {
            index: "price".to_string(),
            source: "error in Any driver mapping: Any driver does not support the Postgres type PgTypeInfo(Numeric)"
                .into(),
        };
        let rewritten = actionable_any_type_error(&e).expect("unsupported type must rewrite");
        let msg = rewritten.to_string();
        assert!(msg.contains("price"), "column name missing: {msg}");
        assert!(msg.contains("Numeric"), "driver type missing: {msg}");
        assert!(
            msg.contains("CAST(price AS double precision)"),
            "workaround missing: {msg}"
        );
    }

    #[test]
    fn other_sqlx_errors_are_not_rewritten() {
        assert!(actionable_any_type_error(&sqlx::Error::PoolTimedOut).is_none());
        let decode = sqlx::Error::ColumnDecode {
            index: "price".to_string(),
            source: "mismatched types".into(),
        };
        assert!(actionable_any_type_error(&decode).is_none());
    }

    // ── REAL (f32) decode (bd sql-any-decode-fidelity) ───────────────

    #[test]
    fn real_f32_finite_decodes_as_json_number() {
        for (input, expected) in [(1.5_f32, 1.5_f64), (2.5_f32, 2.5_f64)] {
            match f32_column_value("r", "REAL", Some(input)) {
                Ok(serde_json::Value::Number(n)) => {
                    assert_eq!(
                        n.as_f64(),
                        Some(expected),
                        "REAL {input} must widen exactly to f64"
                    );
                }
                Ok(other) => panic!("REAL {input} must decode as a JSON number, got {other:?}"),
                Err(e) => panic!("finite REAL {input} must decode, got error: {e}"),
            }
        }
        // SQL NULL REAL stays null (it is not an undecodable value).
        match f32_column_value("r", "REAL", None) {
            Ok(serde_json::Value::Null) => {}
            other => panic!("SQL NULL REAL must stay JSON null, got {other:?}"),
        }
    }

    #[test]
    fn real_f32_non_finite_fails_typed() {
        for input in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let err = f32_column_value("r", "REAL", Some(input))
                .expect_err("non-finite REAL cannot be represented in JSON");
            match err {
                CamelError::TypeConversionFailed(msg) => {
                    assert!(msg.contains("column 'r'"), "column named: {msg}");
                    assert!(msg.contains("REAL"), "driver type named: {msg}");
                    assert!(msg.contains("CAST(r AS text)"), "workaround shown: {msg}");
                }
                other => panic!("non-finite REAL must fail with the typed error, got {other:?}"),
            }
        }
    }
}
