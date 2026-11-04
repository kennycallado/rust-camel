//! Fallible boundary conversions between exchange [`Value`]s and
//! `rhai::Dynamic` (change `language-value-boundary`, task 2.1).
//!
//! The converters are the single inbound (`json_to_dynamic`), outbound
//! (`dynamic_to_value`) and mutating write-back (`rhai_map_to_value_map`)
//! boundary. Each is fallible: a value the rhai engine cannot represent is
//! refused with a typed [`LanguageError::ConversionError`] naming a GENERIC
//! destination label — never runtime exchange data.

use camel_language_api::{LanguageError, Value};

use crate::json::JsonNumber;

/// Convert a serde_json Value into a rhai::Dynamic — the single, fallible
/// inbound boundary conversion (change `language-value-boundary`, task 2.1).
///
/// JSON integers in `(i64::MAX, u64::MAX]` bind as the `json number` wrapper
/// carrying their exact decimal token on every inbound path (bd rc-141om;
/// refines owner decision 2026-10-02, sealed Q4: no lossy float). The
/// explicit lossy path is `x.to_float()`.
///
/// `target` is a GENERIC destination label (`"value"`, `"body"`, `"header
/// entry"`, `"property entry"`). Recursion passes it through UNCHANGED: the
/// carrier rewrites it to the trusted `EvalMeta.target` for read-only verbs,
/// and it never carries runtime exchange data.
pub(crate) fn json_to_dynamic(v: &Value, target: &str) -> Result<rhai::Dynamic, LanguageError> {
    let refused = |source_type: &str| LanguageError::ConversionError {
        source_type: source_type.to_string(),
        target: target.to_string(),
    };
    Ok(match v {
        Value::String(s) => rhai::Dynamic::from(s.clone()),
        Value::Bool(b) => rhai::Dynamic::from(*b),
        Value::Null => rhai::Dynamic::UNIT,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rhai::Dynamic::from(i)
            } else if n.as_u64().is_some() {
                // Only u64 > i64::MAX reaches here (as_i64 already missed).
                // serde_json renders a u64-backed number as the exact plain
                // decimal, so the wrapper carries the exact inbound image.
                rhai::Dynamic::from(JsonNumber::new(n.to_string()))
            } else if let Some(f) = n.as_f64() {
                debug_assert!(f.is_finite(), "valid JSON numbers are finite");
                rhai::Dynamic::from_float(f)
            } else {
                return Err(refused("number"));
            }
        }
        Value::Array(arr) => {
            let mut rhai_arr = rhai::Array::new();
            for item in arr {
                rhai_arr.push(json_to_dynamic(item, target)?);
            }
            rhai::Dynamic::from(rhai_arr)
        }
        Value::Object(obj) => {
            let mut rhai_map = rhai::Map::new();
            for (k, val) in obj {
                rhai_map.insert(k.clone().into(), json_to_dynamic(val, target)?);
            }
            rhai::Dynamic::from(rhai_map)
        }
    })
}

