use camel_api::Value;
use camel_api::exchange::Exchange;
use camel_language_api::{Expression, Language, LanguageError, MutatingExpression, Predicate};
use rhai::{Engine, Scope};
use std::sync::{Arc, RwLock};

/// Default maximum number of Rhai operations per evaluation (prevents infinite loops).
const DEFAULT_MAX_OPERATIONS: u64 = 100_000;
/// Default maximum string size in bytes.
const DEFAULT_MAX_STRING_SIZE: usize = 1_048_576; // 1 MB
/// Default maximum array size.
const DEFAULT_MAX_ARRAY_SIZE: usize = 10_000;
/// Default maximum map size.
const DEFAULT_MAX_MAP_SIZE: usize = 10_000;

/// Rhai scripting language for rust-camel.
///
/// Scripts have access to:
/// - `body` — exchange body as a string variable
/// - `headers` — exchange headers as a Rhai Map
/// - `header(name)` — look up a header value by name
/// - `set_header(name, value)` — set a header (visible within the same script evaluation)
/// - `property(name)` — look up an exchange property by name
/// - `set_property(name, value)` — set a property (visible within the same script evaluation)
///
/// **Note:** `set_header` and `set_property` only affect values visible within the current
/// script evaluation. Changes do **not** propagate back to the Exchange because expressions
/// receive a read-only `&Exchange` reference.
///
/// ## Resource Limits
///
/// The engine enforces limits to prevent denial-of-service:
/// - Max operations: 100,000 (prevents infinite loops)
/// - Max string size: 1 MB
/// - Max array size: 10,000 elements
/// - Max map size: 10,000 entries
/// - Max expression depth: 64 (32 in functions)
pub struct RhaiLanguage {
    engine: Engine,
}

impl RhaiLanguage {
    pub fn new() -> Self {
        let engine = Self::create_base_engine();
        Self { engine }
    }

    /// Create a base engine with all resource limits configured.
    fn create_base_engine() -> Engine {
        let mut engine = Engine::new();
        engine.set_max_expr_depths(64, 32);
        engine.set_max_operations(DEFAULT_MAX_OPERATIONS);
        engine.set_max_string_size(DEFAULT_MAX_STRING_SIZE);
        engine.set_max_array_size(DEFAULT_MAX_ARRAY_SIZE);
        engine.set_max_map_size(DEFAULT_MAX_MAP_SIZE);
        engine
    }

    /// Build a scope with `body` and `headers` variables from the exchange.
    fn make_scope(exchange: &Exchange) -> (Scope<'static>, rhai::Map, rhai::Map) {
        let mut scope = Scope::new();
        let body = exchange.input.body.as_text().unwrap_or("").to_string();
        scope.push("body", body);

        let mut headers = rhai::Map::new();
        for (k, v) in &exchange.input.headers {
            headers.insert(k.clone().into(), json_to_dynamic(v));
        }
        scope.push("headers", headers.clone());

        let mut properties = rhai::Map::new();
        for (k, v) in &exchange.properties {
            properties.insert(k.clone().into(), json_to_dynamic(v));
        }

        (scope, headers, properties)
    }

    /// Create a fresh engine with `header()`, `set_header()`, `property()`, and
    /// `set_property()` registered as native functions. A new engine per eval
    /// avoids sharing mutable state between evaluations.
    fn create_eval_engine(headers: rhai::Map, properties: rhai::Map) -> Engine {
        let mut engine = Self::create_base_engine();

        // Shared mutable headers map: header() reads, set_header() writes
        let h = Arc::new(RwLock::new(headers));

        let h_read = h.clone();
        engine.register_fn("header", move |name: String| -> rhai::Dynamic {
            h_read
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(name.as_str())
                .cloned()
                .unwrap_or(rhai::Dynamic::UNIT)
        });

        let h_write = h.clone();
        engine.register_fn("set_header", move |name: String, value: rhai::Dynamic| {
            h_write
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name.into(), value);
        });

        // Shared mutable properties map: property() reads, set_property() writes
        let p = Arc::new(RwLock::new(properties));

        let p_read = p.clone();
        engine.register_fn("property", move |name: String| -> rhai::Dynamic {
            p_read
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(name.as_str())
                .cloned()
                .unwrap_or(rhai::Dynamic::UNIT)
        });

        let p_write = p.clone();
        engine.register_fn("set_property", move |name: String, value: rhai::Dynamic| {
            p_write
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name.into(), value);
        });

        engine
    }

    fn eval_to_value(script: &str, exchange: &Exchange) -> Result<Value, LanguageError> {
        let (mut scope, headers, properties) = Self::make_scope(exchange);
        let engine = Self::create_eval_engine(headers, properties);

        let result: rhai::Dynamic = engine
            .eval_with_scope::<rhai::Dynamic>(&mut scope, script)
            .map_err(|e| LanguageError::EvalError(e.to_string()))?;

        dynamic_to_json(result)
    }
}

