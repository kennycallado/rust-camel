use camel_component_api::CamelError;
use sqlx::any::AnyArguments;
use sqlx::any::AnyRow;
use sqlx::any::AnyTypeInfoKind;
use sqlx::query::Query;
use sqlx::{Any, Column, Row};
use tracing::warn;

/// Converts a database row to a JSON object.
///
/// Iterates columns and extracts values by trying types in order:
/// Option<i64>, Option<i32>, Option<f64>, Option<bool>, Option<String>, timestamp strings.
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

/// Binds JSON values to a sqlx query.
///
/// Handles: Null, Bool, Number (i64/f64), String, Array/Object (serialized to JSON string).
pub(crate) fn bind_json_values<'q>(
    mut query: Query<'q, Any, AnyArguments>,
    values: &'q [serde_json::Value],
) -> Query<'q, Any, AnyArguments> {
    for value in values {
        query = bind_single_json_value(query, value);
    }
    query
}

/// Binds one JSON value through the default JSON path.
///
/// Handles: Null, Bool, Number (i64/f64), String, Array/Object (serialized to JSON string).
fn bind_single_json_value<'q>(
    query: Query<'q, Any, AnyArguments>,
    value: &'q serde_json::Value,
) -> Query<'q, Any, AnyArguments> {
    match value {
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
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => query.bind(value.to_string()),
    }
}

// ── Batch bind-kind normalization (bd rc-fef4z) ──────────────────────

/// Conservative exactness limit for integers bound as f64 (owner-specified
/// policy): integers with magnitude above 2^53 fail with a typed error
/// instead of silently rounding.
const F64_EXACT_INT_LIMIT: i128 = 9_007_199_254_740_992;

/// Exactly representable `i64::MIN` as f64 (`-2^63`).
const I64_MIN_AS_F64: f64 = -9_223_372_036_854_775_808.0;

/// The first f64 above every `i64` value (`2^63`; `i64::MAX as f64` rounds
/// up to this, so the upper bound is exclusive).
const I64_BOUND_EXCLUSIVE_AS_F64: f64 = 9_223_372_036_854_775_808.0;

/// Wire bind encoding for a described numeric parameter position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BindTarget {
    I16,
    I32,
    I64,
    F32,
    F64,
}

impl BindTarget {
    fn name(self) -> &'static str {
        match self {
            BindTarget::I16 => "SMALLINT",
            BindTarget::I32 => "INTEGER",
            BindTarget::I64 => "BIGINT",
            BindTarget::F32 => "REAL",
            BindTarget::F64 => "DOUBLE PRECISION",
        }
    }
}

/// Maps a described parameter type to its exact wire bind encoding.
///
/// Broad `i64`/`f64` categories are not used: an `i64` payload under a
/// described `int4`/`int2` OID fails the server's binary width check.
pub(crate) fn bind_target_from_any_kind(kind: AnyTypeInfoKind) -> Option<BindTarget> {
    match kind {
        AnyTypeInfoKind::SmallInt => Some(BindTarget::I16),
        AnyTypeInfoKind::Integer => Some(BindTarget::I32),
        AnyTypeInfoKind::BigInt => Some(BindTarget::I64),
        AnyTypeInfoKind::Real => Some(BindTarget::F32),
        AnyTypeInfoKind::Double => Some(BindTarget::F64),
        _ => None,
    }
}

/// Fallback scan when the server description is unavailable: a parameter
/// position binds as the float encoding once any row carries a float JSON
/// number there; positions carrying only integers bind as the integer
/// encoding; positions without any JSON number keep the default binding.
pub(crate) fn scan_bind_targets(rows: &[&[serde_json::Value]]) -> Vec<Option<BindTarget>> {
    let width = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let mut targets = vec![None; width];

    for row in rows {
        for (position, value) in row.iter().enumerate() {
            let serde_json::Value::Number(n) = value else {
                continue;
            };
            // serde_json never converts an f64-backed number in `as_i64`,
            // so a float literal is exactly: no i64, but an f64.
            if n.as_i64().is_none() && n.as_f64().is_some() {
                targets[position] = Some(BindTarget::F64);
            } else if targets[position].is_none() && n.as_i64().is_some() {
                targets[position] = Some(BindTarget::I64);
            }
        }
    }

    targets
}

