mod evaluator;
mod parser;

use camel_language_api::{Exchange, Expression, Language, LanguageError, Predicate, Value};

pub struct SimpleLanguage;

struct SimpleExpression(parser::Expr);
struct SimplePredicate(parser::Expr);

impl Expression for SimpleExpression {
    fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        evaluator::evaluate(&self.0, exchange)
    }
}

impl Predicate for SimplePredicate {
    fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let val = evaluator::evaluate(&self.0, exchange)?;
        Ok(match &val {
            Value::Bool(b) => *b,
            Value::Null => false,
            _ => true,
        })
    }
}

impl Language for SimpleLanguage {
    fn name(&self) -> &'static str {
        "simple"
    }

    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
        let ast = parser::parse(script)?;
        Ok(Box::new(SimpleExpression(ast)))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        let ast = parser::parse(script)?;
        Ok(Box::new(SimplePredicate(ast)))
    }
}

#[cfg(test)]
mod tests {
    use super::SimpleLanguage;
    use camel_language_api::Language;
    use camel_language_api::{Exchange, Message, Value};

    fn exchange_with_header(key: &str, val: &str) -> Exchange {
        let mut msg = Message::default();
        msg.set_header(key, Value::String(val.to_string()));
        Exchange::new(msg)
    }

    fn exchange_with_body(body: &str) -> Exchange {
        Exchange::new(Message::new(body))
    }