/// Convert a serde_json::Value to a rhai::Dynamic.
fn json_to_dynamic(v: &Value) -> rhai::Dynamic {
    match v {
        Value::String(s) => rhai::Dynamic::from(s.clone()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rhai::Dynamic::from(i)
            } else if let Some(f) = n.as_f64() {
                rhai::Dynamic::from_float(f)
            } else {
                rhai::Dynamic::from(n.to_string())
            }
        }
        Value::Bool(b) => rhai::Dynamic::from(*b),
        Value::Null => rhai::Dynamic::UNIT,
        Value::Array(arr) => {
            let rhai_arr: rhai::Array = arr.iter().map(json_to_dynamic).collect();
            rhai::Dynamic::from(rhai_arr)
        }
        Value::Object(obj) => {
            let mut rhai_map = rhai::Map::new();
            for (k, v) in obj {
                rhai_map.insert(k.clone().into(), json_to_dynamic(v));
            }
            rhai::Dynamic::from(rhai_map)
        }
    }
}

fn dynamic_to_json(d: rhai::Dynamic) -> Result<Value, LanguageError> {
    if d.is_string() {
        Ok(Value::String(d.cast::<String>()))
    } else if d.is::<bool>() {
        Ok(Value::Bool(d.cast::<bool>()))
    } else if d.is_float() {
        Ok(Value::from(d.cast::<f64>()))
    } else if d.is_int() {
        Ok(Value::from(d.cast::<i64>()))
    } else if d.is_unit() {
        Ok(Value::Null)
    } else {
        Ok(Value::String(d.to_string()))
    }
}

/// Convert a rhai::Dynamic to a serde_json::Value (infallible version for mutating expressions).
fn dynamic_to_value(d: rhai::Dynamic) -> Value {
    if d.is_string() {
        Value::String(d.cast::<String>())
    } else if d.is::<bool>() {
        Value::Bool(d.cast::<bool>())
    } else if d.is_int() {
        Value::from(d.cast::<i64>())
    } else if d.is_float() {
        Value::from(d.cast::<f64>())
    } else if d.is_unit() {
        Value::Null
    } else {
        Value::String(d.to_string())
    }
}

/// Convert a Rhai Map to a HashMap<String, Value> for syncing back to exchange.
fn rhai_map_to_value_map(map: &rhai::Map) -> std::collections::HashMap<String, Value> {
    let mut result = std::collections::HashMap::new();
    for (k, v) in map {
        result.insert(k.to_string(), dynamic_to_value(v.clone()));
    }
    result
}

struct RhaiExpression {
    script: String,
}

struct RhaiPredicate {
    script: String,
}

impl Expression for RhaiExpression {
    fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        RhaiLanguage::eval_to_value(&self.script, exchange)
    }
}

impl Predicate for RhaiPredicate {
    fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let val = RhaiLanguage::eval_to_value(&self.script, exchange)?;
        Ok(match &val {
            Value::Bool(b) => *b,
            Value::Null => false,
            _ => true,
        })
    }
}

/// A Rhai script expression that can mutate the Exchange during evaluation.
///
/// The script has access to three mutable scope variables:
/// - `headers` — a Rhai map (`#{}`) representing the exchange headers
/// - `properties` — a Rhai map (`#{}`) representing the exchange properties
/// - `body` — a string representing the exchange body
///
/// Changes to these variables are propagated back to the Exchange after evaluation.
/// If evaluation fails, all changes are **rolled back atomically**.
///
/// # Note on API differences
///
/// Unlike non-mutating Rhai expressions, this engine does NOT provide
/// `header()`, `set_header()`, `property()`, or `set_property()` functions.
/// Use direct map assignment syntax instead:
///
/// ```rhai
/// headers["tenant"] = "acme";       // set header
/// properties["trace"] = "enabled";  // set property
/// body = "new content";             // set body
/// let v = headers["existing"];      // read header
/// ```
struct RhaiMutatingExpression {
    script: String,
}

impl MutatingExpression for RhaiMutatingExpression {
    fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
        // 1. Snapshot original state for rollback on error
        let original_headers = exchange.input.headers.clone();
        let original_properties = exchange.properties.clone();
        let original_body = exchange.input.body.clone();

        // 2. Create scope with Rhai Map variables
        let mut scope = Scope::new();

        let mut headers_map = rhai::Map::new();
        for (k, v) in &exchange.input.headers {
            headers_map.insert(k.clone().into(), json_to_dynamic(v));
        }

