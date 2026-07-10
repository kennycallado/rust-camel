use camel_component_api::CamelError;
use sqlx::any::AnyArguments;
use sqlx::any::AnyRow;
use sqlx::query::Query;
use sqlx::{Any, Column, Row};
use tracing::warn;

/// Converts a database row to a JSON object.
///
/// Iterates columns and extracts values by trying types in order:
/// Option<i64>, Option<i32>, Option<f64>, Option<bool>, Option<String>, timestamp strings.
/// SQL NULLs are properly represented as JSON null.
pub(crate) fn row_to_json(row: &AnyRow) -> Result<serde_json::Value, CamelError> {
    let mut map = serde_json::Map::new();

    for (i, column) in row.columns().iter().enumerate() {
        let name = column.name().to_string();

        let value = if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(i) {
            serde_json::Value::Number(v.into())
        } else if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(i) {
            serde_json::Value::Number(v.into())
        } else if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(i) {
            serde_json::Number::from_f64(v)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null)
        } else if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(i) {
            serde_json::Value::Bool(v)
        } else if let Ok(Some(v)) = row.try_get::<Option<String>, _>(i) {
            serde_json::Value::String(v)
        } else if let Ok(Some(v)) = row.try_get::<Option<Vec<u8>>, _>(i) {
            if let Ok(text) = std::str::from_utf8(&v) {
                if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
                    serde_json::Value::String(date.format("%Y-%m-%d").to_string())
                } else if let Ok(dt) =
                    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
                {
                    serde_json::Value::String(dt.format("%Y-%m-%dT%H:%M:%S%.f").to_string())
                } else if let Ok(dt) =
                    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                {
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
            } else {
                serde_json::Value::Null
            }
        } else if matches!(row.try_get::<Option<String>, _>(i), Ok(None)) {
            // Confirmed SQL NULL — no warning needed
            serde_json::Value::Null
        } else {
            // Truly undecodable — log a warning
            warn!(
                column = column.name(),
                "Could not decode column value, falling back to null"
            );
            serde_json::Value::Null
        };

        map.insert(name, value);
    }

    Ok(serde_json::Value::Object(map))
}

/// Binds JSON values to a sqlx query.
///
/// Handles: Null, Bool, Number (i64/f64), String, Array/Object (serialized to JSON string).
pub(crate) fn bind_json_values<'q>(
    mut query: Query<'q, Any, AnyArguments<'q>>,
    values: &'q [serde_json::Value],
) -> Query<'q, Any, AnyArguments<'q>> {
    for value in values {
        query = match value {
            serde_json::Value::Null => query.bind(Option::<String>::None),
            serde_json::Value::Bool(b) => query.bind(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    query.bind(i)
                } else if let Some(f) = n.as_f64() {
                    query.bind(f)
                } else {
                    query.bind(n.to_string())
                }
            }
            serde_json::Value::String(s) => query.bind(s.as_str()),
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                query.bind(value.to_string())
            }
        };
    }
    query
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
}