/// Convert a rhai::Dynamic into a serde_json Value — the single, fallible
/// outbound boundary conversion (change `language-value-boundary`, task 2.1).
///
/// Values the engine cannot represent natively (non-finite floats, function
/// pointers, timestamps, custom types) are refused with a typed conversion
/// error. There is deliberately NO `to_string()` fallback.
///
/// `target` follows the same generic-label discipline as [`json_to_dynamic`]:
/// recursion passes it through UNCHANGED because nested keys are runtime data
/// and must never be appended to the trusted target.
pub(crate) fn dynamic_to_value(d: rhai::Dynamic, target: &str) -> Result<Value, LanguageError> {
    let refused = |source_type: &str| LanguageError::ConversionError {
        source_type: source_type.to_string(),
        target: target.to_string(),
    };
    if d.is_string() {
        Ok(Value::String(d.cast::<String>()))
    } else if d.is::<bool>() {
        Ok(Value::Bool(d.cast::<bool>()))
    } else if d.is_int() {
        Ok(Value::from(d.cast::<i64>()))
    } else if d.is_float() {
        let f = d.cast::<f64>();
        if f.is_finite() {
            Ok(Value::from(f))
        } else {
            Err(refused("float (non-finite)"))
        }
    } else if d.is_unit() {
        Ok(Value::Null)
    } else if d.is::<rhai::Map>() {
        let map: rhai::Map = d.cast();
        let entries: Result<Vec<(String, Value)>, LanguageError> = map
            .into_iter()
            .map(|(k, v)| dynamic_to_value(v, target).map(|val| (k.to_string(), val)))
            .collect();
        Ok(Value::Object(entries?.into_iter().collect()))
    } else if d.is::<rhai::Array>() {
        let arr: rhai::Array = d.cast();
        let items: Result<Vec<Value>, LanguageError> = arr
            .into_iter()
            .map(|v| dynamic_to_value(v, target))
            .collect();
        Ok(Value::Array(items?))
    } else if d.is::<char>() {
        let c = d.cast::<char>();
        Ok(Value::String(c.to_string()))
    } else if d.is::<rhai::Blob>() {
        let blob: rhai::Blob = d.cast();
        Ok(Value::Array(blob.into_iter().map(Value::from).collect()))
    } else if d.is::<crate::stream_body::StreamBodyRef>() {
        // Defense-in-depth (task 2.2): a stream marker is a refusal handle,
        // not data — it must never cross the outbound boundary, even if a
        // post-eval counter race let it escape into a nested result.
        Err(refused("Body::Stream"))
    } else if d.is::<rhai::FnPtr>() {
        Err(refused("FnPtr"))
    } else if d.is::<std::time::Instant>() {
        // rhai timestamps are `std::time::Instant` (the workspace does not
        // enable `no_time`), so this guard is live code, not dead armor.
        Err(refused("timestamp"))
    } else if d.is::<crate::json::JsonValue>() {
        // Task 1.5 outbound refusal: direct wrapper conversion stays deferred
        // and reports the stable label, never a Rust type path.
        Err(refused(crate::json::JSON_VALUE_TYPE_NAME))
    } else if d.is::<JsonNumber>() {
        // Only the exact inbound image converts; every other token keeps the
        // stable `json number` refusal (rc-qka42 deferral).
        let token = d.cast::<JsonNumber>();
        match crate::json::as_u64_above_i64(token.as_str()) {
            Some(u) => Ok(Value::from(u)),
            None => Err(refused(crate::json::JSON_NUMBER_TYPE_NAME)),
        }
    } else {
        Err(refused(d.type_name()))
    }
}

