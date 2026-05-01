//! Conversions between [`serde_json::Value`] and [`boa_engine::JsValue`].

use boa_engine::{Context, JsString, JsValue, object::builtins::JsArray};
use serde_json::{Map, Value};

use crate::error::JsLanguageError;

const MAX_DEPTH: u32 = 128;
const JS_MAX_SAFE_INT_I64: i64 = (1_i64 << 53) - 1;
const JS_MAX_SAFE_INT_F64: f64 = 9_007_199_254_740_991.0;

/// Convert a [`serde_json::Value`] into a [`boa_engine::JsValue`].
///
/// Requires a mutable `context` to allocate JS objects/arrays.
pub fn value_to_js(value: &Value, ctx: &mut Context) -> Result<JsValue, JsLanguageError> {
    value_to_js_inner(value, ctx, 0)
}

fn value_to_js_inner(
    value: &Value,
    ctx: &mut Context,
    depth: u32,
) -> Result<JsValue, JsLanguageError> {
    if depth > MAX_DEPTH {
        return Err(JsLanguageError::TypeConversion {
            message: format!("value nesting exceeds maximum depth of {MAX_DEPTH}"),
        });
    }

    match value {
        Value::Null => Ok(JsValue::null()),
        Value::Bool(b) => Ok(JsValue::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i >= i32::MIN as i64 && i <= i32::MAX as i64 {
                    Ok(JsValue::from(i as i32))
                } else if (-JS_MAX_SAFE_INT_I64..=JS_MAX_SAFE_INT_I64).contains(&i) {
                    Ok(JsValue::rational(i as f64))
                } else {
                    Err(JsLanguageError::TypeConversion {
                        message: format!(
                            "integer {i} is beyond JS safe integer range (±2^53-1); use a string or BigInt instead"
                        ),
                    })
                }
            } else if let Some(f) = n.as_f64() {
                Ok(JsValue::rational(f))
            } else {
                Err(JsLanguageError::TypeConversion {
                    message: format!("cannot convert number: {n}"),
                })
            }
        }
        Value::String(s) => Ok(JsValue::from(JsString::from(s.as_str()))),
        Value::Array(arr) => {
            let js_arr = JsArray::new(ctx);
            for (i, item) in arr.iter().enumerate() {
                let js_item = value_to_js_inner(item, ctx, depth + 1)?;
                js_arr.set(i as u32, js_item, false, ctx).map_err(|e| {
                    JsLanguageError::TypeConversion {
                        message: format!("array set error at {i}: {e}"),
                    }
                })?;
            }
            Ok(js_arr.into())
        }
        Value::Object(map) => {
            let obj = boa_engine::object::ObjectInitializer::new(ctx).build();
            for (k, v) in map {
                let js_val = value_to_js_inner(v, ctx, depth + 1)?;
                obj.set(boa_engine::JsString::from(k.as_str()), js_val, false, ctx)
                    .map_err(|e| JsLanguageError::TypeConversion {
                        message: format!("object set error for key '{k}': {e}"),
                    })?;
            }
            Ok(obj.into())
        }
    }
}

/// Convert a [`boa_engine::JsValue`] back into a [`serde_json::Value`].
///
/// Requires a mutable `context` to call JS methods (e.g., `toString`, array length).
pub fn js_to_value(value: &JsValue, ctx: &mut Context) -> Result<Value, JsLanguageError> {
    js_to_value_inner(value, ctx, 0)
}

