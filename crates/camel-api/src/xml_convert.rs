use crate::error::CamelError;
use quick_xml::Reader;
use quick_xml::events::Event;

/// Validate that the input is well-formed XML.
///
/// This performs syntax validation only.
pub fn validate_xml(input: &str) -> Result<(), CamelError> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut root_count = 0usize;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => {
                if depth == 0 {
                    root_count += 1;
                    if root_count > 1 {
                        return Err(CamelError::TypeConversionFailed(
                            "multiple root elements found".into(),
                        ));
                    }
                }
                depth += 1;
            }
            Ok(Event::Empty(_)) => {
                if depth == 0 {
                    root_count += 1;
                    if root_count > 1 {
                        return Err(CamelError::TypeConversionFailed(
                            "multiple root elements found".into(),
                        ));
                    }
                }
            }
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
            }
            Ok(Event::DocType(_)) => {
                return Err(CamelError::TypeConversionFailed(
                    "DOCTYPE is not allowed in XML body".into(),
                ));
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(CamelError::TypeConversionFailed(format!(
                    "invalid XML at position {}: {e}",
                    reader.error_position()
                )));
            }
            // PI events (<?target ...?>) are allowed — they cannot carry
            // external entity references. The XML declaration (<?xml ...?>)
            // is emitted as Event::Decl, not Event::PI.
            _ => {}
        }
        buf.clear();
    }

    if root_count == 0 {
        return Err(CamelError::TypeConversionFailed(
            "empty XML: no root element found".into(),
        ));
    }

    Ok(())
}

/// Convert an XML string to a JSON value.
///
/// Whitespace in text content is **trimmed** (leading/trailing) consistently with
/// Apache Camel XJ behavior. For example, `<name> Alice </name>` produces
/// `{"name": "Alice"}` — the surrounding spaces are removed. Indentation whitespace
/// between elements is also ignored via `trim_text(true)` on the reader.
///
/// # Errors
/// Returns `CamelError::TypeConversionFailed` if the input is not well-formed XML,
/// contains multiple root elements, or is empty.
pub fn xml_to_json(input: &str) -> Result<serde_json::Value, CamelError> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().trim_text(true);

    let mut stack: Vec<XmlNode> = Vec::new();
    let mut got_root = false;
    let mut result: Option<serde_json::Value> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if result.is_some() {
                    return Err(CamelError::TypeConversionFailed(
                        "multiple root elements found".into(),
                    ));
                }
                got_root = true;
                let name = local_name(&e);
                let attrs = parse_attrs(&e)?;
                stack.push(XmlNode {
                    name,
                    attrs,
                    children: serde_json::Map::new(),
                    text: String::new(),
                });
            }
            Ok(Event::Empty(e)) => {
                if result.is_some() {
                    return Err(CamelError::TypeConversionFailed(
                        "multiple root elements found".into(),
                    ));
                }
                got_root = true;
                let name = local_name(&e);
                let attrs = parse_attrs(&e)?;
                let value = if attrs.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::Object(attrs)
                };
                if let Some(parent) = stack.last_mut() {
                    insert_child(&mut parent.children, name, value);
                } else {
                    result = Some(serde_json::Value::Object(single_entry_map(name, value)));
                }
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().map_err(|err| {
                    CamelError::TypeConversionFailed(format!("cannot unescape XML text: {err}"))
                })?;
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&text);
                }
            }
            Ok(Event::CData(e)) => {
                let text = String::from_utf8_lossy(e.as_ref()).into_owned();
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&text);
                }
            }
            Ok(Event::End(_)) => {
                let node = stack.pop().ok_or_else(|| {
                    CamelError::TypeConversionFailed("unexpected closing tag".into())
                })?;
                let name = node.name.clone();
                let value = build_node_value(node);
                if let Some(parent) = stack.last_mut() {
                    insert_child(&mut parent.children, name, value);
                } else {
                    result = Some(serde_json::Value::Object(single_entry_map(name, value)));
                }
            }
            Ok(Event::Eof) => {
                if !got_root {
                    return Err(CamelError::TypeConversionFailed(
                        "empty XML: no root element found".into(),
                    ));
                }
                if let Some(res) = result {
                    return Ok(res);
                }
                break;
            }
            Err(e) => {
                return Err(CamelError::TypeConversionFailed(format!(
                    "invalid XML at position {}: {e}",
                    reader.error_position()
                )));
            }
            _ => {}
        }
    }

    Err(CamelError::TypeConversionFailed(
        "unexpected end of XML input".into(),
    ))
}

