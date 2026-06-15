//! Vector helpers for extracting `Vec<f32>` from SurrealDB JSON values.
//!
//! SurrealDB stores vectors as JSON arrays of floats. These helpers extract
//! them from either:
//! - A named field within a JSON object (`extract_vector_from_json`)
//! - A raw JSON value that is itself an array (`extract_vector_raw`)

use serde_json::Value;

use crate::error::SurrealDbError;

/// Extract a `Vec<f32>` from a named field inside a JSON object.
///
/// Expects `value` to be a JSON object with `field_name` containing an
/// array of numbers (integers or floats convertible to f32).
pub fn extract_vector_from_json(
    value: &Value,
    field_name: &str,
) -> Result<Vec<f32>, SurrealDbError> {
    let arr = value
        .get(field_name)
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            SurrealDbError::InvalidVector(format!(
                "field '{}' is missing or not an array",
                field_name
            ))
        })?;

    arr.iter()
        .map(|v| {
            v.as_f64().map(|f| f as f32).ok_or_else(|| {
                SurrealDbError::InvalidVector(format!("non-numeric element in vector array: {v}"))
            })
        })
        .collect()
}

/// Extract a `Vec<f32>` from a raw JSON value that must itself be an array.
pub fn extract_vector_raw(value: &Value) -> Result<Vec<f32>, SurrealDbError> {
    let arr = value.as_array().ok_or_else(|| {
        SurrealDbError::InvalidVector("expected JSON array, got something else".into())
    })?;

    arr.iter()
        .map(|v| {
            v.as_f64().map(|f| f as f32).ok_or_else(|| {
                SurrealDbError::InvalidVector(format!("non-numeric element in vector array: {v}"))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    // =========================================================================
    // extract_vector_from_json tests (4)
    // =========================================================================

    #[test]
    fn from_json_object_field() {
        let value = json!({"embedding": [0.1, 0.2, 0.3]});
        let vec = super::extract_vector_from_json(&value, "embedding")
            .expect("should extract embedding field");
        assert_eq!(vec, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn from_json_object_missing_field() {
        let value = json!({"name": "alice"});
        let err = super::extract_vector_from_json(&value, "embedding").unwrap_err();
        assert!(err.to_string().contains("vector"));
    }

    #[test]
    fn from_json_object_wrong_type() {
        let value = json!({"embedding": "not an array"});
        let err = super::extract_vector_from_json(&value, "embedding").unwrap_err();
        assert!(err.to_string().contains("vector"));
    }

    #[test]
    fn from_json_object_non_float_elements() {
        let value = json!({"embedding": [1, 2, "three"]});
        let err = super::extract_vector_from_json(&value, "embedding").unwrap_err();
        assert!(err.to_string().contains("vector"));
    }

    // =========================================================================
    // extract_vector_raw tests (3)
    // =========================================================================

    #[test]
    fn from_raw_array() {
        let value = json!([0.5, 0.6, 0.7]);
        let vec = super::extract_vector_raw(&value).expect("should extract raw array");
        assert_eq!(vec, vec![0.5, 0.6, 0.7]);
    }

    #[test]
    fn from_raw_non_array() {
        let value = json!("string");
        let err = super::extract_vector_raw(&value).unwrap_err();
        assert!(err.to_string().contains("vector"));
    }

    #[test]
    fn from_raw_non_float_elements() {
        let value = json!([1, 2, "bad"]);
        let err = super::extract_vector_raw(&value).unwrap_err();
        assert!(err.to_string().contains("vector"));
    }
}
