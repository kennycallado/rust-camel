pub(crate) fn yaml_value_to_json_value(
    value: serde_yml::Value,
) -> Result<serde_json::Value, String> {
    convert_yaml_to_json(value)
}

fn convert_yaml_to_json(value: serde_yml::Value) -> Result<serde_json::Value, String> {
    match value {
        serde_yml::Value::Null => Ok(serde_json::Value::Null),
        serde_yml::Value::Bool(b) => Ok(serde_json::Value::Bool(b)),
        serde_yml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(serde_json::Value::Number(serde_json::Number::from(i)))
            } else if let Some(u) = n.as_u64() {
                Ok(serde_json::Value::Number(serde_json::Number::from(u)))
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| "invalid float number".to_string())
            } else {
                Err("unsupported number type".to_string())
            }
        }
        serde_yml::Value::String(s) => Ok(serde_json::Value::String(s)),
        serde_yml::Value::Sequence(seq) => {
            let items: Result<Vec<_>, _> = seq.into_iter().map(convert_yaml_to_json).collect();
            Ok(serde_json::Value::Array(items?))
        }
        serde_yml::Value::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                let key = k
                    .as_str()
                    .ok_or_else(|| "mapping key is not a string".to_string())?
                    .to_string();
                obj.insert(key, convert_yaml_to_json(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        serde_yml::Value::Tagged(tagged) => convert_yaml_to_json(tagged.value),
    }
}

#[cfg(test)]
mod tests {
    use super::yaml_value_to_json_value as convert;

    #[test]
    fn converts_null() {
        let result = convert(serde_yml::Value::Null).unwrap();
        assert!(result.is_null());
    }

    #[test]
    fn converts_bool() {
        let result = convert(serde_yml::Value::Bool(true)).unwrap();
        assert_eq!(result, serde_json::Value::Bool(true));
    }

    #[test]
    fn converts_i64_number() {
        let n = serde_yml::Value::Number(serde_yml::Number::from(-42i64));
        let result = convert(n).unwrap();
        assert_eq!(
            result,
            serde_json::Value::Number(serde_json::Number::from(-42i64))
        );
    }

    #[test]
    fn converts_u64_number() {
        let n = serde_yml::Value::Number(serde_yml::Number::from(u64::MAX));
        let result = convert(n).unwrap();
        assert_eq!(
            result,
            serde_json::Value::Number(serde_json::Number::from(u64::MAX))
        );
    }

    #[test]
    fn converts_f64_number() {
        let n = serde_yml::Value::Number(serde_yml::Number::from(3.14f64));
        let result = convert(n).unwrap();
        assert!(result.as_f64().unwrap() > 3.0);
    }

    #[test]
    fn converts_string() {
        let result = convert(serde_yml::Value::String("hello".into())).unwrap();
        assert_eq!(result, serde_json::Value::String("hello".into()));
    }

    #[test]
    fn converts_sequence() {
        let seq = serde_yml::Value::Sequence(vec![
            serde_yml::Value::Number(serde_yml::Number::from(1)),
            serde_yml::Value::String("two".into()),
        ]);
        let result = convert(seq).unwrap();
        assert_eq!(
            result[0],
            serde_json::Value::Number(serde_json::Number::from(1))
        );
        assert_eq!(result[1], serde_json::Value::String("two".into()));
    }

    #[test]
    fn converts_mapping() {
        let map = serde_yml::Mapping::from_iter(vec![(
            serde_yml::Value::String("key".into()),
            serde_yml::Value::Bool(false),
        )]);
        let result = convert(serde_yml::Value::Mapping(map)).unwrap();
        assert_eq!(result["key"], serde_json::Value::Bool(false));
    }

    #[test]
    fn converts_tagged_by_unwrapping_value() {
        let yaml_str = "!custom hello\n";
        let parsed: serde_yml::Value = serde_yml::from_str(yaml_str).unwrap();
        assert!(matches!(parsed, serde_yml::Value::Tagged(_)));
        let result = convert(parsed).unwrap();
        assert_eq!(result, serde_json::Value::String("hello".into()));
    }

    #[test]
    fn rejects_non_string_mapping_key() {
        let map = serde_yml::Mapping::from_iter(vec![(
            serde_yml::Value::Number(serde_yml::Number::from(42)),
            serde_yml::Value::Null,
        )]);
        let err = convert(serde_yml::Value::Mapping(map)).unwrap_err();
        assert!(err.contains("mapping key is not a string"));
    }

    #[test]
    fn rejects_invalid_float() {
        let n = serde_yml::Number::from(f64::NAN);
        let val = serde_yml::Value::Number(n);
        let err = convert(val).unwrap_err();
        assert!(err.contains("invalid float number"));
    }

    #[test]
    fn rejects_sequence_with_bad_element() {
        let map = serde_yml::Mapping::from_iter(vec![(
            serde_yml::Value::Number(serde_yml::Number::from(1)),
            serde_yml::Value::Null,
        )]);
        let seq = serde_yml::Value::Sequence(vec![serde_yml::Value::Mapping(map)]);
        let err = convert(seq).unwrap_err();
        assert!(err.contains("mapping key is not a string"));
    }
}