/// Validate that a string is a valid XML element/attribute name per the XML Name production.
///
/// Uses Unicode-aware checks: NameStartChar accepts any Unicode alphabetic character,
/// `_`, or `:`. NameChar accepts any Unicode alphanumeric, `_`, `-`, `.`, or `:`.
/// This is intentionally permissive — invalid names are ultimately rejected by
/// `quick-xml` when writing.
fn is_valid_xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' || c == ':' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == ':')
}

pub fn json_to_xml(value: &serde_json::Value) -> Result<String, CamelError> {
    let obj = value.as_object().ok_or_else(|| {
        CamelError::TypeConversionFailed(
            "cannot convert to XML: top-level value must be a JSON object".into(),
        )
    })?;

    // Filter out special keys (@attr, #text) to find actual element keys
    let element_keys: Vec<&String> = obj
        .keys()
        .filter(|k| !k.starts_with('@') && **k != "#text")
        .collect();

    if element_keys.is_empty() {
        return Err(CamelError::TypeConversionFailed(
            "cannot convert to XML: JSON object must contain exactly one root element".into(),
        ));
    }
    if element_keys.len() > 1 {
        return Err(CamelError::TypeConversionFailed(format!(
            "cannot convert to XML: expected exactly one root element, found {} ({})",
            element_keys.len(),
            element_keys
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let root_key = element_keys[0];
    if !is_valid_xml_name(root_key) {
        return Err(CamelError::TypeConversionFailed(format!(
            "invalid XML element name: {root_key:?}"
        )));
    }

    let child = &obj[root_key];
    let mut output = String::new();
    serialize_node(&mut output, root_key, child)?;
    Ok(output)
}

/// Convert any JSON value to its string representation for XML serialization.
fn value_as_str(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => val.to_string(),
    }
}

fn serialize_node(
    output: &mut String,
    tag: &str,
    value: &serde_json::Value,
) -> Result<(), CamelError> {
    if !is_valid_xml_name(tag) {
        return Err(CamelError::TypeConversionFailed(format!(
            "invalid XML element name: {tag:?}"
        )));
    }
    match value {
        serde_json::Value::Null => {
            output.push_str(&format!("<{tag}/>"));
        }
        serde_json::Value::String(s) => {
            output.push_str(&format!("<{tag}>{}</{tag}>", escape_xml_text(s)));
        }
        serde_json::Value::Number(n) => {
            output.push_str(&format!("<{tag}>{n}</{tag}>"));
        }
        serde_json::Value::Bool(b) => {
            output.push_str(&format!("<{tag}>{b}</{tag}>"));
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                serialize_node(output, tag, item)?;
            }
        }
        serde_json::Value::Object(map) => {
            let mut attrs = String::new();
            let mut children = String::new();
            let mut text = String::new();

            for (key, val) in map {
                if let Some(attr_name) = key.strip_prefix('@') {
                    if !is_valid_xml_name(attr_name) {
                        return Err(CamelError::TypeConversionFailed(format!(
                            "invalid XML attribute name: {attr_name:?}"
                        )));
                    }
                    attrs.push_str(&format!(
                        r#" {}="{}""#,
                        attr_name,
                        escape_xml_text(&value_as_str(val))
                    ));
                } else if key == "#text" {
                    text = escape_xml_text(&value_as_str(val));
                } else {
                    serialize_node(&mut children, key, val)?;
                }
            }

            if children.is_empty() && text.is_empty() {
                output.push_str(&format!("<{tag}{attrs}/>"));
            } else {
                output.push_str(&format!("<{tag}{attrs}>{text}{children}</{tag}>"));
            }
        }
    }
    Ok(())
}

fn escape_xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

struct XmlNode {
    name: String,
    attrs: serde_json::Map<String, serde_json::Value>,
    children: serde_json::Map<String, serde_json::Value>,
    text: String,
}

fn local_name(e: &quick_xml::events::BytesStart<'_>) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn parse_attrs(
    e: &quick_xml::events::BytesStart<'_>,
) -> Result<serde_json::Map<String, serde_json::Value>, CamelError> {
    let mut map = serde_json::Map::new();
    for attr_result in e.attributes() {
        let attr = attr_result.map_err(|err| {
            CamelError::TypeConversionFailed(format!("cannot parse attribute: {err}"))
        })?;

        // Skip namespace declarations (both xmlns and xmlns:prefix)
        let full_name = String::from_utf8_lossy(attr.key.as_ref());
        if full_name == "xmlns" || full_name.starts_with("xmlns:") {
            continue;
        }

        let key = format!(
            "@{}",
            String::from_utf8_lossy(attr.key.local_name().as_ref())
        );
        let val_str = String::from_utf8_lossy(&attr.value);
        let val = quick_xml::escape::unescape(&val_str).map_err(|err| {
            CamelError::TypeConversionFailed(format!("cannot unescape attribute value: {err}"))
        })?;
        map.insert(key, serde_json::Value::String(val.to_string()));
    }
    Ok(map)
}

fn build_node_value(node: XmlNode) -> serde_json::Value {
    let has_attrs = !node.attrs.is_empty();
    let has_children = !node.children.is_empty();
    let trimmed = node.text.trim();

    if has_children {
        let mut map = node.attrs;
        if !trimmed.is_empty() {
            map.insert(
                "#text".to_string(),
                serde_json::Value::String(trimmed.to_string()),
            );
        }
        for (k, v) in node.children {
            insert_child(&mut map, k, v);
        }
        serde_json::Value::Object(map)
    } else if has_attrs {
        let mut map = node.attrs;
        if !trimmed.is_empty() {
            map.insert(
                "#text".to_string(),
                serde_json::Value::String(trimmed.to_string()),
            );
        }
        serde_json::Value::Object(map)
    } else if trimmed.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(trimmed.to_string())
    }
}

fn insert_child(
    map: &mut serde_json::Map<String, serde_json::Value>,
    name: String,
    value: serde_json::Value,
) {
    match map.remove(&name) {
        None => {
            map.insert(name, value);
        }
        Some(serde_json::Value::Array(mut arr)) => {
            arr.push(value);
            map.insert(name, serde_json::Value::Array(arr));
        }
        Some(existing) => {
            map.insert(name, serde_json::Value::Array(vec![existing, value]));
        }
    }
}

fn single_entry_map(
    key: String,
    value: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert(key, value);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn simple_element() {
        let xml = "<root><name>Alice</name></root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"name": "Alice"}}));
    }

    #[test]
    fn nested_elements() {
        let xml = "<root><user><city>Madrid</city></user></root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"user": {"city": "Madrid"}}}));
    }

    #[test]
    fn repeated_siblings_become_array() {
        let xml = "<root><item>a</item><item>b</item></root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"item": ["a", "b"]}}));
    }

    #[test]
    fn single_sibling_is_scalar() {
        let xml = "<root><item>only</item></root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"item": "only"}}));
    }

    #[test]
    fn attributes_use_at_prefix() {
        let xml = r#"<root id="123"><name>Alice</name></root>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"@id": "123", "name": "Alice"}}));
    }

    #[test]
    fn text_with_attrs_uses_hash_text() {
        let xml = r#"<root id="1">hello</root>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"@id": "1", "#text": "hello"}}));
    }

    #[test]
    fn self_closing_no_attrs_is_null() {
        let xml = "<root><empty/></root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"empty": null}}));
    }

    #[test]
    fn self_closing_with_attrs_is_object() {
        let xml = r#"<root><link href="http://example.com"/></root>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(
            result,
            json!({"root": {"link": {"@href": "http://example.com"}}})
        );
    }

    #[test]
    fn text_with_children_uses_hash_text() {
        let xml = "<root>hello<child>world</child></root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(
            result,
            json!({"root": {"#text": "hello", "child": "world"}})
        );
    }

    #[test]
    fn repeated_siblings_with_attrs_become_array() {
        let xml = r#"<root><item id="1">a</item><item id="2">b</item></root>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(
            result,
            json!({"root": {"item": [{"@id": "1", "#text": "a"}, {"@id": "2", "#text": "b"}]}})
        );
    }

    #[test]
    fn parent_with_only_child_elements_no_hash_text() {
        let xml = "<person><name>John</name><age>30</age></person>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"person": {"name": "John", "age": "30"}}));
    }

    #[test]
    fn invalid_xml_returns_error() {
        let result = xml_to_json("not xml <unclosed");
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn empty_string_returns_error() {
        let result = xml_to_json("");
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn validate_xml_valid() {
        assert!(validate_xml("<root/>").is_ok());
    }

    #[test]
    fn validate_xml_rejects_doctype() {
        let result = validate_xml("<!DOCTYPE root><root/>");
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn validate_xml_rejects_multiple_roots() {
        let result = validate_xml("<a/><b/>");
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn validate_xml_rejects_empty() {
        let result = validate_xml("");
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn validate_xml_rejects_whitespace_only() {
        let result = validate_xml("   \n\t  ");
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn validate_xml_accepts_prolog() {
        assert!(validate_xml(r#"<?xml version=\"1.0\"?><root/>"#).is_ok());
    }

    #[test]
    fn validate_xml_rejects_prolog_only() {
        let result = validate_xml(r#"<?xml version=\"1.0\"?>"#);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn xml_prolog_accepted() {
        let xml = r#"<?xml version="1.0"?><root><a>1</a></root>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"a": "1"}}));
    }

    #[test]
    fn complex_nested_with_arrays_and_attrs() {
        let xml = r#"<order id="123">
            <item>coffee</item>
            <item>tea</item>
            <status active="true">pending</status>
        </order>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(
            result,
            json!({
                "order": {
                    "@id": "123",
                    "item": ["coffee", "tea"],
                    "status": {"@active": "true", "#text": "pending"}
                }
            })
        );
    }

    #[test]
    fn cdata_treated_as_text() {
        let xml = "<root><msg><![CDATA[hello <world>]]></msg></root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"msg": "hello <world>"}}));
    }

    #[test]
    fn comments_ignored() {
        let xml = "<root><!-- a comment --><a>1</a></root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"a": "1"}}));
    }

    #[test]
    fn whitespace_text_around_children_not_included() {
        let xml = "<root>\n  <a>1</a>\n</root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"a": "1"}}));
    }

    #[test]
    fn test_whitespace_trimmed() {
        // Documents that leading/trailing whitespace in text content is trimmed,
        // consistent with Apache Camel XJ behavior.
        let xml = "<name> Alice </name>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"name": "Alice"}));
    }

    #[test]
    fn xml_entity_escaping_decoded() {
        let xml = "<root><a>&amp;&lt;&gt;</a></root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"a": "&<>"}}));
    }

    #[test]
    fn attribute_entity_escaping_decoded() {
        let xml = r#"<root a="&amp;val"/>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"@a": "&val"}}));
    }

    #[test]
    fn self_closing_root() {
        let xml = "<root/>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": null}));
    }

    #[test]
    fn multiple_root_elements_returns_error() {
        let result = xml_to_json("<a/><b/>");
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn default_namespace_filtered() {
        let xml = r#"<root xmlns="http://example.com"><a>1</a></root>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"a": "1"}}));
    }

    #[test]
    fn prefixed_namespace_filtered() {
        let xml = r#"<root xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><a>1</a></root>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"a": "1"}}));
    }

    #[test]
    fn multiple_namespaces_filtered() {
        let xml = r#"<root xmlns="http://default.com" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xs="http://www.w3.org/2001/XMLSchema"><a>1</a></root>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"a": "1"}}));
    }

    #[test]
    fn mixed_namespace_and_regular_attrs() {
        let xml = r#"<root xmlns="http://example.com" id="123"><a>1</a></root>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"@id": "123", "a": "1"}}));
    }

    #[test]
    fn namespace_like_regular_attr_preserved() {
        let xml = r#"<root xmlnsAttribute="value"><a>1</a></root>"#;
        let result = xml_to_json(xml).unwrap();
        assert_eq!(
            result,
            json!({"root": {"@xmlnsAttribute": "value", "a": "1"}})
        );
    }

    #[test]
    fn prefixed_element_names_stripped() {
        let xml = "<ns:root><ns:a>1</ns:a></ns:root>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"root": {"a": "1"}}));
    }

    // --- json_to_xml tests (Task 2) ---

    #[test]
    fn json_to_xml_simple_object() {
        let json = json!({"root": {"name": "Alice"}});
        let result = json_to_xml(&json).unwrap();
        assert_eq!(result, "<root><name>Alice</name></root>");
    }

    #[test]
    fn json_to_xml_array() {
        let json = json!({"root": {"item": ["a", "b"]}});
        let result = json_to_xml(&json).unwrap();
        assert_eq!(result, "<root><item>a</item><item>b</item></root>");
    }

    #[test]
    fn json_to_xml_attributes() {
        let json = json!({"root": {"@id": "123", "name": "Alice"}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains(r#" id="123""#));
        assert!(result.contains("<name>Alice</name>"));
    }

    #[test]
    fn json_to_xml_null_element() {
        let json = json!({"root": {"empty": null}});
        let result = json_to_xml(&json).unwrap();
        assert_eq!(result, "<root><empty/></root>");
    }

    #[test]
    fn json_to_xml_hash_text() {
        let json = json!({"root": {"@id": "1", "#text": "hello"}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains(r#" id="1""#));
        assert!(result.contains(">hello</root>"));
    }

    #[test]
    fn json_to_xml_nested() {
        let json = json!({"root": {"user": {"city": "Madrid"}}});
        let result = json_to_xml(&json).unwrap();
        assert_eq!(result, "<root><user><city>Madrid</city></user></root>");
    }

    #[test]
    fn json_to_xml_non_object_returns_error() {
        let json = json!("just a string");
        let result = json_to_xml(&json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn json_to_xml_array_with_attrs() {
        let json =
            json!({"root": {"item": [{"@id": "1", "#text": "a"}, {"@id": "2", "#text": "b"}]}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains(r#" id="1""#));
        assert!(result.contains(r#" id="2""#));
        assert!(result.contains(">a<"));
        assert!(result.contains(">b<"));
    }

    #[test]
    fn json_to_xml_number_value() {
        let json = json!({"root": {"count": 42}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains("<count>42</count>"));
    }

    #[test]
    fn json_to_xml_bool_value() {
        let json = json!({"root": {"active": true}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains("<active>true</active>"));
    }

    #[test]
    fn json_to_xml_escapes_special_chars() {
        let json = json!({"root": {"a": "<&>\"'"}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains("&lt;&amp;&gt;&quot;&apos;"));
    }

    #[test]
    fn json_to_xml_empty_object_becomes_self_closing() {
        let json = json!({"root": {"empty": {}}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains("<empty/>"));
    }

    #[test]
    fn json_to_xml_number_as_attr() {
        let json = json!({"root": {"@count": 42, "#text": "hello"}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains(r#" count="42""#));
        assert!(result.contains(">hello</root>"));
    }

    #[test]
    fn json_to_xml_bool_as_attr() {
        let json = json!({"root": {"@active": true, "#text": "data"}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains(r#" active="true""#));
    }

    #[test]
    fn json_to_xml_number_as_text() {
        let json = json!({"root": {"@id": "1", "#text": 42}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains(r#" id="1""#));
        assert!(result.contains(">42</root>"));
    }

    #[test]
    fn json_to_xml_bool_as_text() {
        let json = json!({"root": {"#text": true}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains(">true</root>"));
    }

    // --- json_to_xml validation tests ---

    #[test]
    fn json_to_xml_multiple_roots_returns_error() {
        let json = json!({"root1": {"a": "1"}, "root2": {"b": "2"}});
        let result = json_to_xml(&json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exactly one root element"));
        assert!(err.contains("root1"));
        assert!(err.contains("root2"));
    }

    #[test]
    fn json_to_xml_empty_object_returns_error() {
        let json = json!({});
        let result = json_to_xml(&json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn json_to_xml_only_attrs_returns_error() {
        let json = json!({"@id": "1", "#text": "hello"});
        let result = json_to_xml(&json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn json_to_xml_invalid_element_name_space() {
        let json = json!({"my element": {"a": "1"}});
        let result = json_to_xml(&json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid XML element name"));
    }

    #[test]
    fn json_to_xml_invalid_element_name_starts_with_digit() {
        let json = json!({"123abc": {"a": "1"}});
        let result = json_to_xml(&json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn json_to_xml_invalid_element_name_special_chars() {
        let json = json!({"<script>": {"a": "1"}});
        let result = json_to_xml(&json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn json_to_xml_invalid_child_element_name() {
        let json = json!({"root": {"bad name": "value"}});
        let result = json_to_xml(&json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn json_to_xml_invalid_attribute_name() {
        let json = json!({"root": {"@bad attr": "value"}});
        let result = json_to_xml(&json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn json_to_xml_valid_names_with_hyphens_and_underscores() {
        let json = json!({"my-root": {"child_element": {"sub-item": "val"}}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains("<my-root>"));
        assert!(result.contains("<child_element>"));
        assert!(result.contains("<sub-item>"));
    }

    // --- Unicode element name tests ---

    #[test]
    fn xml_to_json_unicode_element_names() {
        // Unicode names are valid XML NameStartChar (alphabetic in Unicode)
        let xml = "<café><nombre>María</nombre></café>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"café": {"nombre": "María"}}));
    }

    #[test]
    fn xml_to_json_unicode_cjk_element_names() {
        let xml = "<日本語><値>テスト</値></日本語>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"日本語": {"値": "テスト"}}));
    }

    #[test]
    fn xml_to_json_unicode_spanish_element_names() {
        let xml = "<ñamapa><dirección>Calle Mayor</dirección></ñamapa>";
        let result = xml_to_json(xml).unwrap();
        assert_eq!(result, json!({"ñamapa": {"dirección": "Calle Mayor"}}));
    }

    #[test]
    fn json_to_xml_unicode_element_names() {
        let json = json!({"café": {"nombre": "María"}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains("<café>"));
        assert!(result.contains("<nombre>María</nombre>"));
    }

    #[test]
    fn json_to_xml_unicode_cjk_element_names() {
        let json = json!({"日本語": {"値": "テスト"}});
        let result = json_to_xml(&json).unwrap();
        assert!(result.contains("<日本語>"));
        assert!(result.contains("<値>テスト</値>"));
    }
}
