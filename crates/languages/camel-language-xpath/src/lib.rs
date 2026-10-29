#![doc = include_str!("../README.md")]

use async_trait::async_trait;
use camel_api::ExpressionErrorClass;
use camel_language_api::{Body, Exchange, Value};
use camel_language_api::{Expression, Language, LanguageError, Predicate};
use serde_json::Value as JsonValue;
use sxd_document::parser;
use sxd_xpath::{Context, Factory, Value as SxdValue};
use tracing::debug;

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
    xpath: sxd_xpath::XPath,
    config: XPathConfig,
}

struct XPathPredicate {
    xpath: sxd_xpath::XPath,
    config: XPathConfig,
}

// SAFETY: Audited against sxd-xpath 0.4.2. `sxd_xpath::XPath` is
// `Box<dyn sxd_xpath::expression::Expression + 'static>`. The inner
// `Expression` trait is `&self`-only (see sxd-xpath `expression.rs`),
// and concrete types in production code contain no `Cell`/`RefCell`/etc.
// (the only `Rc<RefCell<…>>` in the crate lives behind `#[cfg(test)]`).
// So `&XPathExpression` / `&XPathPredicate` can be shared across threads
// without data races. `Send` is implied by the same reasoning plus the
// absence of self-references in the value.
// sxd-xpath is pinned to =0.4.2 in Cargo.toml; a version bump requires
// re-auditing expression.rs for interior mutability.
unsafe impl Send for XPathExpression {}
unsafe impl Sync for XPathExpression {}
unsafe impl Send for XPathPredicate {}
unsafe impl Sync for XPathPredicate {}

fn extract_xml(exchange: &Exchange) -> Result<String, LanguageError> {
    match &exchange.input.body {
        Body::Xml(s) => Ok(s.clone()),
        other => other
            .clone()
            .try_into_xml()
            .map_err(|_| LanguageError::EvalFailure {
                class: ExpressionErrorClass::Conversion,
                position: None,
                detail: Some("body is not XML and cannot be coerced".into()),
            })
            .and_then(|b| match b {
                Body::Xml(s) => Ok(s),
                _ => Err(LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Conversion,
                    position: None,
                    detail: Some("body coercion did not produce XML".into()),
                }),
            }),
    }
}

#[cfg(test)]
thread_local! {
    static COMPILE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn compile_xpath(query: &str) -> Result<sxd_xpath::XPath, LanguageError> {
    #[cfg(test)]
    {
        COMPILE_COUNT.with(|c| c.set(c.get() + 1));
    }
    let factory = Factory::new();
    factory
        .build(query)
        .map_err(|e| {
            debug!(error = %e, "xpath expression compile failed");
            LanguageError::ParseError {
                expr: query.to_string(),
                reason: e.to_string(),
            }
        })
        .and_then(|opt| {
            opt.ok_or_else(|| {
                debug!("xpath expression compile failed");
                LanguageError::ParseError {
                    expr: query.to_string(),
                    reason: "empty XPath expression".into(),
                }
            })
        })
}

/// Project an XPath number into its JSON representation.
///
/// XPath numbers are IEEE-754 doubles. Non-finite results (`+Inf`, `-Inf`,
/// `NaN`) are refused with an [`ExpressionErrorClass::Arithmetic`] failure
/// because JSON has no representation for them. Finite integral values are
/// projected to the narrowest exact JSON integer form:
///
/// - `[-2^63, 2^63)` → `i64`. Exact because every integral `f64` strictly
///   below `2^63` is at most `2^63 - 1024`.
/// - `[2^63, 2^64)` → `u64`. Exact because every integral `f64` below `2^64`
///   is at most `2^64 - 2048`.
/// - Everything else → `f64` float form. Fractional values and integral
///   values outside the two half-open intervals above stay floats.
///
/// The `n >= 2^63` guard on the `u64` branch is mandatory: without it a large
/// negative integral (for example `-2^64`) would enter the branch and `as u64`
/// would saturate to `0`, which is lossy.
///
/// `-0.0` projects to the integer `0` because JSON has no negative-zero
/// integer form.
///
/// The float arm's `Number::from_f64` cannot return `None` for finite input;
/// for totality its `None` case maps to the same non-finite refusal instead of
/// panicking.
fn number_to_json(n: f64) -> Result<JsonValue, LanguageError> {
    let non_finite_error = |value: f64| {
        let label = if value.is_nan() {
            "NaN"
        } else if value.is_sign_negative() {
            "-Inf"
        } else {
            "+Inf"
        };
        LanguageError::EvalFailure {
            class: ExpressionErrorClass::Arithmetic,
            position: None,
            detail: Some(format!("xpath number result is not finite: {label}")),
        }
    };
    if !n.is_finite() {
        return Err(non_finite_error(n));
    }
    if n.fract() == 0.0 && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&n)
    {
        return Ok(JsonValue::Number(serde_json::Number::from(n as i64)));
    }
    if n.fract() == 0.0 && (9_223_372_036_854_775_808.0..18_446_744_073_709_551_616.0).contains(&n)
    {
        return Ok(JsonValue::Number(serde_json::Number::from(n as u64)));
    }
    serde_json::Number::from_f64(n)
        .map(JsonValue::Number)
        .ok_or_else(|| non_finite_error(n))
}