/// A JSON value normalized for one parameter position's wire encoding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NormalizedBind {
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Null(BindTarget),
    /// No described numeric target — bind through the default JSON path.
    Default,
}

/// Normalizes one JSON value against its position's bind target.
///
/// Numeric values are converted to the target encoding with checked
/// conversions (ties-to-even rounding for float -> integer, the 2^53
/// conservative guard for integer -> float, f32 overflow/underflow checks).
/// Non-numeric values at described numeric positions fail: raw text or
/// bool payloads must never reach a numeric OID, because an 8-byte string
/// silently reinterprets as float8 bits.
pub(crate) fn normalize_for_target(
    value: &serde_json::Value,
    target: Option<BindTarget>,
    item: usize,
    position: usize,
) -> Result<NormalizedBind, CamelError> {
    let Some(target) = target else {
        return Ok(NormalizedBind::Default);
    };

    let reject = |reason: String| {
        Err(CamelError::ProcessorError(format!(
            "Batch item {item}, parameter {position}: {reason}"
        )))
    };

    match value {
        serde_json::Value::Null => Ok(NormalizedBind::Null(target)),
        serde_json::Value::Number(n) => normalize_number(n, target, item, position),
        other => reject(format!(
            "{} value at a numeric parameter; batch mode requires numeric JSON at described numeric positions",
            json_kind_name(other)
        )),
    }
}

fn normalize_number(
    n: &serde_json::Number,
    target: BindTarget,
    item: usize,
    position: usize,
) -> Result<NormalizedBind, CamelError> {
    // Unsigned magnitudes beyond i64::MAX are detected through the u64
    // value before any lossy f64 conversion.
    if let Some(u) = n.as_u64()
        && n.as_i64().is_none()
    {
        return Err(reject_number(
            item,
            position,
            format!(
                "unsigned integer value {u} exceeds the described {} parameter range",
                target.name()
            ),
        ));
    }

    if let Some(i) = n.as_i64() {
        return match target {
            BindTarget::I64 => Ok(NormalizedBind::I64(i)),
            BindTarget::I32 => i32::try_from(i).map(NormalizedBind::I32).map_err(|_| {
                reject_number(
                    item,
                    position,
                    format!("integer value {i} does not fit in the described INTEGER parameter"),
                )
            }),
            BindTarget::I16 => i16::try_from(i).map(NormalizedBind::I16).map_err(|_| {
                reject_number(
                    item,
                    position,
                    format!("integer value {i} does not fit in the described SMALLINT parameter"),
                )
            }),
            BindTarget::F64 => {
                if i128::from(i).abs() > F64_EXACT_INT_LIMIT {
                    return Err(reject_number(
                        item,
                        position,
                        format!(
                            "integer value {i} exceeds 2^53 and would lose precision bound as f64"
                        ),
                    ));
                }
                Ok(NormalizedBind::F64(i as f64))
            }
            BindTarget::F32 => {
                if i128::from(i).abs() > F64_EXACT_INT_LIMIT {
                    return Err(reject_number(
                        item,
                        position,
                        format!(
                            "integer value {i} exceeds 2^53 and would lose precision bound as f32"
                        ),
                    ));
                }
                to_f32_checked(i as f64, item, position).map(NormalizedBind::F32)
            }
        };
    }

    let Some(f) = n.as_f64() else {
        return Err(reject_number(
            item,
            position,
            "number value cannot be represented as f64".to_string(),
        ));
    };

    match target {
        BindTarget::F64 => Ok(NormalizedBind::F64(f)),
        BindTarget::F32 => to_f32_checked(f, item, position).map(NormalizedBind::F32),
        BindTarget::I64 => f64_to_int(f, item, position, target).map(NormalizedBind::I64),
        BindTarget::I32 | BindTarget::I16 => {
            let rounded = f64_to_int(f, item, position, target)?;
            let reason = |v: i64| {
                format!(
                    "float value {f} rounds to {v}, outside the described {} parameter range",
                    target.name()
                )
            };
            if target == BindTarget::I32 {
                i32::try_from(rounded)
                    .map(NormalizedBind::I32)
                    .map_err(|_| reject_number(item, position, reason(rounded)))
            } else {
                i16::try_from(rounded)
                    .map(NormalizedBind::I16)
                    .map_err(|_| reject_number(item, position, reason(rounded)))
            }
        }
    }
}