    #[test]
    fn test_header_equals_string() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.type} == 'order'").unwrap();
        let ex = exchange_with_header("type", "order");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_header_not_equals() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.type} != 'order'").unwrap();
        let ex = exchange_with_header("type", "invoice");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_body_contains() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${body} contains 'hello'").unwrap();
        let ex = exchange_with_body("say hello world");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_header_null_check() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.missing} == null").unwrap();
        let ex = exchange_with_body("anything");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_header_not_null() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.type} != null").unwrap();
        let ex = exchange_with_header("type", "order");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_expression_header_value() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("${header.type}").unwrap();
        let ex = exchange_with_header("type", "order");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("order".to_string()));
    }

    #[test]
    fn test_expression_body() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("${body}").unwrap();
        let ex = exchange_with_body("hello");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("hello".to_string()));
    }

    #[test]
    fn test_numeric_comparison() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.age} > 18").unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header("age", Value::Number(25.into()));
        assert!(pred.matches(&ex).unwrap());
    }

    // --- Edge case tests ---

    #[test]
    fn test_empty_body() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("${body}").unwrap();
        let ex = Exchange::new(Message::default());
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::Null);
    }

    #[test]
    fn test_parse_error_unrecognized_token() {
        // A pure `${...}` token that doesn't match any known form is a parse error.
        // For example `${unknown}` is not a valid Simple expression.
        let lang = SimpleLanguage;
        let result = lang.create_expression("${unknown}");
        assert!(result.is_err(), "unknown token should be a parse error");
    }

    #[test]
    fn test_empty_header_key_is_parse_error() {
        let lang = SimpleLanguage;
        let result = lang.create_expression("${header.}");
        let err = result.err().expect("should be a parse error");
        let err = format!("{err}");
        assert!(
            err.contains("empty"),
            "error should mention empty key, got: {err}"
        );
    }

    #[test]
    fn test_empty_exchange_property_key_is_parse_error() {
        let lang = SimpleLanguage;
        let result = lang.create_expression("${exchangeProperty.}");
        let err = result.err().expect("should be a parse error");
        let err = format!("{err}");
        assert!(
            err.contains("empty"),
            "error should mention empty key, got: {err}"
        );
    }

    #[test]
    fn test_missing_header_returns_null() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("${header.nonexistent}").unwrap();
        let ex = exchange_with_body("anything");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::Null);
    }

    #[test]
    fn test_exchange_property_expression() {
        let lang = SimpleLanguage;
        let expr = lang
            .create_expression("${exchangeProperty.myProp}")
            .unwrap();
        let mut ex = exchange_with_body("test");
        ex.set_property("myProp".to_string(), Value::String("propVal".to_string()));
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("propVal".to_string()));
    }

    #[test]
    fn test_missing_property_returns_null() {
        let lang = SimpleLanguage;
        let expr = lang
            .create_expression("${exchangeProperty.missing}")
            .unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::Null);
    }

    #[test]
    fn test_string_literal_expression() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("'hello'").unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("hello".to_string()));
    }

    #[test]
    fn test_double_quoted_literal_unescapes_newline() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("\"line1\\nline2\"").unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("line1\nline2".to_string()));
    }

    #[test]
    fn test_double_quoted_literal_unescapes_tab_and_quote() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("\"col1\\t\\\"quoted\\\"\"").unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("col1\t\"quoted\"".to_string()));
    }

    #[test]
    fn test_double_quoted_literal_unescapes_backspace_formfeed_and_slash() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("\"a\\bb\\fc\\/d\"").unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("a\u{0008}b\u{000C}c/d".to_string()));
    }

    #[test]
    fn test_null_expression() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("null").unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::Null);
    }

    #[test]
    fn test_predicate_null_is_false() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.missing}").unwrap();
        let ex = exchange_with_body("test");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_predicate_non_null_is_true() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.type}").unwrap();
        let ex = exchange_with_header("type", "order");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_contains_not_found() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${body} contains 'xyz'").unwrap();
        let ex = exchange_with_body("hello world");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_less_than_or_equal() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.age} <= 18").unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header("age", Value::Number(18.into()));
        assert!(pred.matches(&ex).unwrap());
    }

    // --- Mixed interpolation tests ---

    #[test]
    fn test_interpolated_text_with_header() {
        // "Exchange #${header.CamelTimerCounter}" → "Exchange #42"
        let lang = SimpleLanguage;
        let expr = lang
            .create_expression("Exchange #${header.CamelTimerCounter}")
            .unwrap();
        let ex = exchange_with_header("CamelTimerCounter", "42");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("Exchange #42".to_string()));
    }

    #[test]
    fn test_interpolated_text_with_body() {
        // "Got ${body}" → "Got hello"
        let lang = SimpleLanguage;
        let expr = lang.create_expression("Got ${body}").unwrap();
        let ex = exchange_with_body("hello");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("Got hello".to_string()));
    }

    #[test]
    fn test_interpolated_multiple_expressions() {
        // "Transformed: ${body} (source=${header.source})" → real values
        let lang = SimpleLanguage;
        let expr = lang
            .create_expression("Transformed: ${body} (source=${header.source})")
            .unwrap();
        let mut msg = Message::new("data");
        msg.set_header("source", Value::String("kafka".to_string()));
        let ex = Exchange::new(msg);
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(
            val,
            Value::String("Transformed: data (source=kafka)".to_string())
        );
    }

    #[test]
    fn test_interpolated_missing_header_becomes_empty() {
        // Missing header in interpolated string yields empty string for that slot
        let lang = SimpleLanguage;
        let expr = lang
            .create_expression("prefix-${header.missing}-suffix")
            .unwrap();
        let ex = exchange_with_body("x");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("prefix--suffix".to_string()));
    }

    #[test]
    fn test_interpolated_text_only_no_expressions() {
        // Plain text with no ${...} — treated as a literal (no interpolation needed,
        // but must still work without error)
        let lang = SimpleLanguage;
        let expr = lang.create_expression("Hello World").unwrap();
        let ex = exchange_with_body("x");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("Hello World".to_string()));
    }

    #[test]
    fn test_interpolated_unclosed_brace_treated_as_literal() {
        // An unclosed `${` has no matching `}` — the remainder is treated as
        // plain literal text rather than causing a parse error.
        let lang = SimpleLanguage;
        let expr = lang.create_expression("Got ${body").unwrap();
        let ex = exchange_with_body("hello");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("Got ${body".to_string()));
    }

    #[test]
    fn test_operator_inside_string_literal_not_split() {
        // The string literal 'a>=b' contains '>=' but must NOT be parsed as a
        // BinOp split — the whole thing is a StringLit atom.
        let lang = SimpleLanguage;
        let result = lang.create_expression("'a>=b'");
        let val = result
            .unwrap()
            .evaluate(&Exchange::new(Message::default()))
            .unwrap();
        assert_eq!(
            val,
            Value::String("a>=b".to_string()),
            "string literal 'a>=b' should be parsed as a plain string, not split on >="
        );
    }

    #[test]
    fn test_header_eq_string_literal_containing_operator() {
        // ${header.x} == 'a>=b' — the operator inside the RHS string must not
        // cause the parser to split the LHS at the wrong position.
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.x} == 'a>=b'").unwrap();
        let ex = exchange_with_header("x", "a>=b");
        assert!(
            pred.matches(&ex).unwrap(),
            "predicate should match when header equals 'a>=b'"
        );
    }

    #[test]
    fn test_body_empty_is_null() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("${body}").unwrap();
        let ex = Exchange::new(Message::default());
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::Null);
    }

    #[test]
    fn test_body_empty_not_null_is_false() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${body} != null").unwrap();
        let ex = Exchange::new(Message::default());
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_body_empty_predicate_is_false() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${body}").unwrap();
        let ex = Exchange::new(Message::default());
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_logical_and_true_true() {
        let lang = SimpleLanguage;
        let pred = lang
            .create_predicate("${header.a} == '1' && ${header.b} == '2'")
            .unwrap();
        let mut msg = Message::default();
        msg.set_header("a", Value::String("1".to_string()));
        msg.set_header("b", Value::String("2".to_string()));
        let ex = Exchange::new(msg);
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_logical_and_true_false() {
        let lang = SimpleLanguage;
        let pred = lang
            .create_predicate("${header.a} == '1' && ${header.b} == '2'")
            .unwrap();
        let mut msg = Message::default();
        msg.set_header("a", Value::String("1".to_string()));
        msg.set_header("b", Value::String("99".to_string()));
        let ex = Exchange::new(msg);
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_logical_or_false_true() {
        let lang = SimpleLanguage;
        let pred = lang
            .create_predicate("${header.a} == '1' || ${header.b} == '2'")
            .unwrap();
        let mut msg = Message::default();
        msg.set_header("a", Value::String("0".to_string()));
        msg.set_header("b", Value::String("2".to_string()));
        let ex = Exchange::new(msg);
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_logical_or_false_false() {
        let lang = SimpleLanguage;
        let pred = lang
            .create_predicate("${header.a} == '1' || ${header.b} == '2'")
            .unwrap();
        let mut msg = Message::default();
        msg.set_header("a", Value::String("0".to_string()));
        msg.set_header("b", Value::String("0".to_string()));
        let ex = Exchange::new(msg);
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_logical_and_precedence_over_or() {
        let lang = SimpleLanguage;
        let pred = lang
            .create_predicate("${header.a} == '1' || ${header.b} == '2' && ${header.c} == '3'")
            .unwrap();
        let mut msg = Message::default();
        msg.set_header("a", Value::String("1".to_string()));
        msg.set_header("b", Value::String("2".to_string()));
        msg.set_header("c", Value::String("999".to_string()));
        let ex = Exchange::new(msg);
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_logical_and_short_circuit() {
        let lang = SimpleLanguage;
        let pred = lang
            .create_predicate("${header.a} == 'x' && ${header.nonexistent} > 999")
            .unwrap();
        let mut msg = Message::default();
        msg.set_header("a", Value::String("not-x".to_string()));
        let ex = Exchange::new(msg);
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_logical_or_short_circuit() {
        let lang = SimpleLanguage;
        let pred = lang
            .create_predicate("${header.a} == '1' || ${header.nonexistent} > 999")
            .unwrap();
        let mut msg = Message::default();
        msg.set_header("a", Value::String("1".to_string()));
        let ex = Exchange::new(msg);
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_compound_filter_body_not_null_and_body_not_empty() {
        let lang = SimpleLanguage;
        let pred = lang
            .create_predicate("${body} != null && ${body} != ''")
            .unwrap();
        let ex = exchange_with_body("hello");
        assert!(pred.matches(&ex).unwrap());

        let ex_empty = Exchange::new(Message::default());
        assert!(!pred.matches(&ex_empty).unwrap());
    }

    #[test]
    fn test_header_with_gt_in_key() {
        let lang = SimpleLanguage;
        let mut msg = Message::default();
        msg.set_header("a>b", Value::String("found".to_string()));
        let pred = lang.create_predicate("${header.a>b} == 'found'").unwrap();
        let ex = Exchange::new(msg);
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_double_quoted_string_with_operator() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("\"a >= b\"").unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("a >= b".to_string()));
    }

    #[test]
    fn test_bool_literal_true() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("true").unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::Bool(true));
    }

    #[test]
    fn test_bool_literal_false() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("false").unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::Bool(false));
    }

    #[test]
    fn test_bool_literal_in_comparison() {
        use camel_language_api::Body;

        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(serde_json::json!({"active": true}));
        let pred = lang.create_predicate("${body.active} == true").unwrap();
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_null_gt_number_is_false() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.missing} > 5").unwrap();
        let ex = exchange_with_body("test");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_null_lt_number_is_false() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.missing} < 10").unwrap();
        let ex = exchange_with_body("test");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_null_gte_number_is_false() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.missing} >= 0").unwrap();
        let ex = exchange_with_body("test");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_null_lte_number_is_false() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${header.missing} <= 100").unwrap();
        let ex = exchange_with_body("test");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_number_gt_null_is_false() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("5 > ${header.missing}").unwrap();
        let ex = exchange_with_body("test");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_true_or_false() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("true || false").unwrap();
        let ex = exchange_with_body("test");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_false_and_true() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("false && true").unwrap();
        let ex = exchange_with_body("test");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_contains_null_left_is_false() {
        let lang = SimpleLanguage;
        let pred = lang
            .create_predicate("${header.missing} contains 'x'")
            .unwrap();
        let ex = exchange_with_body("test");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_contains_null_right_is_false() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${body} contains null").unwrap();
        let ex = exchange_with_body("test");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_contains_without_spaces() {
        let lang = SimpleLanguage;
        let pred = lang.create_predicate("${body}contains'hello'").unwrap();
        let ex = exchange_with_body("say hello world");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_non_finite_number_parse_error() {
        let lang = SimpleLanguage;
        // Very long digit string that overflows f64 to infinity
        let result = lang.create_expression(
            "9999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999",
        );
        assert!(result.is_err(), "non-finite number should be a parse error");
    }

    #[test]
    fn test_interpolation_with_empty_body_still_produces_text() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("Got ${body}").unwrap();
        let ex = Exchange::new(Message::default());
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("Got ".to_string()));
    }

    #[test]
    fn test_interpolation_with_empty_body_and_trailing_text() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("${body} tail").unwrap();
        let ex = Exchange::new(Message::default());
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String(" tail".to_string()));
    }
}

