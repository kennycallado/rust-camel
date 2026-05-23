#![doc = include_str!("../README.md")]

use async_trait::async_trait;
use camel_language_api::{Body, Exchange, Value};
use camel_language_api::{Expression, Language, LanguageError, Predicate};
use serde_json::Value as JsonValue;
use sxd_document::parser;
use sxd_xpath::{Context, Factory, Value as SxdValue};
use tracing::{debug, warn};

// TODO(XPH-002): sxd-xpath is unmaintained; replacement planned.
// Input size is bounded to prevent resource exhaustion.

/// Configuration for XPath evaluation.
///
/// # Security note (XPH-002)
/// The underlying `sxd-xpath` crate is unmaintained. This config provides
/// a `max_input_bytes` guard to limit resource consumption.
///
/// # TODO(XPH-001): Namespace support
/// The XPath evaluation context does not yet support namespace declarations.
/// To evaluate expressions with XML namespaces (e.g. `/soap:Envelope/soap:Body`),
/// a namespace map (`HashMap<String, String>` mapping prefix → URI) must be
/// added to this config and registered with the `sxd_xpath::Context` before
/// evaluation.
#[derive(Debug, Clone)]
pub struct XPathConfig {
    /// Maximum allowed XML input size in bytes. Default: 1 MiB.
    pub max_input_bytes: Option<usize>,
}

impl Default for XPathConfig {
    fn default() -> Self {
        Self {
            max_input_bytes: Some(1_048_576), // 1 MiB
        }
    }
}

pub struct XPathLanguage {
    config: XPathConfig,
}

struct XPathExpression {
    query: String,
    config: XPathConfig,
}

struct XPathPredicate {
    query: String,
    config: XPathConfig,
}

fn extract_xml(exchange: &Exchange) -> Result<String, LanguageError> {
    match &exchange.input.body {
        Body::Xml(s) => Ok(s.clone()),
        other => other
            .clone()
            .try_into_xml()
            .map_err(|e| {
                LanguageError::EvalError(format!("body is not XML and cannot be coerced: {e}"))
            })
            .and_then(|b| match b {
                Body::Xml(s) => Ok(s),
                _ => Err(LanguageError::EvalError(
                    "body coercion did not produce XML".into(),
                )),
            }),
    }
}

fn compile_xpath(query: &str) -> Result<sxd_xpath::XPath, LanguageError> {
    let factory = Factory::new();
    factory
        .build(query)
        .map_err(|e| {
            warn!(error = %e, "xpath expression compile failed");
            LanguageError::ParseError {
                expr: query.to_string(),
                reason: e.to_string(),
            }
        })
        .and_then(|opt| {
            opt.ok_or_else(|| {
                warn!("xpath expression compile failed");
                LanguageError::ParseError {
                    expr: query.to_string(),
                    reason: "empty XPath expression".into(),
                }
            })
        })
}

fn run_query(query: &str, xml: &str, config: &XPathConfig) -> Result<JsonValue, LanguageError> {
    if let Some(max) = config.max_input_bytes
        && xml.len() > max
    {
        return Err(LanguageError::EvalError(
            "input exceeds maximum allowed size".into(),
        ));
    }
    let package = parser::parse(xml).map_err(|_| {
        // sxd parse errors can embed document-derived content (e.g. MismatchedTag
        // includes tag names from the exchange body which may be sensitive).
        // Return a generic message; do NOT include the raw error in logs or errors.
        warn!("xpath: body XML could not be parsed");
        LanguageError::EvalError("xml parse error: body is not valid XML".to_string())
    })?;
    let doc = package.as_document();
    let xpath = compile_xpath(query)?;
    // TODO(XPH-001): Namespace declarations are not yet supported. The context
    // should be populated with namespace prefix → URI mappings from XPathConfig
    // before calling xpath.evaluate().
    let context = Context::new();
    let result = xpath.evaluate(&context, doc.root()).map_err(|_| {
        // sxd_xpath eval errors describe query structure issues (unknown variable/function,
        // type mismatch). No document-derived values are embedded, but we follow the
        // same conservative pattern: generic message, no raw external error strings.
        warn!("xpath: expression evaluation failed");
        LanguageError::EvalError(
            "xpath query failed: expression could not be evaluated".to_string(),
        )
    })?;

    Ok(match result {
        SxdValue::Nodeset(ns) => {
            let nodes: Vec<_> = ns.document_order();
            match nodes.len() {
                0 => JsonValue::Null,
                1 => JsonValue::String(nodes[0].string_value()),
                _ => JsonValue::Array(
                    nodes
                        .into_iter()
                        .map(|n| JsonValue::String(n.string_value()))
                        .collect(),
                ),
            }
        }
        SxdValue::Boolean(b) => JsonValue::Bool(b),
        SxdValue::Number(n) => serde_json::Number::from_f64(n)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        SxdValue::String(s) => JsonValue::String(s),
    })
}

#[async_trait]
impl Expression for XPathExpression {
    async fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        let xml = extract_xml(exchange)?;
        run_query(&self.query, &xml, &self.config)
    }
}

#[async_trait]
impl Predicate for XPathPredicate {
    async fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let xml = extract_xml(exchange)?;
        let result = run_query(&self.query, &xml, &self.config)?;
        Ok(match &result {
            JsonValue::Null => false,
            JsonValue::Bool(b) => *b,
            JsonValue::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
            JsonValue::String(s) => !s.is_empty(),
            JsonValue::Array(arr) => !arr.is_empty(),
            _ => true,
        })
    }
}