        let mut properties_map = rhai::Map::new();
        for (k, v) in &exchange.properties {
            properties_map.insert(k.clone().into(), json_to_dynamic(v));
        }

        let body_str = exchange.input.body.as_text().unwrap_or("").to_string();

        scope.push("headers", headers_map);
        scope.push("properties", properties_map);
        scope.push("body", body_str);

        // 3. Evaluate script
        let engine = RhaiLanguage::create_base_engine();

        let result: rhai::Dynamic = match engine.eval_with_scope(&mut scope, &self.script) {
            Ok(v) => v,
            Err(e) => {
                exchange.input.headers = original_headers;
                exchange.properties = original_properties;
                exchange.input.body = original_body;
                return Err(LanguageError::EvalError(e.to_string()));
            }
        };

        // 4. Sync changes back to exchange
        if let Some(h) = scope.get_value::<rhai::Map>("headers") {
            exchange.input.headers = rhai_map_to_value_map(&h);
        }
        if let Some(p) = scope.get_value::<rhai::Map>("properties") {
            exchange.properties = rhai_map_to_value_map(&p);
        }
        if let Some(b) = scope.get_value::<String>("body") {
            exchange.input.body = camel_api::Body::Text(b);
        }

        Ok(dynamic_to_value(result))
    }
}

impl Language for RhaiLanguage {
    fn name(&self) -> &'static str {
        "rhai"
    }

    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
        // Syntax-only validation — function resolution happens at eval time
        self.engine
            .compile(script)
            .map_err(|e| LanguageError::ParseError {
                expr: script.to_string(),
                reason: e.to_string(),
            })?;
        Ok(Box::new(RhaiExpression {
            script: script.to_string(),
        }))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        self.engine
            .compile(script)
            .map_err(|e| LanguageError::ParseError {
                expr: script.to_string(),
                reason: e.to_string(),
            })?;
        Ok(Box::new(RhaiPredicate {
            script: script.to_string(),
        }))
    }

    /// Create a mutating Rhai expression.
    ///
    /// The script can modify `headers`, `properties`, and `body` via assignment syntax.
    /// See [`RhaiMutatingExpression`] for full documentation.
    fn create_mutating_expression(
        &self,
        script: &str,
    ) -> Result<Box<dyn MutatingExpression>, LanguageError> {
        self.engine
            .compile(script)
            .map_err(|e| LanguageError::ParseError {
                expr: script.to_string(),
                reason: e.to_string(),
            })?;
        Ok(Box::new(RhaiMutatingExpression {
            script: script.to_string(),
        }))
    }
}

