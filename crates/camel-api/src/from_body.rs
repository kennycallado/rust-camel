//! `FromBody` trait and built-in implementations.
//!
//! Provides typed deserialization from a [`Body`] variant.
//! For custom types that implement [`serde::de::DeserializeOwned`],
//! use the [`impl_from_body_via_serde!`] macro.

use bytes::Bytes;
use serde_json::Value;

use crate::body::Body;
use crate::error::CamelError;

/// Convert a [`Body`] into a typed value.
///
/// Built-in implementations are provided for `String`, `Vec<u8>`, `Bytes`,
/// and `serde_json::Value`.
///
/// For custom types, implement this trait manually or use
/// [`impl_from_body_via_serde!`].
pub trait FromBody: Sized {
    fn from_body(body: &Body) -> Result<Self, CamelError>;
}

// ---------------------------------------------------------------------------
// String
// ---------------------------------------------------------------------------

impl FromBody for String {
    fn from_body(body: &Body) -> Result<Self, CamelError> {
        match body {
            Body::Text(s) => Ok(s.clone()),
            // Unwrap bare string literals to avoid surprising "abc" -> "\"abc\""
            Body::Json(Value::String(s)) => Ok(s.clone()),
            // Other JSON variants: serialize to string
            Body::Json(v) => Ok(v.to_string()),
            Body::Bytes(b) => String::from_utf8(b.to_vec()).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "cannot convert Body::Bytes to String: invalid UTF-8: {e}"
                ))
            }),
            Body::Xml(s) => Ok(s.clone()),
            Body::Empty | Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot convert empty or stream body to String".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Vec<u8>
// ---------------------------------------------------------------------------

impl FromBody for Vec<u8> {
    fn from_body(body: &Body) -> Result<Self, CamelError> {
        match body {
            Body::Text(s) => Ok(s.clone().into_bytes()),
            Body::Json(v) => serde_json::to_vec(v).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "cannot serialize Body::Json to Vec<u8>: {e}"
                ))
            }),
            Body::Bytes(b) => Ok(b.to_vec()),
            Body::Xml(s) => Ok(s.clone().into_bytes()),
            Body::Empty | Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot convert empty or stream body to Vec<u8>".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Bytes
// ---------------------------------------------------------------------------

impl FromBody for Bytes {
    fn from_body(body: &Body) -> Result<Self, CamelError> {
        match body {
            Body::Text(s) => Ok(Bytes::from(s.clone().into_bytes())),
            Body::Json(v) => {
                let b = serde_json::to_vec(v).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "cannot serialize Body::Json to Bytes: {e}"
                    ))
                })?;
                Ok(Bytes::from(b))
            }
            Body::Bytes(b) => Ok(b.clone()),
            Body::Xml(s) => Ok(Bytes::from(s.clone().into_bytes())),
            Body::Empty | Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot convert empty or stream body to Bytes".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// serde_json::Value
// ---------------------------------------------------------------------------

impl FromBody for Value {
    fn from_body(body: &Body) -> Result<Self, CamelError> {
        match body {
            Body::Json(v) => Ok(v.clone()),
            Body::Text(s) => serde_json::from_str(s).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cannot parse Body::Text as JSON: {e}"))
            }),
            Body::Bytes(b) => serde_json::from_slice(b).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cannot parse Body::Bytes as JSON: {e}"))
            }),
            Body::Xml(_) | Body::Empty | Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot convert Xml, Empty or Stream body to serde_json::Value".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Macro for custom serde types
// ---------------------------------------------------------------------------