fn run_query(
    xpath: &sxd_xpath::XPath,
    xml: &str,
    config: &XPathConfig,
) -> Result<JsonValue, LanguageError> {
    if let Some(max) = config.max_input_bytes
        && xml.len() > max
    {
        return Err(LanguageError::EvalFailure {
            class: ExpressionErrorClass::Limit,
            position: None,
            detail: Some("input exceeds maximum allowed size".into()),
        });
    }
    let package = parser::parse(xml).map_err(|_| {
        // sxd parse errors can embed document-derived content (e.g. MismatchedTag
        // includes tag names from the exchange body which may be sensitive).
        // Return a generic message; do NOT include the raw error in logs or errors.
        debug!("xpath: body XML could not be parsed");
        LanguageError::EvalFailure {
            class: ExpressionErrorClass::Parse,
            position: None,
            detail: Some("body is not valid XML".into()),
        }
    })?;
    let doc = package.as_document();
    // TODO(XPH-001): Namespace declarations are not yet supported. The context
    // should be populated with namespace prefix → URI mappings from XPathConfig
    // before calling xpath.evaluate().
    let context = Context::new();
    let result = xpath.evaluate(&context, doc.root()).map_err(|_| {
        // sxd_xpath eval errors describe query structure issues (unknown variable/function,
        // type mismatch). No document-derived values are embedded, but we follow the
        // same conservative pattern: generic message, no raw external error strings.
        debug!("xpath: expression evaluation failed");
        LanguageError::EvalFailure {
            class: ExpressionErrorClass::Runtime,
            position: None,
            detail: Some("xpath query failed: expression could not be evaluated".into()),
        }
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
        SxdValue::Number(n) => number_to_json(n)?,
        SxdValue::String(s) => JsonValue::String(s),
    })
}

#[async_trait]
impl Expression for XPathExpression {
    async fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        let xml = extract_xml(exchange)?;
        run_query(&self.xpath, &xml, &self.config)
    }
}

#[async_trait]
impl Predicate for XPathPredicate {
    async fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let xml = extract_xml(exchange)?;
        let result = run_query(&self.xpath, &xml, &self.config)?;
        // Strict bool: only an XPath boolean result is valid. Nodesets,
        // numbers, strings, and null are type errors — no truthiness coercion
        // (change `language-value-boundary`, sealed Q3).
        match &result {
            JsonValue::Bool(b) => Ok(*b),
            other => Err(LanguageError::TypeMismatch {
                expected: "bool".to_string(),
                actual: json_type_name(other).to_string(),
                position: None,
            }),
        }
    }
}

/// Type name of a [`JsonValue`] for `TypeMismatch` diagnostics. Type names
/// only — never runtime values.
fn json_type_name(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "bool",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
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
        let xpath = compile_xpath(script)?;
        debug!("xpath expression compiled");
        Ok(Box::new(XPathExpression {
            xpath,
            config: self.config.clone(),
        }))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        let xpath = compile_xpath(script)?;
        debug!("xpath expression compiled");
        Ok(Box::new(XPathPredicate {
            xpath,
            config: self.config.clone(),
        }))
    }
}

