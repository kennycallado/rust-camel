use camel_component_api::CamelError;
use sqlx::any::AnyArguments;
use sqlx::any::AnyRow;
use sqlx::query::Query;
use sqlx::{Any, Column, Row};
use tracing::warn;

/// Converts a database row to a JSON object.
///
/// Iterates columns and extracts values by trying types in order:
/// Option<i64>, Option<i32>, Option<f64>, Option<bool>, Option<String>.
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
