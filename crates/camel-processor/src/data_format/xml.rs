use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;
use camel_api::xml_convert::{json_to_xml, xml_to_json_with_depth_limit};
use serde::Deserialize;

fn default_max_depth() -> usize {
    camel_api::xml_convert::DEFAULT_MAX_XML_DEPTH
}

/// Configuration for [`XmlDataFormat`]. All fields have hardened defaults;
/// setting a non-default value is the per-item explicit choice per ADR-0033.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XmlConfig {
    /// Maximum XML nesting depth accepted by `unmarshal` (DoS cap).
    /// Default 100. Inert during `marshal`.
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
}

impl Default for XmlConfig {
    fn default() -> Self {
        Self {
            max_depth: default_max_depth(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct XmlDataFormat {
    config: XmlConfig,
}

impl XmlDataFormat {
    pub fn new(config: XmlConfig) -> Self {
        Self { config }
    }
}

impl DataFormat for XmlDataFormat {
    fn name(&self) -> &str {
        "xml"
    }

    fn marshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Json(v) => {
                let xml = json_to_xml(&v)?;
                Ok(Body::Text(xml))
            }
            Body::Text(_) => Ok(body),
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot marshal Body::Stream — add 'stream_cache' or 'convert_body_to' before this step".to_string(),
            )),
            Body::Bytes(_) => Err(CamelError::TypeConversionFailed(
                "XmlDataFormat::marshal only supports Body::Json, Body::Text, and Body::Empty"
                    .to_string(),
            )),
            Body::Xml(_) => Err(CamelError::TypeConversionFailed(
                "XmlDataFormat::marshal does not accept Body::Xml — use unmarshal to convert XML to JSON"
                    .to_string(),
            )),
            // Empty-body operations (e.g. REST 204) must not fail marshalling;
            // matches JsonDataFormat, which passes Body::Empty through.
            Body::Empty => Ok(Body::Empty),
            _ => Err(CamelError::TypeConversionFailed(
                "XmlDataFormat::marshal does not support this body type".to_string(),
            )),
        }
    }

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Json(_) => Ok(body),
            Body::Text(s) => {
                let v = xml_to_json_with_depth_limit(&s, self.config.max_depth)?;
                Ok(Body::Json(v))
            }
            Body::Bytes(b) => {
                let s = String::from_utf8(b.to_vec()).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "cannot unmarshal Body::Bytes as XML: {e}"
                    ))
                })?;
                let v = xml_to_json_with_depth_limit(&s, self.config.max_depth)?;
                Ok(Body::Json(v))
            }
            Body::Xml(s) => {
                let v = xml_to_json_with_depth_limit(&s, self.config.max_depth)?;
                Ok(Body::Json(v))
            }
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot unmarshal Body::Stream directly — use UnmarshalService which auto-materializes"
                    .to_string(),
            )),
            Body::Empty => Err(CamelError::TypeConversionFailed(
                "XmlDataFormat::unmarshal only supports Body::Json, Body::Text, Body::Bytes, and Body::Xml"
                    .to_string(),
            )),
            _ => Err(CamelError::TypeConversionFailed(
                "XmlDataFormat::unmarshal does not support this body type".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use serde_json::json;

    #[test]
    fn test_name() {
        assert_eq!(XmlDataFormat::default().name(), "xml");
    }

    #[test]
    fn test_unmarshal_text_to_json() {
        let body = Body::Text("<root><child>value</child></root>".to_string());
        let result = XmlDataFormat::default().unmarshal(body).unwrap();
        match result {
            Body::Json(v) => assert_eq!(v["root"]["child"], json!("value")),
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_bytes_to_json() {
        let body = Body::Bytes(Bytes::from_static(b"<root/>"));
        let result = XmlDataFormat::default().unmarshal(body).unwrap();
        match result {
            Body::Json(v) => assert_eq!(v["root"], serde_json::Value::Null),
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_xml_body_to_json() {
        let body = Body::Xml("<root><a>1</a></root>".to_string());
        let result = XmlDataFormat::default().unmarshal(body).unwrap();
        match result {
            Body::Json(v) => assert_eq!(v["root"]["a"], json!("1")),
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_invalid_xml() {
        let body = Body::Text("not xml".to_string());
        let result = XmlDataFormat::default().unmarshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_unmarshal_json_noop() {
        let body = Body::Json(json!({"x": 1}));
        let result = XmlDataFormat::default().unmarshal(body).unwrap();
        assert!(matches!(result, Body::Json(_)));
    }

    #[test]
    fn test_unmarshal_empty_returns_error() {
        let body = Body::Empty;
        let result = XmlDataFormat::default().unmarshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_marshal_json_to_text() {
        let body = Body::Json(json!({"root": {"name": "Alice"}}));
        let result = XmlDataFormat::default().marshal(body).unwrap();
        match result {
            Body::Text(s) => {
                assert!(s.contains("<root>"));
                assert!(s.contains("<name>Alice</name>"));
            }
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_marshal_text_noop() {
        let body = Body::Text("already text".to_string());
        let result = XmlDataFormat::default().marshal(body).unwrap();
        assert_eq!(result, Body::Text("already text".to_string()));
    }

    #[test]
    fn marshal_empty_is_noop() {
        let result = XmlDataFormat::default().marshal(Body::Empty).unwrap();
        assert_eq!(result, Body::Empty);
    }

    #[test]
    fn unmarshal_empty_fails_closed() {
        let result = XmlDataFormat::default().unmarshal(Body::Empty);
        match result {
            Err(CamelError::TypeConversionFailed(msg)) => {
                assert!(
                    msg.contains("XmlDataFormat::unmarshal"),
                    "message should name the format: {msg}"
                );
            }
            other => panic!("expected TypeConversionFailed, got {other:?}"),
        }
    }

    #[test]
    fn unmarshal_truncated_fails() {
        let result = XmlDataFormat::default().unmarshal(Body::Text("<user><name>".to_string()));
        match result {
            Err(CamelError::TypeConversionFailed(msg)) => {
                assert!(
                    msg.contains("unexpected end of XML input"),
                    "message should name truncated input: {msg}"
                );
            }
            other => panic!("expected TypeConversionFailed, got {other:?}"),
        }
    }

    #[test]
    fn marshal_output_has_no_declaration() {
        let body = Body::Json(json!({"user": {"name": "kenny"}}));
        let result = XmlDataFormat::default().marshal(body).unwrap();
        assert_eq!(
            result,
            Body::Text("<user><name>kenny</name></user>".to_string())
        );
    }

    #[test]
    fn marshal_multi_key_object_fails() {
        let body = Body::Json(json!({"a": 1, "b": 2}));
        match XmlDataFormat::default().marshal(body) {
            Err(CamelError::TypeConversionFailed(msg)) => {
                assert!(
                    msg.contains("exactly one root element"),
                    "message should explain root requirement: {msg}"
                );
            }
            other => panic!("expected TypeConversionFailed, got {other:?}"),
        }
    }

    #[test]
    fn marshal_top_level_array_fails() {
        let body = Body::Json(json!([{"a": 1}]));
        let result = XmlDataFormat::default().marshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn marshal_top_level_scalar_fails() {
        let body = Body::Json(json!("hello"));
        let result = XmlDataFormat::default().marshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn unmarshal_leaf_text_is_string() {
        let result = XmlDataFormat::default()
            .unmarshal(Body::Text("<n>42</n>".to_string()))
            .unwrap();
        assert_eq!(result, Body::Json(json!({"n": "42"})));
    }

    #[test]
    fn unmarshal_drops_xmlns_declarations() {
        let xml = r#"<r xmlns="urn:x" xmlns:p="urn:p"><child>v</child></r>"#;
        let result = XmlDataFormat::default()
            .unmarshal(Body::Text(xml.to_string()))
            .unwrap();
        assert_eq!(result, Body::Json(json!({"r": {"child": "v"}})));
    }

    #[test]
    fn unmarshal_empty_element_is_null() {
        let result = XmlDataFormat::default()
            .unmarshal(Body::Text("<r><empty/></r>".to_string()))
            .unwrap();
        assert_eq!(result, Body::Json(json!({"r": {"empty": null}})));
    }

    #[test]
    fn unmarshal_doctype_entity_ref_fails_closed() {
        let xml = r#"<!DOCTYPE foo [<!ENTITY x "BOOM">]><foo>&x;</foo>"#;
        match XmlDataFormat::default().unmarshal(Body::Text(xml.to_string())) {
            Err(CamelError::TypeConversionFailed(msg)) => {
                assert!(
                    msg.contains("unrecognized entity"),
                    "custom entity should fail closed: {msg}"
                );
            }
            other => panic!("expected TypeConversionFailed, got {other:?}"),
        }
    }

    #[test]
    fn unmarshal_doctype_without_entity_ok() {
        let result = XmlDataFormat::default()
            .unmarshal(Body::Text("<!DOCTYPE foo><foo>v</foo>".to_string()))
            .unwrap();
        assert_eq!(result, Body::Json(json!({"foo": "v"})));
    }

    #[test]
    fn test_marshal_xml_body_rejected() {
        let body = Body::Xml("<root/>".to_string());
        let result = XmlDataFormat::default().marshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_unmarshal_with_raised_max_depth() {
        let mut xml = String::new();
        for _ in 0..150 {
            xml.push_str("<a>");
        }
        xml.push('1');
        for _ in 0..150 {
            xml.push_str("</a>");
        }
        // Default depth (100) rejects 150 nesting; raised cap accepts it.
        let body = Body::Text(xml);
        let df = XmlDataFormat::new(XmlConfig { max_depth: 200 });
        let result = df.unmarshal(body);
        assert!(
            result.is_ok(),
            "should accept depth 150 under raised cap 200"
        );
    }
}
