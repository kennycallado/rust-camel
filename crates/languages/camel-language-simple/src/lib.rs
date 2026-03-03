mod parser;
mod evaluator;

use camel_api::exchange::Exchange;
use camel_language_api::{Expression, Language, LanguageError, Predicate};

pub struct SimpleLanguage;

struct SimpleExpression(parser::Expr);
struct SimplePredicate(parser::Expr);

impl Expression for SimpleExpression {
    fn evaluate(&self, exchange: &Exchange) -> Result<camel_api::Value, LanguageError> {
        evaluator::evaluate(&self.0, exchange)
    }
}

impl Predicate for SimplePredicate {
    fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let val = evaluator::evaluate(&self.0, exchange)?;
        Ok(match &val {
            camel_api::Value::Bool(b) => *b,
            camel_api::Value::Null => false,
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
    use camel_api::{exchange::Exchange, message::Message, Value};
    use camel_language_api::Language;
    use super::SimpleLanguage;

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
        assert_eq!(val, Value::String("".to_string()));
    }

    #[test]
    fn test_parse_error_unrecognized_token() {
        let lang = SimpleLanguage;
        let result = lang.create_expression("@@invalid@@");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_header_key_is_parse_error() {
        let lang = SimpleLanguage;
        let result = lang.create_expression("${header.}");
        let err = result.err().expect("should be a parse error");
        let err = format!("{err}");
        assert!(err.contains("empty"), "error should mention empty key, got: {err}");
    }

    #[test]
    fn test_empty_exchange_property_key_is_parse_error() {
        let lang = SimpleLanguage;
        let result = lang.create_expression("${exchangeProperty.}");
        let err = result.err().expect("should be a parse error");
        let err = format!("{err}");
        assert!(err.contains("empty"), "error should mention empty key, got: {err}");
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
        let expr = lang.create_expression("${exchangeProperty.myProp}").unwrap();
        let mut ex = exchange_with_body("test");
        ex.set_property("myProp".to_string(), Value::String("propVal".to_string()));
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("propVal".to_string()));
    }

    #[test]
    fn test_missing_property_returns_null() {
        let lang = SimpleLanguage;
        let expr = lang.create_expression("${exchangeProperty.missing}").unwrap();
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

    #[test]
    fn test_operator_inside_string_literal_not_split() {
        // The string literal 'a>=b' contains '>=' but must NOT be parsed as a
        // BinOp split — the whole thing is a StringLit atom.
        let lang = SimpleLanguage;
        let result = lang.create_expression("'a>=b'");
        let val = result.unwrap().evaluate(&Exchange::new(Message::default())).unwrap();
        assert_eq!(val, Value::String("a>=b".to_string()),
            "string literal 'a>=b' should be parsed as a plain string, not split on >=");
    }

    #[test]
    fn test_header_eq_string_literal_containing_operator() {
        // ${header.x} == 'a>=b' — the operator inside the RHS string must not
        // cause the parser to split the LHS at the wrong position.
        let lang = SimpleLanguage;
        let pred = lang
            .create_predicate("${header.x} == 'a>=b'")
            .unwrap();
        let ex = exchange_with_header("x", "a>=b");
        assert!(pred.matches(&ex).unwrap(),
            "predicate should match when header equals 'a>=b'");
    }
}