#[cfg(test)]
mod conformance_tests;

#[cfg(test)]
mod number_boundary_tests;

#[cfg(test)]
mod property_tests;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::test_support::{eval_meta, mapped_class_and_cause};
    use super::*;
    use camel_api::{CamelError, ExpressionErrorClass};
    use camel_language_api::Message;
    use std::cell::Cell;

    /// Reset this thread's compile counter, returning the previous value.
    fn reset_compile_count() -> usize {
        COMPILE_COUNT.with(|c| c.replace(0))
    }

    /// Read this thread's compile counter without resetting.
    fn read_compile_count() -> usize {
        COMPILE_COUNT.with(Cell::get)
    }

    async fn exchange_with_xml(xml: &str) -> Exchange {
        Exchange::new(Message::new(Body::Xml(xml.to_string())))
    }

    async fn exchange_with_text_body(text: &str) -> Exchange {
        Exchange::new(Message::new(Body::Text(text.to_string())))
    }

    async fn empty_exchange() -> Exchange {
        Exchange::new(Message::default())
    }

    /// Render every link of the error chain starting at `err`.
    fn render_error_chain(err: &dyn std::error::Error) -> String {
        let mut rendered = err.to_string();
        let mut next = err.source();
        while let Some(link) = next {
            rendered.push_str(" | ");
            rendered.push_str(&link.to_string());
            next = link.source();
        }
        rendered
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
    async fn xpath_predicate_non_bool_is_type_mismatch() {
        // Sealed Q3: only an XPath boolean is a valid predicate result.
        // Nodesets, non-empty strings, and non-zero numbers are type errors.
        let lang = XPathLanguage::new();
        let cases: [(&str, &str, &str); 3] = [
            (
                "/root/item",
                "<root><item>a</item><item>b</item></root>",
                "array",
            ),
            ("/root/missing", "<root><name>test</name></root>", "null"),
            (
                "string(/root/name)",
                "<root><name>hello</name></root>",
                "string",
            ),
        ];
        for (query, xml, actual) in cases {
            let pred = lang.create_predicate(query).unwrap();
            let ex = exchange_with_xml(xml).await;
            match pred.matches(&ex).await {
                Err(LanguageError::TypeMismatch {
                    expected,
                    actual: got,
                    position: None,
                }) => {
                    assert_eq!(expected, "bool");
                    assert_eq!(got, actual, "query `{query}`");
                }
                other => panic!("expected TypeMismatch for `{query}`, got: {other:?}"),
            }
        }

        // A real boolean result still works.
        let pred = lang.create_predicate("count(/root/item) > 1").unwrap();
        let ex = exchange_with_xml("<root><item>a</item><item>b</item></root>").await;
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn xpath_no_warn_on_eval_failure() {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct SharedWriter(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for SharedWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer = SharedWriter(Arc::clone(&buf));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);

        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root").unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Xml("<broken>".to_string());
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err(), "invalid XML must fail the eval");

        drop(guard);
        let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            out.is_empty(),
            "language crate must not emit WARN/ERROR on eval failure, got: {out}"
        );
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
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("oversized input must fail");

        match &err {
            LanguageError::EvalFailure {
                class,
                position,
                detail,
            } => {
                assert_eq!(*class, ExpressionErrorClass::Limit);
                assert_eq!(*position, None);
                assert_eq!(
                    detail.as_deref(),
                    Some("input exceeds maximum allowed size")
                );
            }
            other => panic!("expected EvalFailure Limit, got {other:?}"),
        }

        let mapped = camel_language_api::to_expression_failed(err, &eval_meta());
        match mapped {
            CamelError::ExpressionFailed { class, cause, .. } => {
                assert_eq!(class, ExpressionErrorClass::Limit);
                assert_eq!(
                    cause.expect("expected redacted cause").to_string(),
                    "Processor error: input exceeds maximum allowed size"
                );
            }
            other => panic!("expected ExpressionFailed, got {other:?}"),
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
        match pred
            .matches(&ex)
            .await
            .expect_err("oversized input must fail")
        {
            LanguageError::EvalFailure { class, .. } => {
                assert_eq!(class, ExpressionErrorClass::Limit);
            }
            other => panic!("expected EvalFailure Limit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn coercion_failure_classifies_as_conversion() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root").unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Json(serde_json::json!("SECRETSENTINEL"));

        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("json scalar cannot be coerced to XML");
        match &err {
            LanguageError::EvalFailure { class, .. } => {
                assert_eq!(*class, ExpressionErrorClass::Conversion);
            }
            other => panic!("expected EvalFailure Conversion, got {other:?}"),
        }

        let (class, cause, _) = mapped_class_and_cause(err);
        assert_eq!(class, ExpressionErrorClass::Conversion);
        assert_eq!(
            cause,
            "Processor error: body is not XML and cannot be coerced"
        );
    }

    #[tokio::test]
    async fn malformed_xml_classifies_as_parse() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("/root").unwrap();
        let ex = exchange_with_xml("<root><unclosed>").await;

        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("malformed XML must fail");
        match &err {
            LanguageError::EvalFailure { class, .. } => {
                assert_eq!(*class, ExpressionErrorClass::Parse);
            }
            other => panic!("expected EvalFailure Parse, got {other:?}"),
        }

        let (class, cause, _) = mapped_class_and_cause(err);
        assert_eq!(class, ExpressionErrorClass::Parse);
        assert_eq!(cause, "Processor error: body is not valid XML");
    }

    #[tokio::test]
    async fn genuine_eval_failure_keeps_runtime() {
        let lang = XPathLanguage::new();
        let expr = lang.create_expression("$__undefined_var__").unwrap();
        let ex = exchange_with_xml("<root/>").await;

        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("undefined variable must fail at evaluation");
        match &err {
            LanguageError::EvalFailure { class, .. } => {
                assert_eq!(*class, ExpressionErrorClass::Runtime);
            }
            other => panic!("expected EvalFailure Runtime, got {other:?}"),
        }

        let (class, cause, _) = mapped_class_and_cause(err);
        assert_eq!(class, ExpressionErrorClass::Runtime);
        assert_eq!(
            cause,
            "Processor error: xpath query failed: expression could not be evaluated"
        );
    }

    #[tokio::test]
    async fn mapped_100kb_size_vs_malformed_distinguishable() {
        let limit = 96 * 1024;
        let lang = XPathLanguage::with_config(XPathConfig {
            max_input_bytes: Some(limit),
        });
        let expr = lang.create_expression("/root").unwrap();

        let body_a = format!("<root>{}</root>", "x".repeat(limit));
        assert!(body_a.len() > limit, "body A must exceed the size limit");
        let ex_a = exchange_with_xml(&body_a).await;
        let err_a = expr
            .evaluate(&ex_a)
            .await
            .expect_err("oversized body must fail");
        let (class_a, cause_a, _) = mapped_class_and_cause(err_a);
        assert_eq!(class_a, ExpressionErrorClass::Limit);
        assert!(cause_a.contains("input exceeds maximum allowed size"));

        let body_b = format!("<root><a>{}</root>", "y".repeat(limit - 32));
        assert!(
            body_b.len() < limit,
            "body B must stay below the size limit: {} vs {limit}",
            body_b.len()
        );
        let ex_b = exchange_with_xml(&body_b).await;
        let err_b = expr
            .evaluate(&ex_b)
            .await
            .expect_err("malformed body must fail");
        let (class_b, cause_b, _) = mapped_class_and_cause(err_b);
        assert_eq!(class_b, ExpressionErrorClass::Parse);
        assert!(cause_b.contains("body is not valid XML"));

        assert_ne!(class_a, class_b);
        assert_ne!(cause_a, cause_b);
    }

    #[tokio::test]
    async fn sentinel_never_enters_mapped_cause() {
        const SENTINEL: &str = "SECRETSENTINEL";
        let limit = 96 * 1024;
        let lang = XPathLanguage::with_config(XPathConfig {
            max_input_bytes: Some(limit),
        });
        let expr = lang.create_expression("/root").unwrap();

        // Limit: sentinel inside the padding text, body above the limit.
        let body_a = format!("<root>{SENTINEL}{}</root>", "x".repeat(limit));
        assert!(body_a.len() > limit);
        let ex_a = exchange_with_xml(&body_a).await;
        let err_a = expr
            .evaluate(&ex_a)
            .await
            .expect_err("oversized body must fail");
        let (class_a, cause_a, mapped_a) = mapped_class_and_cause(err_a);
        assert_eq!(class_a, ExpressionErrorClass::Limit);
        assert!(!cause_a.contains(SENTINEL));
        assert!(!render_error_chain(&mapped_a).contains(SENTINEL));

        // Parse: sentinel inside an unclosed tag name, body below the limit.
        let body_b = format!("<root><{SENTINEL}>{}</root>", "y".repeat(limit - 64));
        assert!(
            body_b.len() < limit,
            "body B must stay below the size limit: {} vs {limit}",
            body_b.len()
        );
        let ex_b = exchange_with_xml(&body_b).await;
        let err_b = expr
            .evaluate(&ex_b)
            .await
            .expect_err("malformed body must fail");
        let (class_b, cause_b, mapped_b) = mapped_class_and_cause(err_b);
        assert_eq!(class_b, ExpressionErrorClass::Parse);
        assert!(!cause_b.contains(SENTINEL));
        assert!(!render_error_chain(&mapped_b).contains(SENTINEL));

        // Conversion: sentinel as a JSON scalar body.
        let mut ex_c = Exchange::new(Message::default());
        ex_c.input.body = Body::Json(serde_json::json!(SENTINEL));
        let err_c = expr
            .evaluate(&ex_c)
            .await
            .expect_err("json scalar cannot be coerced");
        let (class_c, cause_c, mapped_c) = mapped_class_and_cause(err_c);
        assert_eq!(class_c, ExpressionErrorClass::Conversion);
        assert!(!cause_c.contains(SENTINEL));
        assert!(!render_error_chain(&mapped_c).contains(SENTINEL));

        // Runtime: undefined variable evaluated against a body containing the sentinel.
        let runtime_expr = lang.create_expression("$__undefined_var__").unwrap();
        let ex_d = exchange_with_xml(&format!("<root>{SENTINEL}</root>")).await;
        let err_d = runtime_expr
            .evaluate(&ex_d)
            .await
            .expect_err("undefined variable must fail");
        let (class_d, cause_d, mapped_d) = mapped_class_and_cause(err_d);
        assert_eq!(class_d, ExpressionErrorClass::Runtime);
        assert!(!cause_d.contains(SENTINEL));
        assert!(!render_error_chain(&mapped_d).contains(SENTINEL));
    }

    /// Regression test for FC-LANG-RECOMPILE: proves the XPath is compiled
    /// exactly once at `create_expression` time, not on every evaluation.
    /// Before the fix, each `evaluate` call re-invoked `compile_xpath`,
    /// so the counter would increment 1 (create) + N (evaluates).
    /// Uses a thread-local counter so parallel tokio tests do not interfere.
    #[tokio::test]
    async fn expression_compiles_only_once() {
        let lang = XPathLanguage::new();
        let _ = reset_compile_count();
        let expr = lang.create_expression("/root/item").unwrap();
        assert_eq!(
            read_compile_count(),
            1,
            "create_expression should compile exactly once (got {})",
            read_compile_count()
        );

        for i in 0..5 {
            let ex = exchange_with_xml("<root><item>a</item></root>").await;
            let _ = expr.evaluate(&ex).await.unwrap();
            assert_eq!(
                read_compile_count(),
                1,
                "evaluate call #{i} must not trigger recompilation (got {})",
                read_compile_count()
            );
        }
    }

    /// Regression test for FC-LANG-RECOMPILE on the predicate path.
    #[tokio::test]
    async fn predicate_compiles_only_once() {
        let lang = XPathLanguage::new();
        let _ = reset_compile_count();
        let pred = lang.create_predicate("count(/root/item) = 1").unwrap();
        assert_eq!(
            read_compile_count(),
            1,
            "create_predicate should compile exactly once (got {})",
            read_compile_count()
        );

        for i in 0..5 {
            let ex = exchange_with_xml("<root><item>a</item></root>").await;
            let _ = pred.matches(&ex).await.unwrap();
            assert_eq!(
                read_compile_count(),
                1,
                "matches call #{i} must not trigger recompilation (got {})",
                read_compile_count()
            );
        }
    }
}