/// Rounds with ties-to-even (Postgres `rint` semantics) and range-checks
/// against `i64` before the cast, so the conversion can never saturate.
fn f64_to_int(f: f64, item: usize, position: usize, target: BindTarget) -> Result<i64, CamelError> {
    let rounded = f.round_ties_even();
    if !rounded.is_finite() || rounded < I64_MIN_AS_F64 || rounded >= I64_BOUND_EXCLUSIVE_AS_F64 {
        return Err(reject_number(
            item,
            position,
            format!(
                "float value {f} rounds outside the described {} parameter range",
                target.name()
            ),
        ));
    }
    Ok(rounded as i64)
}

/// Converts f64 to f32 with the same overflow/underflow rejection Postgres
/// applies in its `float8` -> `float4` cast: a finite value that would
/// become infinite, or a non-zero value that would become zero, fails.
fn to_f32_checked(f: f64, item: usize, position: usize) -> Result<f32, CamelError> {
    let g = f as f32;
    if g.is_infinite() {
        return Err(reject_number(
            item,
            position,
            format!("float value {f} overflows the described REAL parameter"),
        ));
    }
    if f != 0.0 && g == 0.0 {
        return Err(reject_number(
            item,
            position,
            format!("float value {f} underflows the described REAL parameter"),
        ));
    }
    Ok(g)
}

fn reject_number(item: usize, position: usize, reason: String) -> CamelError {
    CamelError::ProcessorError(format!("Batch item {item}, parameter {position}: {reason}"))
}