impl Default for RhaiLanguage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use camel_api::{Value, exchange::Exchange, message::Message};
    use camel_language_api::Language;

    use super::RhaiLanguage;

    fn exchange_with_header(key: &str, val: &str) -> Exchange {
        let mut msg = Message::default();
        msg.set_header(key, Value::String(val.to_string()));
        Exchange::new(msg)
    }

    fn exchange_with_body(body: &str) -> Exchange {
        Exchange::new(Message::new(body))
    }

    #[test]
    fn test_rhai_predicate_simple() {
        let lang = RhaiLanguage::new();
        let pred = lang
            .create_predicate(r#"header("type") == "order""#)
            .unwrap();
        let ex = exchange_with_header("type", "order");
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_rhai_predicate_false() {
        let lang = RhaiLanguage::new();
        let pred = lang
            .create_predicate(r#"header("type") == "order""#)
            .unwrap();
        let ex = exchange_with_header("type", "invoice");
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_rhai_expression_body() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("body").unwrap();
        let ex = exchange_with_body("hello");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("hello".to_string()));
    }

    #[test]
    fn test_rhai_expression_concat() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"body + " world""#).unwrap();
        let ex = exchange_with_body("hello");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("hello world".to_string()));
    }

    #[test]
    fn test_rhai_set_header_visible_within_script() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(
                r#"
            set_header("done", "yes");
            header("done")
        "#,
            )
            .unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("yes".to_string()));
    }

    #[test]
    fn test_rhai_property_access() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"property("myProp")"#).unwrap();
        let mut ex = exchange_with_body("test");
        ex.set_property("myProp".to_string(), Value::String("propVal".to_string()));
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("propVal".to_string()));
    }

    #[test]
    fn test_rhai_set_property_visible_within_script() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(
                r#"
            set_property("key", "value");
            property("key")
        "#,
            )
            .unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("value".to_string()));
    }

    #[test]
    fn test_rhai_missing_header_returns_null() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"header("nonexistent")"#).unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::Null);
    }

    #[test]
    fn test_rhai_syntax_error() {
        let lang = RhaiLanguage::new();
        let result = lang.create_expression("let x = ;");
        assert!(result.is_err());
    }

    #[test]
    fn test_rhai_runtime_error() {
        let lang = RhaiLanguage::new();
        // Calling a nonexistent function will produce a runtime error
        let expr = lang.create_expression(r#"nonexistent_fn()"#).unwrap();
        let ex = exchange_with_body("test");
        let result = expr.evaluate(&ex);
        assert!(result.is_err());
    }

    #[test]
    fn test_rhai_operations_limit_prevents_infinite_loop() {
        let lang = RhaiLanguage::new();
        // This script would loop forever without the operations limit
        let expr = lang
            .create_expression("let x = 0; loop { x += 1; } x")
            .unwrap();
        let ex = exchange_with_body("test");
        let result = expr.evaluate(&ex);
        assert!(result.is_err(), "should error due to operations limit");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("operations") || err_msg.contains("limit"),
            "error should mention operations limit, got: {err_msg}"
        );
    }

    #[test]
    fn test_rhai_empty_body() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("body").unwrap();
        let ex = Exchange::new(Message::default());
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("".to_string()));
    }

    #[test]
    fn test_rhai_numeric_header() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"header("count") + 1"#).unwrap();
        let mut msg = Message::default();
        msg.set_header("count", Value::Number(41.into()));
        let ex = Exchange::new(msg);
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::from(42));
    }

    #[test]
    fn test_rhai_json_array_header_is_native_array() {
        // json_to_dynamic should convert JSON arrays to native Rhai arrays,
        // not to their string representation.
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"header("items").len()"#).unwrap();
        let mut msg = Message::default();
        msg.set_header(
            "items",
            Value::Array(vec![
                Value::String("a".into()),
                Value::String("b".into()),
                Value::String("c".into()),
            ]),
        );
        let ex = Exchange::new(msg);
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(
            val,
            Value::from(3_i64),
            "array len should be 3, not a stringified value"
        );
    }

    #[test]
    fn test_rhai_json_object_header_is_native_map() {
        // json_to_dynamic should convert JSON objects to native Rhai maps.
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"header("obj")["key"]"#).unwrap();
        let mut msg = Message::default();
        // Build a JSON object via Value's FromStr impl (serde_json::Value)
        let obj: Value = r#"{"key": "value"}"#.parse().unwrap();
        msg.set_header("obj", obj);
        let ex = Exchange::new(msg);
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val, Value::String("value".to_string()));
    }

    #[test]
    fn test_mutating_set_header_propagates_to_exchange() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["tenant"] = "acme""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        expr.evaluate(&mut ex).unwrap();
        assert_eq!(
            ex.input.headers.get("tenant"),
            Some(&Value::String("acme".into()))
        );
    }

    #[test]
    fn test_mutating_set_body_propagates_to_exchange() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"body = "modified""#)
            .unwrap();
        let mut ex = Exchange::new(Message::new("original"));
        expr.evaluate(&mut ex).unwrap();
        assert_eq!(ex.input.body.as_text(), Some("modified"));
    }

    #[test]
    fn test_mutating_set_property_propagates_to_exchange() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"properties["auth"] = "ok""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        expr.evaluate(&mut ex).unwrap();
        assert_eq!(ex.properties.get("auth"), Some(&Value::String("ok".into())));
    }

    #[test]
    fn test_mutating_rollback_on_error() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["x"] = "modified"; throw "error""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input
            .headers
            .insert("x".to_string(), Value::String("original".into()));
        let result = expr.evaluate(&mut ex);
        assert!(result.is_err());
        assert_eq!(
            ex.input.headers.get("x"),
            Some(&Value::String("original".into()))
        );
    }

    #[test]
    fn test_mutating_rollback_on_error_includes_body() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"body = "modified"; throw "error""#)
            .unwrap();
        let mut ex = Exchange::new(Message::new("original"));
        let result = expr.evaluate(&mut ex);
        assert!(result.is_err());
        assert_eq!(ex.input.body.as_text(), Some("original"));
    }

    #[test]
    fn test_mutating_rollback_on_error_includes_property() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"properties["p"] = "modified"; throw "error""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.properties
            .insert("p".to_string(), Value::String("original".into()));
        let result = expr.evaluate(&mut ex);
        assert!(result.is_err());
        assert_eq!(
            ex.properties.get("p"),
            Some(&Value::String("original".into()))
        );
    }

    #[test]
    fn test_mutating_combined_read_write() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["out"] = headers["in"] + "_processed""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input
            .headers
            .insert("in".to_string(), Value::String("value".into()));
        expr.evaluate(&mut ex).unwrap();
        assert_eq!(
            ex.input.headers.get("out"),
            Some(&Value::String("value_processed".into()))
        );
    }
}