impl Default for XPathLanguage {
    fn default() -> Self {
        Self::new()
    }
}

impl XPathLanguage {
    /// Create an XPathLanguage::new() with the default configuration (1 MiB input limit).
    pub fn new() -> Self {
        Self::with_config(XPathConfig::default())
    }

    /// Create an XPathLanguage::new() with a custom configuration.
    pub fn with_config(config: XPathConfig) -> Self {
        Self { config }
    }
}

impl Language for XPathLanguage {
    fn name(&self) -> &'static str {
        "xpath"
    }

    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
        compile_xpath(script)?;
        debug!("xpath expression compiled");
        Ok(Box::new(XPathExpression {
            query: script.to_string(),
            config: self.config.clone(),
        }))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        compile_xpath(script)?;
        debug!("xpath expression compiled");
        Ok(Box::new(XPathPredicate {
            query: script.to_string(),
            config: self.config.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_language_api::Message;

    async fn exchange_with_xml(xml: &str) -> Exchange {
        Exchange::new(Message::new(Body::Xml(xml.to_string())))
    }

    async fn exchange_with_text_body(text: &str) -> Exchange {
        Exchange::new(Message::new(Body::Text(text.to_string())))
    }

    async fn empty_exchange() -> Exchange {
        Exchange::new(Message::default())
    }

    #[tokio::test]
    async fn expression_simple_path() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root/name").unwrap();
        let ex = exchange_with_xml("<root><name>books</name></root>").await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("books".to_string()));
    }

    #[tokio::test]
    async fn expression_nested_path() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root/inner/value").unwrap();
        let ex = exchange_with_xml("<root><inner><value>42</value></inner></root>").await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("42".to_string()));
    }

    #[tokio::test]
    async fn expression_attribute_access() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root/item/@id").unwrap();
        let ex = exchange_with_xml("<root><item id=\"123\"/></root>").await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("123".to_string()));
    }

    #[tokio::test]
    async fn expression_text_function() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root/name/text()").unwrap();
        let ex = exchange_with_xml("<root><name>hello</name></root>").await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("hello".to_string()));
    }

    #[tokio::test]
    async fn expression_wildcard() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root/item").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item></root>").await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(
            result,
            JsonValue::Array(vec![
                JsonValue::String("a".to_string()),
                JsonValue::String("b".to_string()),
            ])
        );
    }

    #[tokio::test]
    async fn expression_predicate_position() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root/item[2]").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item><item>c</item></root>").await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("b".to_string()));
    }

    #[tokio::test]
    async fn expression_count_function() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("count(/root/item)").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item></root>").await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(
            result,
            JsonValue::Number(serde_json::Number::from_f64(2.0).unwrap())
        );
    }

    #[tokio::test]
    async fn expression_text_body_with_valid_xml() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root/value").unwrap();
        let ex = exchange_with_text_body("<root><value>test</value></root>").await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("test".to_string()));
    }

    #[tokio::test]
    async fn expression_text_body_with_invalid_xml() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root").unwrap();
        let ex = exchange_with_text_body("not xml at all").await;
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn expression_empty_body_is_error() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root").unwrap();
        let ex = empty_exchange().await;
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn expression_empty_result_is_null() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root/missing").unwrap();
        let ex = exchange_with_xml("<root><name>test</name></root>").await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::Null);
    }

    #[tokio::test]
    async fn expression_invalid_xpath_syntax() {
        let lang = XPathLanguage::new();
        let result = lang.create_expression("//[invalid");
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected ParseError"),
        };
        match err {
            LanguageError::ParseError { expr, reason } => {
                assert!(!expr.is_empty());
                assert!(!reason.is_empty());
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn predicate_non_empty_nodeset_is_true() {
        let lang = XPathLanguage::new();
        let pred = lang.create_predicate("/root/item").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item></root>").await;
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_empty_result_is_false() {
        let lang = XPathLanguage::new();
        let pred = lang.create_predicate("/root/missing").unwrap();
        let ex = exchange_with_xml("<root><name>test</name></root>").await;
        assert!(!pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_boolean_expression() {
        let lang = XPathLanguage::new();
        let pred = lang.create_predicate("count(/root/item) > 2").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item><item>c</item></root>").await;
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_numeric_comparison_false() {
        let lang = XPathLanguage::new();
        let pred = lang.create_predicate("count(/root/item) > 5").unwrap();
        let ex = exchange_with_xml("<root><item>a</item></root>").await;
        assert!(!pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn expression_rejects_oversized_input() {
        let lang = XPathLanguage::with_config(XPathConfig {
            max_input_bytes: Some(100),
        });
        let expr = lang.create_expression("/root").unwrap();
        let big_xml = format!("<root>{}</root>", "x".repeat(200));
        let ex = exchange_with_xml(&big_xml).await;
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            LanguageError::EvalError(msg) => {
                assert!(msg.contains("input exceeds maximum allowed size"));
            }
            other => panic!("expected EvalError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn predicate_rejects_oversized_input() {
        let lang = XPathLanguage::with_config(XPathConfig {
            max_input_bytes: Some(100),
        });
        let pred = lang.create_predicate("/root").unwrap();
        let big_xml = format!("<root>{}</root>", "x".repeat(200));
        let ex = exchange_with_xml(&big_xml).await;
        let result = pred.matches(&ex).await;
        assert!(result.is_err());
    }
}