fn js_to_value_inner(
    value: &JsValue,
    ctx: &mut Context,
    depth: u32,
) -> Result<Value, JsLanguageError> {
    if depth > MAX_DEPTH {
        return Err(JsLanguageError::TypeConversion {
            message: format!("JS value nesting exceeds maximum depth of {MAX_DEPTH}"),
        });
    }

    if value.is_null() || value.is_undefined() {
        return Ok(Value::Null);
    }
    if let Some(b) = value.as_boolean() {
        return Ok(Value::Bool(b));
    }
    if let Some(i) = value.as_i32() {
        return Ok(Value::Number(i.into()));
    }
    if let Some(f) = value.as_number() {
        if f.fract() == 0.0 && (-JS_MAX_SAFE_INT_F64..=JS_MAX_SAFE_INT_F64).contains(&f) {
            return Ok(Value::Number((f as i64).into()));
        }
        let n = serde_json::Number::from_f64(f).ok_or_else(|| JsLanguageError::TypeConversion {
            message: format!("non-finite float: {f}"),
        })?;
        return Ok(Value::Number(n));
    }
    if let Some(s) = value.as_string() {
        return Ok(Value::String(s.to_std_string_escaped()));
    }
    if value.is_symbol() {
        return Err(JsLanguageError::TypeConversion {
            message: "JS Symbol cannot be converted to serde_json::Value".to_string(),
        });
    }
    if value.is_bigint() {
        return Err(JsLanguageError::TypeConversion {
            message: "JS BigInt cannot be converted to serde_json::Value".to_string(),
        });
    }
    if let Some(obj) = value.as_object() {
        // Check if it's an Array
        if obj.is_array() {
            let js_arr =
                JsArray::from_object(obj.clone()).map_err(|e| JsLanguageError::TypeConversion {
                    message: format!("array conversion error: {e}"),
                })?;
            let len = js_arr
                .length(ctx)
                .map_err(|e| JsLanguageError::TypeConversion {
                    message: format!("array length error: {e}"),
                })?;
            let mut arr = Vec::with_capacity(len as usize);
            for i in 0..len {
                let item = js_arr
                    .get(i, ctx)
                    .map_err(|e| JsLanguageError::TypeConversion {
                        message: format!("array get error at {i}: {e}"),
                    })?;
                arr.push(js_to_value_inner(&item, ctx, depth + 1)?);
            }
            return Ok(Value::Array(arr));
        } else {
            // Plain object — enumerate own string keys
            let keys = obj
                .own_property_keys(ctx)
                .map_err(|e| JsLanguageError::TypeConversion {
                    message: format!("object keys error: {e}"),
                })?;
            let mut map = Map::new();
            for key in keys {
                let key_str = match &key {
                    boa_engine::property::PropertyKey::String(s) => s.to_std_string_escaped(),
                    boa_engine::property::PropertyKey::Index(i) => i.get().to_string(),
                    boa_engine::property::PropertyKey::Symbol(_) => continue, // skip symbols
                };
                let val = obj
                    .get(boa_engine::JsString::from(key_str.as_str()), ctx)
                    .map_err(|e| JsLanguageError::TypeConversion {
                        message: format!("object get error for key '{key_str}': {e}"),
                    })?;
                map.insert(key_str, js_to_value_inner(&val, ctx, depth + 1)?);
            }
            return Ok(Value::Object(map));
        }
    }
    unreachable!("unhandled JsValue type")
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;
    use serde_json::json;

    fn ctx() -> Context {
        Context::default()
    }

    #[test]
    fn test_null_roundtrip() {
        let mut ctx = ctx();
        let js = value_to_js(&Value::Null, &mut ctx).unwrap();
        assert!(js.is_null());
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert_eq!(back, Value::Null);
    }

    #[test]
    fn test_bool_roundtrip() {
        let mut ctx = ctx();
        for b in [true, false] {
            let v = json!(b);
            let js = value_to_js(&v, &mut ctx).unwrap();
            let back = js_to_value(&js, &mut ctx).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn test_integer_roundtrip() {
        let mut ctx = ctx();
        let v = json!(42);
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert_eq!(back.as_i64().unwrap(), 42);
    }

    #[test]
    fn test_large_integer_no_truncation() {
        let mut ctx = ctx();
        let v = json!(2147483648i64); // 2^31, beyond i32::MAX
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert_eq!(back.as_i64().unwrap(), 2147483648i64);
    }

    #[test]
    fn test_negative_integer() {
        let mut ctx = ctx();
        let v = json!(-42);
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert_eq!(back.as_i64().unwrap(), -42);
    }

    #[test]
    fn test_float_roundtrip() {
        let mut ctx = ctx();
        let v = json!(PI);
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert!((back.as_f64().unwrap() - PI).abs() < 1e-10);
    }

    #[test]
    fn test_string_roundtrip() {
        let mut ctx = ctx();
        let v = json!("hello world");
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn test_array_roundtrip() {
        let mut ctx = ctx();
        let v = json!([1, "two", true, null]);
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert_eq!(back[0].as_i64().unwrap(), 1);
        assert_eq!(back[1].as_str().unwrap(), "two");
        assert!(back[2].as_bool().unwrap());
        assert!(back[3].is_null());
    }

    #[test]
    fn test_empty_array() {
        let mut ctx = ctx();
        let v = json!([]);
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert!(back.as_array().unwrap().is_empty());
    }

    #[test]
    fn test_object_roundtrip() {
        let mut ctx = ctx();
        let v = json!({"name": "test", "count": 3, "active": true});
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert_eq!(back["name"].as_str().unwrap(), "test");
        assert_eq!(back["count"].as_i64().unwrap(), 3);
        assert!(back["active"].as_bool().unwrap());
    }

    #[test]
    fn test_empty_object() {
        let mut ctx = ctx();
        let v = json!({});
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert!(back.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_nested_object_roundtrip() {
        let mut ctx = ctx();
        let v = json!({"outer": {"inner": [1, 2, 3]}});
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert_eq!(back["outer"]["inner"][1].as_i64().unwrap(), 2);
    }

    #[test]
    fn test_depth_limit() {
        let mut ctx = ctx();
        // Build a deeply nested object (200 levels)
        let mut v = json!("leaf");
        for _ in 0..200 {
            v = json!({"inner": v});
        }
        let result = value_to_js(&v, &mut ctx);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("depth"));
    }

    #[test]
    fn test_beyond_safe_integer_errors() {
        let mut ctx = ctx();
        let v = json!(9007199254740993i64); // 2^53 + 1

        let result = value_to_js(&v, &mut ctx);

        assert!(result.is_err(), "should error for beyond-safe-integer");
        assert!(result.unwrap_err().to_string().contains("safe integer"));
    }

    #[test]
    fn test_safe_integer_boundary() {
        let mut ctx = ctx();
        let v = json!(9007199254740991i64); // 2^53 - 1

        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();

        assert_eq!(back.as_i64().unwrap(), 9007199254740991i64);
    }

    #[test]
    fn test_safe_integer_negative_boundary() {
        let mut ctx = ctx();
        let v = json!(-9007199254740991i64); // exactly -(2^53 - 1), safe
        let js = value_to_js(&v, &mut ctx).unwrap();
        let back = js_to_value(&js, &mut ctx).unwrap();
        assert_eq!(back.as_i64().unwrap(), -9007199254740991i64);
    }

    #[test]
    fn test_beyond_safe_integer_negative_errors() {
        let mut ctx = ctx();
        let v = json!(-9007199254740993i64); // -(2^53 + 1), unsafe
        let result = value_to_js(&v, &mut ctx);
        assert!(
            result.is_err(),
            "should error for negative beyond-safe-integer"
        );
        assert!(result.unwrap_err().to_string().contains("safe integer"));
    }
}