/// Convert a Rhai Map into a HashMap<String, Value> for syncing back to the
/// exchange (mutating transaction write-back).
///
/// `target` stays GENERIC ("header entry" / "property entry") because the
/// map's keys are runtime data and must never enter the error target.
pub(crate) fn rhai_map_to_value_map(
    map: &rhai::Map,
    target: &str,
) -> Result<std::collections::HashMap<String, Value>, LanguageError> {
    let mut result = std::collections::HashMap::new();
    for (k, v) in map {
        result.insert(k.to_string(), dynamic_to_value(v.clone(), target)?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::JsonNumber;
    use super::{dynamic_to_value, json_to_dynamic};
    use camel_language_api::{LanguageError, Value};

    fn sample_map_dynamic() -> rhai::Dynamic {
        let inner: rhai::Array = vec![rhai::Dynamic::from(2_i64), rhai::Dynamic::from(3_i64)];
        let mut map = rhai::Map::new();
        map.insert("a".into(), rhai::Dynamic::from(1_i64));
        map.insert("b".into(), rhai::Dynamic::from(inner));
        rhai::Dynamic::from(map)
    }

    /// GH#62 repro 2 core: a rhai map must become a JSON object (recursively),
    /// never a debug string.
    #[test]
    fn map_round_trips_to_object_property() {
        let d = sample_map_dynamic();
        let v = dynamic_to_value(d.clone(), "property m").expect("map converts");
        let obj = v.as_object().expect("map must convert to a JSON object");
        assert_eq!(obj.get("a"), Some(&Value::from(1)));
        assert_eq!(
            obj.get("b"),
            Some(&Value::Array(vec![Value::from(2), Value::from(3)]))
        );
        let back = json_to_dynamic(&v, "property m").expect("object converts back");
        let back_v = dynamic_to_value(back, "property m").expect("back-conversion");
        assert_eq!(back_v, v, "round-trip must preserve the map");
    }

    #[test]
    fn nan_refused_with_target() {
        let err = dynamic_to_value(rhai::Dynamic::from(f64::NAN), "property x")
            .expect_err("NaN must be refused");
        match err {
            LanguageError::ConversionError {
                source_type,
                target,
            } => {
                assert_eq!(source_type, "float (non-finite)");
                assert_eq!(target, "property x");
            }
            other => panic!("expected ConversionError, got {other:?}"),
        }
    }

    #[test]
    fn fnptr_refused() {
        let fn_ptr = rhai::FnPtr::new("square").expect("valid fn name");
        let d = rhai::Dynamic::from(fn_ptr);
        let err = dynamic_to_value(d, "value").expect_err("FnPtr must be refused");
        match err {
            LanguageError::ConversionError {
                source_type,
                target,
            } => {
                assert_eq!(source_type, "FnPtr");
                assert_eq!(target, "value");
            }
            other => panic!("expected ConversionError, got {other:?}"),
        }
    }

    #[test]
    fn blob_becomes_byte_array() {
        let blob: rhai::Blob = vec![104_u8, 105];
        let v = dynamic_to_value(rhai::Dynamic::from(blob), "value").expect("blob converts");
        assert_eq!(v, Value::Array(vec![Value::from(104), Value::from(105)]));
    }

    #[test]
    fn char_becomes_one_char_string() {
        let v = dynamic_to_value(rhai::Dynamic::from('x'), "value").expect("char converts");
        assert_eq!(v, Value::String("x".to_string()));
    }

    #[test]
    fn json_value_refused_outbound_stable_label() {
        let d = rhai::Dynamic::from(crate::json::JsonValue::new(crate::json::JsonKind::Null));
        let err = dynamic_to_value(d, "value").expect_err("wrapper must be refused");
        match err {
            LanguageError::ConversionError {
                source_type,
                target,
            } => {
                assert_eq!(source_type, "json value");
                assert_eq!(target, "value");
            }
            other => panic!("expected ConversionError, got {other:?}"),
        }
    }

    #[test]
    fn json_number_non_u64_token_still_refused_outbound() {
        for token in ["1.5", "5", "18446744073709551616", "09223372036854775808"] {
            let d = rhai::Dynamic::from(JsonNumber::new(token));
            let err = dynamic_to_value(d, "value").expect_err("token must be refused");
            match err {
                LanguageError::ConversionError {
                    source_type,
                    target,
                } => {
                    assert_eq!(source_type, "json number");
                    assert_eq!(target, "value");
                }
                other => panic!("expected ConversionError, got {other:?}"),
            }
        }
    }

    #[test]
    fn json_number_u64_round_trips_outbound() {
        let inbound = json_to_dynamic(&Value::from(u64::MAX), "value").expect("inbound binds");
        let out = dynamic_to_value(inbound, "value").expect("exact image converts");
        assert_eq!(out, Value::from(u64::MAX));
        assert!(out.is_u64());
    }

    #[test]
    fn u64_above_i64_max_binds_json_number_inbound() {
        let v = Value::from(u64::MAX);
        let d = json_to_dynamic(&v, "property big").expect("huge u64 binds");
        assert!(d.is::<JsonNumber>());
        let n = d.cast::<JsonNumber>();
        assert_eq!(n.as_str(), "18446744073709551615");
    }

    #[test]
    fn empty_containers_and_unicode_keys() {
        // Empty map -> empty object -> empty map.
        let empty_map = rhai::Dynamic::from(rhai::Map::new());
        let v = dynamic_to_value(empty_map, "value").expect("empty map converts");
        assert!(v.as_object().expect("empty map -> object").is_empty());
        let back = json_to_dynamic(&v, "value").unwrap();
        let back_v = dynamic_to_value(back, "value").unwrap();
        assert_eq!(back_v, v);

        // Empty array -> empty array -> empty array.
        let empty_arr = rhai::Dynamic::from(rhai::Array::new());
        let v = dynamic_to_value(empty_arr, "value").expect("empty array converts");
        assert!(
            v.as_array()
                .expect("empty rhai array -> JSON array")
                .is_empty()
        );
        let back = json_to_dynamic(&v, "value").unwrap();
        let back_v = dynamic_to_value(back, "value").unwrap();
        assert_eq!(back_v, v);

        // Unicode keys survive the round-trip.
        let mut cafe = rhai::Map::new();
        cafe.insert("café".into(), rhai::Dynamic::from(1_i64));
        let d = rhai::Dynamic::from(cafe);
        let v = dynamic_to_value(d, "value").expect("unicode map converts");
        assert!(v.as_object().expect("object").contains_key("café"));
        let back = json_to_dynamic(&v, "value").unwrap();
        let back_v = dynamic_to_value(back, "value").unwrap();
        assert_eq!(back_v, v);
    }
}
