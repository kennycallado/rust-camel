use crate::body::Body;
use crate::error::CamelError;
use bytes::Bytes;

/// Target type for body conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyType {
    Text,
    Json,
    Bytes,
    Xml,
    Empty,
}

/// Validate that a string is well-formed XML.
///
/// Requirements:
/// - Must not be empty or whitespace-only.
/// - Must contain at least one root element (not just a prolog/declaration).
/// - Must not contain multiple root elements.
/// - Must be parseable by libxml without errors.
fn validate_xml(s: &str) -> Result<(), CamelError> {
    if s.trim().is_empty() {
        return Err(CamelError::TypeConversionFailed(
            "invalid XML: document is empty".to_string(),
        ));
    }
    let parser = libxml::parser::Parser::default();
    let doc = parser
        .parse_string(s.as_bytes())
        .map_err(|e| CamelError::TypeConversionFailed(format!("invalid XML: {e}")))?;

    if doc.get_root_element().is_none() {
        return Err(CamelError::TypeConversionFailed(
            "invalid XML: missing root element".to_string(),
        ));
    }

    Ok(())
}

/// Convert a `Body` to the target `BodyType`.
///
/// `Body::Stream` is always an error — materialize with `into_bytes()` first.
/// Returns `CamelError::TypeConversionFailed` on any incompatible conversion.
pub fn convert(body: Body, target: BodyType) -> Result<Body, CamelError> {
    match (body, target) {
        // noop: same variant
        (b @ Body::Text(_), BodyType::Text) => Ok(b),
        (b @ Body::Json(_), BodyType::Json) => Ok(b),
        (b @ Body::Bytes(_), BodyType::Bytes) => Ok(b),
        (b @ Body::Xml(_), BodyType::Xml) => Ok(b),
        (Body::Empty, BodyType::Empty) => Ok(Body::Empty),

        // Text conversions
        (Body::Text(s), BodyType::Json) => {
            let v = serde_json::from_str(&s).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cannot convert Body::Text to Json: {e}"))
            })?;
            Ok(Body::Json(v))
        }
        (Body::Text(s), BodyType::Bytes) => Ok(Body::Bytes(Bytes::from(s.into_bytes()))),
        (Body::Text(s), BodyType::Xml) => {
            validate_xml(&s)?;
            Ok(Body::Xml(s))
        }
        (Body::Text(_), BodyType::Empty) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Text to Empty".to_string(),
        )),

        // Json conversions
        (Body::Json(serde_json::Value::String(s)), BodyType::Text) => Ok(Body::Text(s)),
        (Body::Json(v), BodyType::Text) => Ok(Body::Text(v.to_string())),
        (Body::Json(v), BodyType::Bytes) => {
            let b = serde_json::to_vec(&v).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cannot convert Body::Json to Bytes: {e}"))
            })?;
            Ok(Body::Bytes(Bytes::from(b)))
        }
        (Body::Json(_), BodyType::Xml) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Json to Xml: JSON to XML conversion is not supported".to_string(),
        )),
        (Body::Json(_), BodyType::Empty) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Json to Empty".to_string(),
        )),

        // Bytes conversions
        (Body::Bytes(b), BodyType::Text) => {
            let s = String::from_utf8(b.to_vec()).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "cannot convert Body::Bytes to Text: invalid UTF-8 sequence: {e}"
                ))
            })?;
            Ok(Body::Text(s))
        }
        (Body::Bytes(b), BodyType::Json) => {
            let s = String::from_utf8(b.to_vec()).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "cannot convert Body::Bytes to Json (UTF-8 error): {e}"
                ))
            })?;
            let v = serde_json::from_str(&s).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cannot convert Body::Bytes to Json: {e}"))
            })?;
            Ok(Body::Json(v))
        }
        (Body::Bytes(b), BodyType::Xml) => {
            let s = String::from_utf8(b.to_vec()).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "cannot convert Body::Bytes to Xml (UTF-8 error): {e}"
                ))
            })?;
            validate_xml(&s)?;
            Ok(Body::Xml(s))
        }
        (Body::Bytes(_), BodyType::Empty) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Bytes to Empty".to_string(),
        )),

        // Xml conversions
        (Body::Xml(s), BodyType::Text) => Ok(Body::Text(s)),
        (Body::Xml(s), BodyType::Bytes) => Ok(Body::Bytes(Bytes::from(s.into_bytes()))),
        (Body::Xml(_), BodyType::Json) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Xml to Json: XML to JSON conversion is not supported".to_string(),
        )),
        (Body::Xml(_), BodyType::Empty) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Xml to Empty".to_string(),
        )),

        // Empty conversions
        (Body::Empty, BodyType::Text) => Err(CamelError::TypeConversionFailed(
            "cannot convert Empty body to Text".to_string(),
        )),
        (Body::Empty, BodyType::Json) => Err(CamelError::TypeConversionFailed(
            "cannot convert Empty body to Json".to_string(),
        )),
        (Body::Empty, BodyType::Bytes) => Err(CamelError::TypeConversionFailed(
            "cannot convert Empty body to Bytes".to_string(),
        )),
        (Body::Empty, BodyType::Xml) => Err(CamelError::TypeConversionFailed(
            "cannot convert Empty body to Xml".to_string(),
        )),

        // Stream: always fails
        (Body::Stream(_), _) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Stream: materialize first with into_bytes()".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_to_json_valid() {
        let body = Body::Text(r#"{"a":1}"#.to_string());
        let result = convert(body, BodyType::Json).unwrap();
        assert_eq!(result, Body::Json(json!({"a": 1})));
    }

    #[test]
    fn text_to_json_invalid() {
        let body = Body::Text("not json".to_string());
        let result = convert(body, BodyType::Json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn json_to_text() {
        let body = Body::Json(json!({"a": 1}));
        let result = convert(body, BodyType::Text).unwrap();
        match result {
            Body::Text(s) => assert!(s.contains("\"a\"")),
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn json_to_bytes() {
        let body = Body::Json(json!({"x": 2}));
        let result = convert(body, BodyType::Bytes).unwrap();
        assert!(matches!(result, Body::Bytes(_)));
    }

    #[test]
    fn bytes_to_text_valid() {
        let body = Body::Bytes(Bytes::from_static(b"hello"));
        let result = convert(body, BodyType::Text).unwrap();
        assert_eq!(result, Body::Text("hello".to_string()));
    }

    #[test]
    fn bytes_to_text_invalid_utf8() {
        let body = Body::Bytes(Bytes::from_static(&[0xFF, 0xFE]));
        let result = convert(body, BodyType::Text);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn text_to_bytes() {
        let body = Body::Text("hi".to_string());
        let result = convert(body, BodyType::Bytes).unwrap();
        assert_eq!(result, Body::Bytes(Bytes::from_static(b"hi")));
    }

    #[test]
    fn empty_to_text_fails() {
        let result = convert(Body::Empty, BodyType::Text);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn empty_to_empty_noop() {
        let result = convert(Body::Empty, BodyType::Empty).unwrap();
        assert!(matches!(result, Body::Empty));
    }

    #[test]
    fn noop_same_type_text() {
        let body = Body::Text("x".to_string());
        let result = convert(body, BodyType::Text).unwrap();
        assert!(matches!(result, Body::Text(_)));
    }

    #[test]
    fn noop_same_type_json() {
        let body = Body::Json(json!(1));
        let result = convert(body, BodyType::Json).unwrap();
        assert!(matches!(result, Body::Json(_)));
    }

    #[test]
    fn noop_same_type_bytes() {
        let body = Body::Bytes(Bytes::from_static(b"x"));
        let result = convert(body, BodyType::Bytes).unwrap();
        assert!(matches!(result, Body::Bytes(_)));
    }

    #[test]
    fn stream_to_any_fails() {
        use crate::body::{StreamBody, StreamMetadata};
        use futures::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let stream = stream::iter(vec![Ok(Bytes::from_static(b"data"))]);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });
        let result = convert(body, BodyType::Text);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn bytes_to_json_valid() {
        let body = Body::Bytes(Bytes::from_static(b"{\"k\":1}"));
        let result = convert(body, BodyType::Json).unwrap();
        assert!(matches!(result, Body::Json(_)));
    }

    #[test]
    fn bytes_to_json_invalid_utf8() {
        let body = Body::Bytes(Bytes::from_static(&[0xFF, 0xFE]));
        let result = convert(body, BodyType::Json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn to_empty_always_fails() {
        assert!(matches!(
            convert(Body::Text("x".into()), BodyType::Empty),
            Err(CamelError::TypeConversionFailed(_))
        ));
        assert!(matches!(
            convert(Body::Json(serde_json::json!(1)), BodyType::Empty),
            Err(CamelError::TypeConversionFailed(_))
        ));
        assert!(matches!(
            convert(Body::Bytes(Bytes::from_static(b"x")), BodyType::Empty),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }

    // =============================================================================
    // XML Conversion Tests
    // =============================================================================

    #[test]
    fn noop_same_type_xml() {
        let body = Body::Xml("<root/>".to_string());
        let result = convert(body, BodyType::Xml).unwrap();
        assert!(matches!(result, Body::Xml(_)));
    }

    #[test]
    fn test_text_to_xml() {
        let xml = r#"<root><child>value</child></root>"#;
        let body = Body::Text(xml.to_string());
        let result = convert(body, BodyType::Xml).unwrap();
        match result {
            Body::Xml(s) => assert_eq!(s, xml),
            _ => panic!("expected Body::Xml"),
        }
    }

    #[test]
    fn test_xml_to_text() {
        let xml = r#"<root><child>value</child></root>"#;
        let body = Body::Xml(xml.to_string());
        let result = convert(body, BodyType::Text).unwrap();
        match result {
            Body::Text(s) => assert_eq!(s, xml),
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_bytes_to_xml() {
        let xml = r#"<root><child>value</child></root>"#;
        let body = Body::Bytes(Bytes::from(xml.as_bytes()));
        let result = convert(body, BodyType::Xml).unwrap();
        match result {
            Body::Xml(s) => assert_eq!(s, xml),
            _ => panic!("expected Body::Xml"),
        }
    }

    #[test]
    fn test_xml_to_bytes() {
        let xml = r#"<root><child>value</child></root>"#;
        let body = Body::Xml(xml.to_string());
        let result = convert(body, BodyType::Bytes).unwrap();
        match result {
            Body::Bytes(b) => assert_eq!(b.as_ref(), xml.as_bytes()),
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_invalid_xml_rejected() {
        let invalid_xml = "not valid xml <unclosed";
        let body = Body::Text(invalid_xml.to_string());
        let result = convert(body, BodyType::Xml);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_json_to_xml_unsupported() {
        let body = Body::Json(json!({"key": "value"}));
        let result = convert(body, BodyType::Xml);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
        if let Err(CamelError::TypeConversionFailed(msg)) = result {
            assert!(
                msg.contains("not supported"),
                "error message should mention 'not supported', got: {}",
                msg
            );
        }
    }

    #[test]
    fn test_xml_to_json_unsupported() {
        let body = Body::Xml("<root/>".to_string());
        let result = convert(body, BodyType::Json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
        if let Err(CamelError::TypeConversionFailed(msg)) = result {
            assert!(
                msg.contains("not supported"),
                "error message should mention 'not supported', got: {}",
                msg
            );
        }
    }

    #[test]
    fn test_empty_to_xml_fails() {
        let result = convert(Body::Empty, BodyType::Xml);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_xml_to_empty_fails() {
        let body = Body::Xml("<root/>".to_string());
        let result = convert(body, BodyType::Empty);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_bytes_to_xml_invalid_utf8() {
        let body = Body::Bytes(Bytes::from_static(&[0xFF, 0xFE]));
        let result = convert(body, BodyType::Xml);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_bytes_to_xml_invalid_xml() {
        let invalid = b"valid utf-8 but <invalid xml";
        let body = Body::Bytes(Bytes::from_static(invalid));
        let result = convert(body, BodyType::Xml);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    // =============================================================================
    // XML Validation Edge Case Tests
    // =============================================================================

    #[test]
    fn test_empty_string_rejected_as_xml() {
        let body = Body::Text("".to_string());
        let result = convert(body, BodyType::Xml);
        assert!(
            matches!(result, Err(CamelError::TypeConversionFailed(_))),
            "empty string should be rejected as XML"
        );
    }

    #[test]
    fn test_whitespace_only_rejected_as_xml() {
        let body = Body::Text("   \n\t  ".to_string());
        let result = convert(body, BodyType::Xml);
        assert!(
            matches!(result, Err(CamelError::TypeConversionFailed(_))),
            "whitespace-only string should be rejected as XML"
        );
    }

    #[test]
    fn test_prolog_only_rejected_as_xml() {
        // XML declaration without any root element
        let body = Body::Text(r#"<?xml version="1.0" encoding="UTF-8"?>"#.to_string());
        let result = convert(body, BodyType::Xml);
        assert!(
            matches!(result, Err(CamelError::TypeConversionFailed(_))),
            "XML prolog without root element should be rejected"
        );
    }

    #[test]
    fn test_multiple_root_elements_rejected() {
        let body = Body::Text("<root1/><root2/>".to_string());
        let result = convert(body, BodyType::Xml);
        assert!(
            matches!(result, Err(CamelError::TypeConversionFailed(_))),
            "XML with multiple root elements should be rejected"
        );
    }

    #[test]
    fn test_multiple_root_elements_with_children_rejected() {
        let body = Body::Text("<a><b/></a><c/>".to_string());
        let result = convert(body, BodyType::Xml);
        assert!(
            matches!(result, Err(CamelError::TypeConversionFailed(_))),
            "XML with multiple root elements (one with children) should be rejected"
        );
    }

    #[test]
    fn test_valid_xml_with_prolog_accepted() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><root><child>value</child></root>"#;
        let body = Body::Text(xml.to_string());
        let result = convert(body, BodyType::Xml);
        assert!(
            result.is_ok(),
            "XML with prolog and root element should be accepted"
        );
    }

    #[test]
    fn test_self_closing_root_accepted() {
        let body = Body::Text("<root/>".to_string());
        let result = convert(body, BodyType::Xml);
        assert!(
            result.is_ok(),
            "self-closing root element should be accepted"
        );
    }
}