#[cfg(test)]
mod body_field_parser_tests {
    use crate::parser::{Expr, PathSegment, parse};

    #[test]
    fn parse_body_field_simple_key() {
        let expr = parse("${body.name}").unwrap();
        assert_eq!(
            expr,
            Expr::BodyField(vec![PathSegment::Key("name".to_string())])
        );
    }

    #[test]
    fn parse_body_field_nested() {
        let expr = parse("${body.user.city}").unwrap();
        assert_eq!(
            expr,
            Expr::BodyField(vec![
                PathSegment::Key("user".to_string()),
                PathSegment::Key("city".to_string()),
            ])
        );
    }

    #[test]
    fn parse_body_field_array_index() {
        let expr = parse("${body.items.0}").unwrap();
        assert_eq!(
            expr,
            Expr::BodyField(vec![
                PathSegment::Key("items".to_string()),
                PathSegment::Index(0),
            ])
        );
    }

    #[test]
    fn parse_body_field_array_nested() {
        let expr = parse("${body.users.0.name}").unwrap();
        assert_eq!(
            expr,
            Expr::BodyField(vec![
                PathSegment::Key("users".to_string()),
                PathSegment::Index(0),
                PathSegment::Key("name".to_string()),
            ])
        );
    }

    #[test]
    fn parse_body_field_empty_segment_error() {
        let result = parse("${body.}");
        assert!(result.is_err());
    }