/// Implements [`FromBody`] for a type that implements [`serde::de::DeserializeOwned`].
///
/// # Example
/// ```rust
/// use camel_api::{impl_from_body_via_serde, FromBody};
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Order { id: u64 }
///
/// impl_from_body_via_serde!(Order);
/// ```
#[macro_export]
macro_rules! impl_from_body_via_serde {
    ($t:ty) => {
        impl $crate::FromBody for $t {
            fn from_body(body: &$crate::body::Body) -> Result<Self, $crate::error::CamelError> {
                match body {
                    $crate::body::Body::Json(v) => serde_json::from_value(v.clone()).map_err(|e| {
                        $crate::error::CamelError::TypeConversionFailed(e.to_string())
                    }),
                    $crate::body::Body::Text(s) => serde_json::from_str(s).map_err(|e| {
                        $crate::error::CamelError::TypeConversionFailed(e.to_string())
                    }),
                    $crate::body::Body::Bytes(b) => serde_json::from_slice(b).map_err(|e| {
                        $crate::error::CamelError::TypeConversionFailed(e.to_string())
                    }),
                    _ => Err($crate::error::CamelError::TypeConversionFailed(format!(
                        "impl_from_body_via_serde: unsupported body variant for {}",
                        std::any::type_name::<$t>()
                    ))),
                }
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use serde::Deserialize;
    use serde_json::json;

    // --- String ---

    #[test]
    fn string_from_text() {
        let body = Body::Text("hello".into());
        assert_eq!(String::from_body(&body).unwrap(), "hello");
    }

    #[test]
    fn string_from_json_string_literal() {
        // Body::Json(Value::String) must NOT produce extra quotes
        let body = Body::Json(json!("hello"));
        assert_eq!(String::from_body(&body).unwrap(), "hello");
    }

    #[test]
    fn string_from_json_number() {
        let body = Body::Json(json!(42));
        assert_eq!(String::from_body(&body).unwrap(), "42");
    }

    #[test]
    fn string_from_json_object() {
        let body = Body::Json(json!({"a": 1}));
        let s = String::from_body(&body).unwrap();
        assert!(s.contains("\"a\""));
    }

    #[test]
    fn string_from_bytes_valid_utf8() {
        let body = Body::Bytes(Bytes::from_static(b"world"));
        assert_eq!(String::from_body(&body).unwrap(), "world");
    }

    #[test]
    fn string_from_bytes_invalid_utf8() {
        let body = Body::Bytes(Bytes::from_static(&[0xFF, 0xFE]));
        assert!(matches!(
            String::from_body(&body),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }

    #[test]
    fn string_from_xml() {
        let body = Body::Xml("<root/>".into());
        assert_eq!(String::from_body(&body).unwrap(), "<root/>");
    }

    #[test]
    fn string_from_empty_fails() {
        assert!(matches!(
            String::from_body(&Body::Empty),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }

    // --- Vec<u8> ---

    #[test]
    fn vec_u8_from_text() {
        let body = Body::Text("hi".into());
        assert_eq!(Vec::<u8>::from_body(&body).unwrap(), b"hi");
    }

    #[test]
    fn vec_u8_from_json() {
        let body = Body::Json(json!({"k": 1}));
        let v = Vec::<u8>::from_body(&body).unwrap();
        let s = String::from_utf8(v).unwrap();
        assert!(s.contains("\"k\""));
    }

    #[test]
    fn vec_u8_from_bytes() {
        let body = Body::Bytes(Bytes::from_static(b"data"));
        assert_eq!(Vec::<u8>::from_body(&body).unwrap(), b"data");
    }

    #[test]
    fn vec_u8_from_xml() {
        let body = Body::Xml("<r/>".into());
        assert_eq!(Vec::<u8>::from_body(&body).unwrap(), b"<r/>");
    }

    #[test]
    fn vec_u8_from_empty_fails() {
        assert!(matches!(
            Vec::<u8>::from_body(&Body::Empty),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }

    // --- Bytes ---

    #[test]
    fn bytes_from_text() {
        let body = Body::Text("hi".into());
        assert_eq!(Bytes::from_body(&body).unwrap(), Bytes::from_static(b"hi"));
    }

    #[test]
    fn bytes_from_json() {
        let body = Body::Json(json!(1));
        let b = Bytes::from_body(&body).unwrap();
        assert_eq!(&b[..], b"1");
    }

    #[test]
    fn bytes_from_bytes() {
        let body = Body::Bytes(Bytes::from_static(b"raw"));
        assert_eq!(Bytes::from_body(&body).unwrap(), Bytes::from_static(b"raw"));
    }

    #[test]
    fn bytes_from_xml() {
        let body = Body::Xml("<x/>".into());
        assert_eq!(
            Bytes::from_body(&body).unwrap(),
            Bytes::from_static(b"<x/>")
        );
    }

    #[test]
    fn bytes_from_empty_fails() {
        assert!(matches!(
            Bytes::from_body(&Body::Empty),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }

    // --- serde_json::Value ---

    #[test]
    fn value_from_json() {
        let body = Body::Json(json!({"x": 2}));
        assert_eq!(Value::from_body(&body).unwrap(), json!({"x": 2}));
    }

    #[test]
    fn value_from_text_valid_json() {
        let body = Body::Text(r#"{"a":1}"#.into());
        let v = Value::from_body(&body).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn value_from_text_invalid_json() {
        let body = Body::Text("not json".into());
        assert!(matches!(
            Value::from_body(&body),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }

    #[test]
    fn value_from_bytes_valid_json() {
        let body = Body::Bytes(Bytes::from_static(b"{\"z\":3}"));
        let v = Value::from_body(&body).unwrap();
        assert_eq!(v["z"], 3);
    }

    #[test]
    fn value_from_xml_fails() {
        let body = Body::Xml("<root/>".into());
        assert!(matches!(
            Value::from_body(&body),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }

    #[test]
    fn value_from_empty_fails() {
        assert!(matches!(
            Value::from_body(&Body::Empty),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }

    // --- impl_from_body_via_serde! ---

    #[derive(Debug, PartialEq, Deserialize)]
    struct Order {
        id: u64,
        amount: f64,
    }

    crate::impl_from_body_via_serde!(Order);

    #[test]
    fn serde_macro_from_json() {
        let body = Body::Json(json!({"id": 1, "amount": 9.99}));
        let order = Order::from_body(&body).unwrap();
        assert_eq!(
            order,
            Order {
                id: 1,
                amount: 9.99
            }
        );
    }

    #[test]
    fn serde_macro_from_text() {
        let body = Body::Text(r#"{"id": 2, "amount": 5.0}"#.into());
        let order = Order::from_body(&body).unwrap();
        assert_eq!(order, Order { id: 2, amount: 5.0 });
    }

    #[test]
    fn serde_macro_from_bytes() {
        let body = Body::Bytes(Bytes::from_static(br#"{"id": 3, "amount": 1.0}"#));
        let order = Order::from_body(&body).unwrap();
        assert_eq!(order, Order { id: 3, amount: 1.0 });
    }

    #[test]
    fn serde_macro_from_xml_fails() {
        let body = Body::Xml("<root/>".into());
        assert!(matches!(
            Order::from_body(&body),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }

    #[test]
    fn serde_macro_invalid_json_fails() {
        let body = Body::Json(json!({"wrong_field": "x"}));
        assert!(matches!(
            Order::from_body(&body),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }
}
