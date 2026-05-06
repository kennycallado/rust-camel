use crate::bindings::camel::plugin::types::{
    WasmBody, WasmError, WasmExchange, WasmMessage, WasmPattern,
};

impl WasmBody {
    pub fn empty() -> Self {
        WasmBody::Empty
    }

    pub fn text(s: impl Into<String>) -> Self {
        WasmBody::Text(s.into())
    }

    pub fn bytes(b: impl Into<Vec<u8>>) -> Self {
        WasmBody::Bytes(b.into())
    }

    pub fn json(s: impl Into<String>) -> Self {
        WasmBody::Json(s.into())
    }

    pub fn xml(s: impl Into<String>) -> Self {
        WasmBody::Xml(s.into())
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            WasmBody::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            WasmBody::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_json(&self) -> Option<&str> {
        match self {
            WasmBody::Json(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_xml(&self) -> Option<&str> {
        match self {
            WasmBody::Xml(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, WasmBody::Empty)
    }

    pub fn is_text(&self) -> bool {
        matches!(self, WasmBody::Text(_))
    }

    pub fn is_bytes(&self) -> bool {
        matches!(self, WasmBody::Bytes(_))
    }

    pub fn is_json(&self) -> bool {
        matches!(self, WasmBody::Json(_))
    }

    pub fn is_xml(&self) -> bool {
        matches!(self, WasmBody::Xml(_))
    }

    pub fn into_text(self) -> Option<String> {
        match self {
            WasmBody::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn into_bytes(self) -> Option<Vec<u8>> {
        match self {
            WasmBody::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn into_json(self) -> Option<String> {
        match self {
            WasmBody::Json(s) => Some(s),
            _ => None,
        }
    }

    pub fn into_xml(self) -> Option<String> {
        match self {
            WasmBody::Xml(s) => Some(s),
            _ => None,
        }
    }
}

impl Default for WasmBody {
    fn default() -> Self {
        WasmBody::Empty
    }
}

impl WasmMessage {
    pub fn new(body: WasmBody) -> Self {
        WasmMessage {
            headers: Vec::new(),
            body,
        }
    }

    pub fn with_body(mut self, body: WasmBody) -> Self {
        self.body = body;
        self
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    pub fn get_header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_header(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        if let Some(entry) = self.headers.iter_mut().find(|(k, _)| *k == key) {
            entry.1 = value.into();
        } else {
            self.headers.push((key, value.into()));
        }
    }

    pub fn remove_header(&mut self, key: &str) {
        self.headers.retain(|(k, _)| k != key);
    }
}

impl Default for WasmMessage {
    fn default() -> Self {
        WasmMessage {
            headers: Vec::new(),
            body: WasmBody::Empty,
        }
    }
}

impl WasmExchange {
    pub fn new(input: WasmMessage) -> Self {
        WasmExchange {
            input,
            output: None,
            properties: Vec::new(),
            pattern: WasmPattern::InOnly,
            correlation_id: String::new(),
        }
    }

    pub fn new_in_out(input: WasmMessage) -> Self {
        WasmExchange {
            input,
            output: None,
            properties: Vec::new(),
            pattern: WasmPattern::InOut,
            correlation_id: String::new(),
        }
    }

    pub fn body(&self) -> &WasmBody {
        &self.input.body
    }

    pub fn body_mut(&mut self) -> &mut WasmBody {
        &mut self.input.body
    }

    pub fn with_output(mut self, body: WasmBody) -> Self {
        self.output = Some(WasmMessage::new(body));
        self
    }

    pub fn with_output_message(mut self, message: WasmMessage) -> Self {
        self.output = Some(message);
        self
    }

    pub fn get_property(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_property(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        if let Some(entry) = self.properties.iter_mut().find(|(k, _)| *k == key) {
            entry.1 = value.into();
        } else {
            self.properties.push((key, value.into()));
        }
    }

    pub fn remove_property(&mut self, key: &str) {
        self.properties.retain(|(k, _)| k != key);
    }

    pub fn set_body(&mut self, body: WasmBody) {
        self.input.body = body;
    }

    pub fn set_output_body(&mut self, body: WasmBody) {
        self.output = Some(WasmMessage::new(body));
    }

    pub fn output_body(&self) -> Option<&WasmBody> {
        self.output.as_ref().map(|m| &m.body)
    }
}

impl Default for WasmExchange {
    fn default() -> Self {
        WasmExchange {
            input: WasmMessage::default(),
            output: None,
            properties: Vec::new(),
            pattern: WasmPattern::InOnly,
            correlation_id: String::new(),
        }
    }
}

impl WasmPattern {
    pub fn is_in_only(&self) -> bool {
        matches!(self, WasmPattern::InOnly)
    }

    pub fn is_in_out(&self) -> bool {
        matches!(self, WasmPattern::InOut)
    }
}

impl WasmError {
    pub fn processor(msg: impl Into<String>) -> Self {
        WasmError::ProcessorError(msg.into())
    }

    pub fn type_conversion(msg: impl Into<String>) -> Self {
        WasmError::TypeConversion(msg.into())
    }

    pub fn io(msg: impl Into<String>) -> Self {
        WasmError::Io(msg.into())
    }

    pub fn timeout() -> Self {
        WasmError::Timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_body_constructors() {
        assert!(WasmBody::empty().is_empty());
        assert!(WasmBody::text("hello").is_text());
        assert!(WasmBody::bytes(vec![1, 2, 3]).is_bytes());
        assert!(WasmBody::json("{}").is_json());
        assert!(WasmBody::xml("<root/>").is_xml());
    }

    #[test]
    fn test_wasm_body_accessors() {
        let body = WasmBody::text("hello");
        assert_eq!(body.as_text(), Some("hello"));
        assert_eq!(body.as_bytes(), None);

        let body = WasmBody::bytes(vec![1, 2, 3]);
        assert_eq!(body.as_bytes(), Some(&[1u8, 2, 3][..]));

        let body = WasmBody::json("{\"a\":1}");
        assert_eq!(body.as_json(), Some("{\"a\":1}"));

        let body = WasmBody::xml("<a/>");
        assert_eq!(body.as_xml(), Some("<a/>"));

        assert!(WasmBody::empty().as_text().is_none());
    }

    #[test]
    fn test_wasm_body_into() {
        assert_eq!(WasmBody::text("hi").into_text(), Some("hi".to_string()));
        assert_eq!(WasmBody::bytes(vec![1]).into_bytes(), Some(vec![1]));
        assert_eq!(WasmBody::json("{}").into_json(), Some("{}".to_string()));
        assert_eq!(WasmBody::xml("<a/>").into_xml(), Some("<a/>".to_string()));
        assert!(WasmBody::empty().into_text().is_none());
    }

    #[test]
    fn test_wasm_body_default() {
        assert!(WasmBody::default().is_empty());
    }

    #[test]
    fn test_wasm_message_new() {
        let msg = WasmMessage::new(WasmBody::text("data"));
        assert!(msg.headers.is_empty());
        assert!(msg.body.is_text());
    }

    #[test]
    fn test_wasm_message_default() {
        let msg = WasmMessage::default();
        assert!(msg.headers.is_empty());
        assert!(msg.body.is_empty());
    }

    #[test]
    fn test_wasm_message_with_body() {
        let msg = WasmMessage::default().with_body(WasmBody::text("replaced"));
        assert_eq!(msg.body.as_text(), Some("replaced"));
    }

    #[test]
    fn test_wasm_message_headers() {
        let msg = WasmMessage::default()
            .with_header("content-type", "text/plain")
            .with_header("x-custom", "value");

        assert_eq!(msg.get_header("content-type"), Some("text/plain"));
        assert_eq!(msg.get_header("x-custom"), Some("value"));
        assert_eq!(msg.get_header("missing"), None);
    }

    #[test]
    fn test_wasm_message_set_header_updates_existing() {
        let mut msg = WasmMessage::default();
        msg.set_header("key", "old");
        msg.set_header("key", "new");
        assert_eq!(msg.get_header("key"), Some("new"));
        assert_eq!(msg.headers.len(), 1);
    }

    #[test]
    fn test_wasm_message_remove_header() {
        let mut msg = WasmMessage::default();
        msg.set_header("a", "1");
        msg.set_header("b", "2");
        msg.remove_header("a");
        assert_eq!(msg.get_header("a"), None);
        assert_eq!(msg.get_header("b"), Some("2"));
    }

    #[test]
    fn test_wasm_exchange_new() {
        let ex = WasmExchange::new(WasmMessage::new(WasmBody::text("input")));
        assert!(ex.output.is_none());
        assert!(ex.properties.is_empty());
        assert!(ex.pattern.is_in_only());
        assert!(ex.correlation_id.is_empty());
    }

    #[test]
    fn test_wasm_exchange_new_in_out() {
        let ex = WasmExchange::new_in_out(WasmMessage::default());
        assert!(ex.pattern.is_in_out());
    }

    #[test]
    fn test_wasm_exchange_default() {
        let ex = WasmExchange::default();
        assert!(ex.body().is_empty());
        assert!(ex.output.is_none());
    }

    #[test]
    fn test_wasm_exchange_body_shortcut() {
        let ex = WasmExchange::new(WasmMessage::new(WasmBody::text("hello")));
        assert_eq!(ex.body().as_text(), Some("hello"));
    }

    #[test]
    fn test_wasm_exchange_with_output() {
        let ex = WasmExchange::default().with_output(WasmBody::text("response"));
        assert!(ex.output.is_some());
        assert!(ex.output_body().unwrap().as_text() == Some("response"));
    }

    #[test]
    fn test_wasm_exchange_properties() {
        let mut ex = WasmExchange::default();
        ex.set_property("key", "value");
        assert_eq!(ex.get_property("key"), Some("value"));

        ex.set_property("key", "updated");
        assert_eq!(ex.get_property("key"), Some("updated"));
        assert_eq!(ex.properties.len(), 1);

        ex.remove_property("key");
        assert_eq!(ex.get_property("key"), None);
    }

    #[test]
    fn test_wasm_exchange_set_body() {
        let mut ex = WasmExchange::default();
        ex.set_body(WasmBody::json("{}"));
        assert!(ex.body().is_json());
    }

    #[test]
    fn test_wasm_exchange_set_output_body() {
        let mut ex = WasmExchange::default();
        ex.set_output_body(WasmBody::xml("<r/>"));
        assert_eq!(ex.output_body().unwrap().as_xml(), Some("<r/>"));
    }

    #[test]
    fn test_wasm_pattern_predicates() {
        assert!(WasmPattern::InOnly.is_in_only());
        assert!(!WasmPattern::InOnly.is_in_out());
        assert!(WasmPattern::InOut.is_in_out());
        assert!(!WasmPattern::InOut.is_in_only());
    }

    #[test]
    fn test_wasm_error_constructors() {
        let e = WasmError::processor("p");
        assert!(matches!(e, WasmError::ProcessorError(s) if s == "p"));

        let e = WasmError::type_conversion("tc");
        assert!(matches!(e, WasmError::TypeConversion(s) if s == "tc"));

        let e = WasmError::io("io");
        assert!(matches!(e, WasmError::Io(s) if s == "io"));

        let e = WasmError::timeout();
        assert!(matches!(e, WasmError::Timeout));
    }

    #[test]
    fn test_round_trip_transform() {
        let ex = WasmExchange::new(WasmMessage::new(WasmBody::text("input")));
        let result = WasmExchange {
            input: WasmMessage {
                body: WasmBody::text(ex.body().as_text().unwrap().to_uppercase()),
                ..ex.input
            },
            ..ex
        };
        assert_eq!(result.body().as_text(), Some("INPUT"));
    }
}