    #[test]
    fn parse_body_field_exact_still_works() {
        // Regression: ${body} must still produce Expr::Body, not BodyField
        let expr = parse("${body}").unwrap();
        assert_eq!(expr, Expr::Body);
    }

    #[test]
    fn parse_body_field_double_dots_error() {
        // ${body..name} has an empty segment between the two dots
        let result = parse("${body..name}");
        assert!(result.is_err());
    }

    #[test]
    fn parse_body_field_index_only() {
        // ${body.0} — single index segment (e.g. body is a JSON array)
        let expr = parse("${body.0}").unwrap();
        assert_eq!(expr, Expr::BodyField(vec![PathSegment::Index(0)]));
    }

    #[test]
    fn parse_body_field_leading_zero_is_key() {
        // ${body.01} — leading zero means it's a string key, not an array index
        let expr = parse("${body.01}").unwrap();
        assert_eq!(
            expr,
            Expr::BodyField(vec![PathSegment::Key("01".to_string())])
        );
    }
}

#[cfg(test)]
mod body_field_eval_tests {
    use crate::SimpleLanguage;
    use camel_language_api::Language;
    use camel_language_api::{Body, Exchange, Value};
    use serde_json::json;

    fn eval(expr_str: &str, body: Body) -> Value {
        let mut ex = Exchange::default();
        ex.input.body = body;
        let lang = SimpleLanguage;
        lang.create_expression(expr_str)
            .unwrap()
            .evaluate(&ex)
            .unwrap()
    }

