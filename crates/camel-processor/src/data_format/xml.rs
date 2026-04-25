use camel_api::body::Body;
use camel_api::body_converter::{BodyType, convert};
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;

pub struct XmlDataFormat;

impl DataFormat for XmlDataFormat {
    fn name(&self) -> &str {
        "xml"
    }

    fn marshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Xml(_) => convert(body, BodyType::Text),
            Body::Text(_) => Ok(body),
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot marshal Body::Stream — add 'stream_cache' or 'convert_body_to' before this step".to_string(),
            )),
            Body::Empty | Body::Bytes(_) | Body::Json(_) => Err(CamelError::TypeConversionFailed(
                "XmlDataFormat::marshal only supports Body::Xml and Body::Text".to_string(),
            )),
        }
    }

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Xml(_) => Ok(body),
            Body::Text(_) | Body::Bytes(_) => convert(body, BodyType::Xml),
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot unmarshal Body::Stream directly — use UnmarshalService which auto-materializes"
                    .to_string(),
            )),
            Body::Empty | Body::Json(_) => Err(CamelError::TypeConversionFailed(
                "XmlDataFormat::unmarshal only supports Body::Xml, Body::Text, and Body::Bytes"
                    .to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn test_name() {
        assert_eq!(XmlDataFormat.name(), "xml");
    }

    #[test]
    fn test_unmarshal_text_to_xml() {
        let body = Body::Text("<root><child>value</child></root>".to_string());
        let result = XmlDataFormat.unmarshal(body).unwrap();
        assert!(matches!(result, Body::Xml(_)));
    }

    #[test]
    fn test_unmarshal_bytes_to_xml() {
        let body = Body::Bytes(Bytes::from_static(b"<root/>"));
        let result = XmlDataFormat.unmarshal(body).unwrap();
        assert!(matches!(result, Body::Xml(_)));
    }

    #[test]
    fn test_unmarshal_invalid_xml() {
        let body = Body::Text("not xml".to_string());
        let result = XmlDataFormat.unmarshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_unmarshal_xml_noop() {
        let body = Body::Xml("<root/>".to_string());
        let result = XmlDataFormat.unmarshal(body).unwrap();
        assert!(matches!(result, Body::Xml(_)));
    }

    #[test]
    fn test_unmarshal_unsupported_variant() {
        let body = Body::Empty;
        let result = XmlDataFormat.unmarshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_marshal_xml_to_text() {
        let body = Body::Xml("<root>content</root>".to_string());
        let result = XmlDataFormat.marshal(body).unwrap();
        assert_eq!(result, Body::Text("<root>content</root>".to_string()));
    }

    #[test]
    fn test_marshal_text_noop() {
        let body = Body::Text("already text".to_string());
        let result = XmlDataFormat.marshal(body).unwrap();
        assert_eq!(result, Body::Text("already text".to_string()));
    }

    #[test]
    fn test_marshal_unsupported_variant() {
        let body = Body::Empty;
        let result = XmlDataFormat.marshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }
}
