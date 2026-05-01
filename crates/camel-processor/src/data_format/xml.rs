use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;
use camel_api::xml_convert::{json_to_xml, xml_to_json};

pub struct XmlDataFormat;

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
            Body::Empty | Body::Bytes(_) => Err(CamelError::TypeConversionFailed(
                "XmlDataFormat::marshal only supports Body::Json and Body::Text".to_string(),
            )),
            Body::Xml(_) => Err(CamelError::TypeConversionFailed(
                "XmlDataFormat::marshal does not accept Body::Xml — use unmarshal to convert XML to JSON"
                    .to_string(),
            )),
        }
    }

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Json(_) => Ok(body),
            Body::Text(s) => {
                let v = xml_to_json(&s)?;
                Ok(Body::Json(v))
            }
            Body::Bytes(b) => {
                let s = String::from_utf8(b.to_vec()).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "cannot unmarshal Body::Bytes as XML: {e}"
                    ))
                })?;
                let v = xml_to_json(&s)?;
                Ok(Body::Json(v))
            }
            Body::Xml(s) => {
                let v = xml_to_json(&s)?;
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
        assert_eq!(XmlDataFormat.name(), "xml");
    }

    #[test]
    fn test_unmarshal_text_to_json() {
        let body = Body::Text("<root><child>value</child></root>".to_string());
        let result = XmlDataFormat.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => assert_eq!(v["root"]["child"], json!("value")),
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_bytes_to_json() {
        let body = Body::Bytes(Bytes::from_static(b"<root/>"));
        let result = XmlDataFormat.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => assert_eq!(v["root"], serde_json::Value::Null),
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_xml_body_to_json() {
        let body = Body::Xml("<root><a>1</a></root>".to_string());
        let result = XmlDataFormat.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => assert_eq!(v["root"]["a"], json!("1")),
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_invalid_xml() {
        let body = Body::Text("not xml".to_string());
        let result = XmlDataFormat.unmarshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_unmarshal_json_noop() {
        let body = Body::Json(json!({"x": 1}));
        let result = XmlDataFormat.unmarshal(body).unwrap();
        assert!(matches!(result, Body::Json(_)));
    }

    #[test]
    fn test_unmarshal_empty_returns_error() {
        let body = Body::Empty;
        let result = XmlDataFormat.unmarshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_marshal_json_to_text() {
        let body = Body::Json(json!({"root": {"name": "Alice"}}));
        let result = XmlDataFormat.marshal(body).unwrap();
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
        let result = XmlDataFormat.marshal(body).unwrap();
        assert_eq!(result, Body::Text("already text".to_string()));
    }

    #[test]
    fn test_marshal_empty_returns_error() {
        let body = Body::Empty;
        let result = XmlDataFormat.marshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_marshal_xml_body_rejected() {
        let body = Body::Xml("<root/>".to_string());
        let result = XmlDataFormat.marshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }
}