    #[test]
    fn body_field_simple_key() {
        let result = eval("${body.name}", Body::Json(json!({"name": "Alice"})));
        assert_eq!(result, json!("Alice"));
    }

    #[test]
    fn body_field_number_value() {
        let result = eval("${body.age}", Body::Json(json!({"age": 30})));
        assert_eq!(result, json!(30));
    }

    #[test]
    fn body_field_bool_value() {
        let result = eval("${body.active}", Body::Json(json!({"active": true})));
        assert_eq!(result, json!(true));
    }

    #[test]
    fn body_field_nested() {
        let result = eval(
            "${body.user.city}",
            Body::Json(json!({"user": {"city": "Madrid"}})),
        );
        assert_eq!(result, json!("Madrid"));
    }

    #[test]
    fn body_field_array_index() {
        let result = eval("${body.items.0}", Body::Json(json!({"items": ["a", "b"]})));
        assert_eq!(result, json!("a"));
    }

    #[test]
    fn body_field_array_nested() {
        let result = eval(
            "${body.users.0.name}",
            Body::Json(json!({"users": [{"name": "Bob"}]})),
        );
        assert_eq!(result, json!("Bob"));
    }

    #[test]
    fn body_field_missing_key_returns_null() {
        let result = eval("${body.missing}", Body::Json(json!({"name": "Alice"})));
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn body_field_missing_nested_returns_null() {
        let result = eval("${body.a.b.c}", Body::Json(json!({"a": {"x": 1}})));
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn body_field_out_of_bounds_index_returns_null() {
        let result = eval("${body.items.5}", Body::Json(json!({"items": ["a"]})));
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn body_field_non_json_body_returns_null() {
        let result = eval(
            "${body.name}",
            Body::Text(r#"{"name":"Alice"}"#.to_string()),
        );
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn body_field_empty_body_returns_null() {
        let result = eval("${body.name}", Body::Empty);
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn body_field_in_interpolation() {
        let result = eval("Hello ${body.name}!", Body::Json(json!({"name": "Alice"})));
        assert_eq!(result, json!("Hello Alice!"));
    }

    #[test]
    fn body_field_in_predicate_true() {
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(json!({"status": "active"}));
        let result = lang
            .create_expression("${body.status} == 'active'")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn body_field_in_predicate_false() {
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(json!({"status": "inactive"}));
        let result = lang
            .create_expression("${body.status} == 'active'")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn body_field_bytes_body_returns_null() {
        // Note: Using Body::Text here instead of Body::Bytes since bytes crate
        // is not available in test context. Both should behave the same for JSON field access.
        let result = eval(
            "${body.name}",
            Body::Text(r#"{"name":"Alice"}"#.to_string()),
        );
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn body_field_json_null_value_returns_null() {
        // key exists but its value is JSON null → returns Value::Null
        let result = eval(
            "${body.name}",
            Body::Json(serde_json::json!({"name": null})),
        );
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn body_field_numeric_predicate() {
        // JSON number 42.0 compares equal to the parsed number 42 from the
        // Simple Language expression, because both resolve to the same f64.
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(json!({"score": 42.0}));
        let result = lang
            .create_expression("${body.score} == 42")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn body_bytes_utf8_returns_string() {
        // Body::Bytes with valid UTF-8 content should be readable via ${body}
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::from(b"hello from bytes".to_vec());
        let val = lang
            .create_expression("${body}")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(val, Value::String("hello from bytes".to_string()));
    }

    #[test]
    fn body_json_returns_serialized_string() {
        // Body::Json should be serialized to a JSON string when accessed via ${body}
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(json!({"msg": "world"}));
        let val = lang
            .create_expression("${body}")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        // The result should be the JSON serialization, not empty
        let s = match val {
            Value::String(s) => s,
            other => panic!("expected String, got {other:?}"),
        };
        assert!(!s.is_empty(), "${{body}} on Body::Json should not be empty");
        let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["msg"], "world");
    }

    #[test]
    fn body_bytes_in_interpolation() {
        // Body::Bytes should work in interpolated expressions like "Received: ${body}"
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::from(b"ping".to_vec());
        let val = lang
            .create_expression("Received: ${body}")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(val, Value::String("Received: ping".to_string()));
    }

    // --- XML→Json→Simple integration tests ---
    // These verify that JSON keys produced by xml_to_json() (@attr, #text, arrays)
    // are navigable via Simple language BodyField expressions.

    #[test]
    fn body_field_xml_attr_key() {
        // XML: <order id="123"> → JSON: {"order": {"@id": "123"}}
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(json!({"order": {"@id": "123"}}));
        let val = lang
            .create_expression("${body.order.@id}")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(val, json!("123"));
    }

    #[test]
    fn body_field_xml_hash_text() {
        // XML: <status active="true">pending</status> → JSON: {"status": {"@active": "true", "#text": "pending"}}
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(json!({"status": {"@active": "true", "#text": "pending"}}));
        let val = lang
            .create_expression("${body.status.#text}")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(val, json!("pending"));
    }

    #[test]
    fn body_field_xml_array_items() {
        // XML: <order><item>coffee</item><item>tea</item></order> → JSON: {"order": {"item": ["coffee", "tea"]}}
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(json!({"order": {"item": ["coffee", "tea"]}}));
        let val0 = lang
            .create_expression("${body.order.item.0}")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(val0, json!("coffee"));
        let val1 = lang
            .create_expression("${body.order.item.1}")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(val1, json!("tea"));
    }

    #[test]
    fn body_field_xml_array_with_attrs() {
        // XML: <root><item id="1">a</item><item id="2">b</item></root>
        // → JSON: {"root": {"item": [{"@id": "1", "#text": "a"}, {"@id": "2", "#text": "b"}]}}
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(
            json!({"root": {"item": [{"@id": "1", "#text": "a"}, {"@id": "2", "#text": "b"}]}}),
        );
        let val = lang
            .create_expression("${body.root.item.0.@id}")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(val, json!("1"));
        let val_text = lang
            .create_expression("${body.root.item.1.#text}")
            .unwrap()
            .evaluate(&ex)
            .unwrap();
        assert_eq!(val_text, json!("b"));
    }

    #[test]
    fn body_field_xml_predicate_on_attr() {
        // Predicate: ${body.order.@id} == '123'
        let lang = SimpleLanguage;
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(json!({"order": {"@id": "123", "name": "test"}}));
        let pred = lang.create_predicate("${body.order.@id} == '123'").unwrap();
        assert!(pred.matches(&ex).unwrap());
    }
}
