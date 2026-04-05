#![doc = include_str!("../README.md")]

use camel_api::Value;
use camel_api::body::Body;
use camel_api::exchange::Exchange;
use camel_language_api::{Expression, Language, LanguageError, Predicate};
use serde_json::Value as JsonValue;
use sxd_document::parser;
use sxd_xpath::{Context, Factory, Value as SxdValue};

pub struct XPathLanguage;

struct XPathExpression {
    query: String,
}

struct XPathPredicate {
    query: String,
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
        .map_err(|e| LanguageError::ParseError {
            expr: query.to_string(),
            reason: e.to_string(),
        })
        .and_then(|opt| {
            opt.ok_or_else(|| LanguageError::ParseError {
                expr: query.to_string(),
                reason: "empty XPath expression".into(),
            })
        })
}

fn run_query(query: &str, xml: &str) -> Result<JsonValue, LanguageError> {
    let package = parser::parse(xml).map_err(|e| {
        LanguageError::EvalError(format!("xml parse error for xpath '{query}': {e}"))
    })?;
    let doc = package.as_document();
    let xpath = compile_xpath(query)?;
    let context = Context::new();
    let result = xpath
        .evaluate(&context, doc.root())
        .map_err(|e| LanguageError::EvalError(format!("xpath query '{query}' failed: {e}")))?;

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

impl Expression for XPathExpression {
    fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        let xml = extract_xml(exchange)?;
        run_query(&self.query, &xml)
    }
}

impl Predicate for XPathPredicate {
    fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let xml = extract_xml(exchange)?;
        let result = run_query(&self.query, &xml)?;
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

impl Language for XPathLanguage {
    fn name(&self) -> &'static str {
        "xpath"
    }

    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
        compile_xpath(script)?;
        Ok(Box::new(XPathExpression {
            query: script.to_string(),
        }))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        compile_xpath(script)?;
        Ok(Box::new(XPathPredicate {
            query: script.to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::message::Message;

    fn exchange_with_xml(xml: &str) -> Exchange {
        Exchange::new(Message::new(Body::Xml(xml.to_string())))
    }

    fn exchange_with_text_body(text: &str) -> Exchange {
        Exchange::new(Message::new(Body::Text(text.to_string())))
    }

    fn empty_exchange() -> Exchange {
        Exchange::new(Message::default())
    }

    #[test]
    fn expression_simple_path() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("/root/name").unwrap();
        let ex = exchange_with_xml("<root><name>books</name></root>");
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::String("books".to_string()));
    }

    #[test]
    fn expression_nested_path() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("/root/inner/value").unwrap();
        let ex = exchange_with_xml("<root><inner><value>42</value></inner></root>");
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::String("42".to_string()));
    }

    #[test]
    fn expression_attribute_access() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("/root/item/@id").unwrap();
        let ex = exchange_with_xml("<root><item id=\"123\"/></root>");
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::String("123".to_string()));
    }

    #[test]
    fn expression_text_function() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("/root/name/text()").unwrap();
        let ex = exchange_with_xml("<root><name>hello</name></root>");
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::String("hello".to_string()));
    }

    #[test]
    fn expression_wildcard() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("/root/item").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item></root>");
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(
            result,
            JsonValue::Array(vec![
                JsonValue::String("a".to_string()),
                JsonValue::String("b".to_string()),
            ])
        );
    }

    #[test]
    fn expression_predicate_position() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("/root/item[2]").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item><item>c</item></root>");
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::String("b".to_string()));
    }

    #[test]
    fn expression_count_function() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("count(/root/item)").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item></root>");
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(
            result,
            JsonValue::Number(serde_json::Number::from_f64(2.0).unwrap())
        );
    }

    #[test]
    fn expression_text_body_with_valid_xml() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("/root/value").unwrap();
        let ex = exchange_with_text_body("<root><value>test</value></root>");
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::String("test".to_string()));
    }

    #[test]
    fn expression_text_body_with_invalid_xml() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("/root").unwrap();
        let ex = exchange_with_text_body("not xml at all");
        let result = expr.evaluate(&ex);
        assert!(result.is_err());
    }

    #[test]
    fn expression_empty_body_is_error() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("/root").unwrap();
        let ex = empty_exchange();
        let result = expr.evaluate(&ex);
        assert!(result.is_err());
    }

    #[test]
    fn expression_empty_result_is_null() {
        let lang = XPathLanguage;
        let expr = lang.create_expression("/root/missing").unwrap();
        let ex = exchange_with_xml("<root><name>test</name></root>");
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::Null);
    }

    #[test]
    fn expression_invalid_xpath_syntax() {
        let lang = XPathLanguage;
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

    #[test]
    fn predicate_non_empty_nodeset_is_true() {
        let lang = XPathLanguage;
        let pred = lang.create_predicate("/root/item").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item></root>");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn predicate_empty_result_is_false() {
        let lang = XPathLanguage;
        let pred = lang.create_predicate("/root/missing").unwrap();
        let ex = exchange_with_xml("<root><name>test</name></root>");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn predicate_boolean_expression() {
        let lang = XPathLanguage;
        let pred = lang.create_predicate("count(/root/item) > 2").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item><item>c</item></root>");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn predicate_numeric_comparison_false() {
        let lang = XPathLanguage;
        let pred = lang.create_predicate("count(/root/item) > 5").unwrap();
        let ex = exchange_with_xml("<root><item>a</item></root>");
        assert!(!pred.matches(&ex).unwrap());
    }
}
