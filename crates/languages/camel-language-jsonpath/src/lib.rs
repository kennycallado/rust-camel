use camel_language_api::{Body, Exchange, Value};
use camel_language_api::{Expression, Language, LanguageError, Predicate};
use jsonpath_rust::JsonPath;
use serde_json::Value as JsonValue;

pub struct JsonPathLanguage;

struct JsonPathExpression {
    query: String,
}

struct JsonPathPredicate {
    query: String,
}

fn extract_json(exchange: &Exchange) -> Result<JsonValue, LanguageError> {
    match &exchange.input.body {
        Body::Json(v) => Ok(v.clone()),
        other => other
            .clone()
            .try_into_json()
            .map_err(|e| {
                LanguageError::EvalError(format!("body is not JSON and cannot be coerced: {e}"))
            })
            .and_then(|b| match b {
                Body::Json(v) => Ok(v),
                _ => Err(LanguageError::EvalError(
                    "body coercion did not produce JSON".into(),
                )),
            }),
    }
}

fn run_query(query: &str, json: &JsonValue) -> Result<JsonValue, LanguageError> {
    json.query(query)
        .map_err(|e| LanguageError::EvalError(format!("jsonpath query '{query}' failed: {e}")))
        .map(|results| match results.len() {
            0 => JsonValue::Null,
            1 => results[0].clone(),
            _ => JsonValue::Array(results.into_iter().cloned().collect()),
        })
}

impl Expression for JsonPathExpression {
    fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        let json = extract_json(exchange)?;
        run_query(&self.query, &json)
    }
}

impl Predicate for JsonPathPredicate {
    fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let json = extract_json(exchange)?;
        let result = run_query(&self.query, &json)?;
        Ok(match &result {
            JsonValue::Null => false,
            JsonValue::Bool(b) => *b,
            JsonValue::Array(arr) => !arr.is_empty(),
            _ => true,
        })
    }
}

impl Language for JsonPathLanguage {
    fn name(&self) -> &'static str {
        "jsonpath"
    }

    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
        let empty = JsonValue::Object(serde_json::Map::new());
        empty.query(script).map_err(|e| LanguageError::ParseError {
            expr: script.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Box::new(JsonPathExpression {
            query: script.to_string(),
        }))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        let empty = JsonValue::Object(serde_json::Map::new());
        empty.query(script).map_err(|e| LanguageError::ParseError {
            expr: script.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Box::new(JsonPathPredicate {
            query: script.to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_language_api::Message;

    fn exchange_with_json(json: &str) -> Exchange {
        let value: JsonValue = serde_json::from_str(json).unwrap();
        Exchange::new(Message::new(Body::Json(value)))
    }

    fn exchange_with_text_body(text: &str) -> Exchange {
        Exchange::new(Message::new(Body::Text(text.to_string())))
    }

    fn empty_exchange() -> Exchange {
        Exchange::new(Message::default())
    }

    #[test]
    fn expression_simple_path() {
        let lang = JsonPathLanguage;
        let expr = lang.create_expression("$.store.name").unwrap();
        let ex = exchange_with_json(r#"{"store":{"name":"books"}}"#);
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::String("books".to_string()));
    }

    #[test]
    fn expression_nested_path() {
        let lang = JsonPathLanguage;
        let expr = lang.create_expression("$.a.b.c").unwrap();
        let ex = exchange_with_json(r#"{"a":{"b":{"c":42}}}"#);
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::Number(42.into()));
    }

    #[test]
    fn expression_array_index() {
        let lang = JsonPathLanguage;
        let expr = lang.create_expression("$.items[0]").unwrap();
        let ex = exchange_with_json(r#"{"items":["a","b","c"]}"#);
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::String("a".to_string()));
    }

    #[test]
    fn expression_wildcard() {
        let lang = JsonPathLanguage;
        let expr = lang.create_expression("$.items[*].name").unwrap();
        let ex = exchange_with_json(r#"{"items":[{"name":"a"},{"name":"b"}]}"#);
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(
            result,
            JsonValue::Array(vec![
                JsonValue::String("a".to_string()),
                JsonValue::String("b".to_string())
            ])
        );
    }

    #[test]
    fn expression_root_path() {
        let lang = JsonPathLanguage;
        let expr = lang.create_expression("$").unwrap();
        let ex = exchange_with_json(r#"{"x":1}"#);
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result["x"], JsonValue::Number(1.into()));
    }

    #[test]
    fn expression_text_body_with_valid_json() {
        let lang = JsonPathLanguage;
        let expr = lang.create_expression("$.name").unwrap();
        let ex = exchange_with_text_body(r#"{"name":"test"}"#);
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result, JsonValue::String("test".to_string()));
    }

    #[test]
    fn expression_empty_body_is_error() {
        let lang = JsonPathLanguage;
        let expr = lang.create_expression("$.x").unwrap();
        let ex = empty_exchange();
        let result = expr.evaluate(&ex);
        assert!(result.is_err());
    }

    #[test]
    fn expression_invalid_jsonpath_syntax() {
        let lang = JsonPathLanguage;
        let result = lang.create_expression("$[invalid");
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
    fn predicate_non_empty_array_is_true() {
        let lang = JsonPathLanguage;
        let pred = lang.create_predicate("$.items[*]").unwrap();
        let ex = exchange_with_json(r#"{"items":[1,2,3]}"#);
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn predicate_empty_result_is_false() {
        let lang = JsonPathLanguage;
        let pred = lang.create_predicate("$.missing").unwrap();
        let ex = exchange_with_json(r#"{"other":1}"#);
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn predicate_boolean_true() {
        let lang = JsonPathLanguage;
        let pred = lang.create_predicate("$.active").unwrap();
        let ex = exchange_with_json(r#"{"active":true}"#);
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn predicate_boolean_false() {
        let lang = JsonPathLanguage;
        let pred = lang.create_predicate("$.active").unwrap();
        let ex = exchange_with_json(r#"{"active":false}"#);
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn predicate_found_value_is_true() {
        let lang = JsonPathLanguage;
        let pred = lang.create_predicate("$.name").unwrap();
        let ex = exchange_with_json(r#"{"name":"test"}"#);
        assert!(pred.matches(&ex).unwrap());
    }
}