fn json_kind_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Binds one batch item's values against the position targets.
///
/// `targets[position]` drives normalization; positions without a target
/// bind through the default JSON path. On a normalization failure the
/// returned error names the 0-based batch item index and parameter
/// position.
pub(crate) fn bind_batch_item<'q>(
    mut query: Query<'q, Any, AnyArguments>,
    values: &'q [serde_json::Value],
    targets: &[Option<BindTarget>],
    item: usize,
) -> Result<Query<'q, Any, AnyArguments>, CamelError> {
    for (position, value) in values.iter().enumerate() {
        query = match normalize_for_target(
            value,
            targets.get(position).copied().flatten(),
            item,
            position,
        )? {
            NormalizedBind::I16(v) => query.bind(v),
            NormalizedBind::I32(v) => query.bind(v),
            NormalizedBind::I64(v) => query.bind(v),
            NormalizedBind::F32(v) => query.bind(v),
            NormalizedBind::F64(v) => query.bind(v),
            NormalizedBind::Null(target) => match target {
                BindTarget::I16 => query.bind(Option::<i16>::None),
                BindTarget::I32 => query.bind(Option::<i32>::None),
                BindTarget::I64 => query.bind(Option::<i64>::None),
                BindTarget::F32 => query.bind(Option::<f32>::None),
                BindTarget::F64 => query.bind(Option::<f64>::None),
            },
            NormalizedBind::Default => bind_single_json_value(query, value),
        };
    }
    Ok(query)
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
    // ── Batch bind-kind normalization (bd rc-fef4z) ──────────────────

    mod batch_kinds {
        use super::*;
        use serde_json::json;

        fn norm_f64(v: serde_json::Value) -> Result<NormalizedBind, CamelError> {
            normalize_for_target(&v, Some(BindTarget::F64), 1, 0)
        }

        fn norm_i64(v: serde_json::Value) -> Result<NormalizedBind, CamelError> {
            normalize_for_target(&v, Some(BindTarget::I64), 1, 0)
        }

        fn err_text(result: Result<NormalizedBind, CamelError>) -> String {
            result.expect_err("expected typed error").to_string()
        }

        #[test]
        fn target_mapping_is_exact_per_wire_encoding() {
            let cases = [
                (AnyTypeInfoKind::SmallInt, Some(BindTarget::I16)),
                (AnyTypeInfoKind::Integer, Some(BindTarget::I32)),
                (AnyTypeInfoKind::BigInt, Some(BindTarget::I64)),
                (AnyTypeInfoKind::Real, Some(BindTarget::F32)),
                (AnyTypeInfoKind::Double, Some(BindTarget::F64)),
                (AnyTypeInfoKind::Text, None),
                (AnyTypeInfoKind::Bool, None),
                (AnyTypeInfoKind::Blob, None),
                (AnyTypeInfoKind::Null, None),
            ];
            for (kind, expected) in cases {
                assert_eq!(
                    bind_target_from_any_kind(kind),
                    expected,
                    "unexpected mapping for {kind:?}"
                );
            }
        }

        #[test]
        fn scan_lets_float_win_per_position() {
            let batch = [
                [json!(1), json!(2), json!("x")],
                [json!(3.5), json!(4), json!("y")],
            ];
            let rows: Vec<&[serde_json::Value]> = batch.iter().map(|r| r.as_slice()).collect();
            let targets = scan_bind_targets(&rows);
            assert_eq!(
                targets,
                vec![Some(BindTarget::F64), Some(BindTarget::I64), None]
            );
        }

        #[test]
        fn scan_without_numbers_defaults() {
            let batch = [[json!("a"), json!(true)], [json!("b"), json!(false)]];
            let rows: Vec<&[serde_json::Value]> = batch.iter().map(|r| r.as_slice()).collect();
            let targets = scan_bind_targets(&rows);
            assert_eq!(targets, vec![None, None]);
        }

        #[test]
        fn int_to_double_stays_exact_within_2_pow_53() {
            assert_eq!(
                norm_f64(json!(9007199254740992i64)).unwrap(),
                NormalizedBind::F64(9007199254740992.0)
            );
            assert_eq!(norm_f64(json!(20)).unwrap(), NormalizedBind::F64(20.0));
            assert_eq!(norm_f64(json!(-3)).unwrap(), NormalizedBind::F64(-3.0));
        }

        #[test]
        fn int_over_2_pow_53_is_rejected_for_float_targets() {
            let text = err_text(norm_f64(json!(9007199254740993i64)));
            assert!(text.contains("Batch item 1, parameter 0"), "got: {text}");
            assert!(text.contains("2^53"), "got: {text}");
            // i64::MIN exceeds 2^53 in magnitude and must be rejected too.
            let text = err_text(norm_f64(json!(i64::MIN)));
            assert!(text.contains("2^53"), "got: {text}");
        }

        #[test]
        fn int_to_smallint_integer_range_checks() {
            assert_eq!(
                normalize_for_target(&json!(30000), Some(BindTarget::I16), 0, 1).unwrap(),
                NormalizedBind::I16(30000)
            );
            assert_eq!(
                normalize_for_target(&json!(300000), Some(BindTarget::I32), 0, 1).unwrap(),
                NormalizedBind::I32(300000)
            );
            let text = err_text(normalize_for_target(
                &json!(300000),
                Some(BindTarget::I16),
                2,
                1,
            ));
            assert!(text.contains("SMALLINT"), "got: {text}");
            assert!(text.contains("Batch item 2, parameter 1"), "got: {text}");
            let text = err_text(normalize_for_target(
                &json!(i64::MAX),
                Some(BindTarget::I32),
                0,
                0,
            ));
            assert!(text.contains("INTEGER"), "got: {text}");
        }

        #[test]
        fn float_to_int_rounds_ties_even_like_postgres_rint() {
            assert_eq!(norm_i64(json!(2.5)).unwrap(), NormalizedBind::I64(2));
            assert_eq!(norm_i64(json!(3.5)).unwrap(), NormalizedBind::I64(4));
            assert_eq!(norm_i64(json!(-2.5)).unwrap(), NormalizedBind::I64(-2));
            assert_eq!(norm_i64(json!(0.5)).unwrap(), NormalizedBind::I64(0));
            // 6.5 ties to the even integer 6, like Postgres `rint`.
            assert_eq!(norm_i64(json!(6.5)).unwrap(), NormalizedBind::I64(6));
            assert_eq!(
                normalize_for_target(&json!(7.5), Some(BindTarget::I16), 0, 0).unwrap(),
                NormalizedBind::I16(8)
            );
        }

        #[test]
        fn float_to_int_out_of_range_is_rejected() {
            let text = err_text(norm_i64(json!(1e30)));
            assert!(text.contains("outside the described BIGINT"), "got: {text}");
            let text = err_text(normalize_for_target(
                &json!(1e10),
                Some(BindTarget::I16),
                0,
                0,
            ));
            assert!(text.contains("SMALLINT"), "got: {text}");
        }

        #[test]
        fn f32_overflow_and_underflow_are_rejected() {
            let text = err_text(normalize_for_target(
                &json!(1e40),
                Some(BindTarget::F32),
                0,
                0,
            ));
            assert!(text.contains("overflows the described REAL"), "got: {text}");
            let text = err_text(normalize_for_target(
                &json!(1e-50),
                Some(BindTarget::F32),
                0,
                0,
            ));
            assert!(
                text.contains("underflows the described REAL"),
                "got: {text}"
            );
            assert_eq!(
                normalize_for_target(&json!(6.5), Some(BindTarget::F32), 0, 0).unwrap(),
                NormalizedBind::F32(6.5)
            );
        }

        #[test]
        fn unsigned_beyond_i64_is_rejected_everywhere() {
            let huge = serde_json::Number::from(u64::MAX);
            for target in [
                BindTarget::I16,
                BindTarget::I32,
                BindTarget::I64,
                BindTarget::F32,
                BindTarget::F64,
            ] {
                let result = normalize_for_target(
                    &serde_json::Value::Number(huge.clone()),
                    Some(target),
                    3,
                    1,
                );
                let text = err_text(result);
                assert!(text.contains("Batch item 3, parameter 1"), "got: {text}");
            }
        }

        #[test]
        fn string_at_numeric_position_is_rejected() {
            let text = err_text(norm_f64(json!("12345678")));
            assert!(text.contains("string value"), "got: {text}");
            assert!(text.contains("numeric"), "got: {text}");
            let text = err_text(norm_i64(json!(true)));
            assert!(text.contains("bool value"), "got: {text}");
        }

        #[test]
        fn null_binds_typed_per_target() {
            assert_eq!(
                normalize_for_target(&json!(null), Some(BindTarget::I32), 0, 0).unwrap(),
                NormalizedBind::Null(BindTarget::I32)
            );
            assert_eq!(
                normalize_for_target(&json!(null), Some(BindTarget::F32), 0, 0).unwrap(),
                NormalizedBind::Null(BindTarget::F32)
            );
        }

        #[test]
        fn no_target_keeps_default_binding() {
            assert_eq!(
                normalize_for_target(&json!("abc"), None, 0, 0).unwrap(),
                NormalizedBind::Default
            );
            assert_eq!(
                normalize_for_target(&json!(5), None, 0, 0).unwrap(),
                NormalizedBind::Default
            );
        }

        #[test]
        fn bind_batch_item_uses_targets_per_position() {
            let values = vec![json!(20), json!(6.5), json!("x")];
            let targets = [Some(BindTarget::F64), Some(BindTarget::I64), None];
            let query = sqlx::query(sqlx::AssertSqlSafe("SELECT 1".to_string()));
            let bound = bind_batch_item(query, &values, &targets, 0);
            assert!(
                bound.is_ok(),
                "expected all values to bind, got {:?}",
                bound.err()
            );
        }
    }
}
