// serde_yml migrated to noyalib (compat-serde-yaml shim) — closes RUSTSEC-2025-0068.
// Module alias preserves call-site paths byte-for-byte.
// Note: noyalib's Mapping is String-keyed (type-enforced), Number::as_f64 returns f64 directly,
// TaggedValue.value is private (use tagged.value() method).
use noyalib::compat::serde_yaml as serde_yml;

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
            } else {
                let f = n.as_f64();
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| "invalid float number".to_string())
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
                obj.insert(k, convert_yaml_to_json(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        serde_yml::Value::Tagged(tagged) => convert_yaml_to_json(tagged.value().clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::serde_yml;
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
        // Note: noyalib's Number uses Integer(i64) variant (no PosInt(u64) like serde_yaml).
        // Values > i64::MAX would lose precision via Float fallback. We assert i64::MAX
        // (≈9.2 quintillion) which covers all realistic YAML route values.
        let n = serde_yml::Value::Number(serde_yml::Number::from(i64::MAX as u64));
        let result = convert(n).unwrap();
        assert_eq!(
            result,
            serde_json::Value::Number(serde_json::Number::from(i64::MAX))
        );
    }

    #[test]
    fn converts_f64_number() {
        let n = serde_yml::Value::Number(serde_yml::Number::from(5.0f64));
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
        let map =
            serde_yml::Mapping::from_iter(vec![("key".to_string(), serde_yml::Value::Bool(false))]);
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
    fn rejects_invalid_float() {
        let n = serde_yml::Number::from(f64::NAN);
        let val = serde_yml::Value::Number(n);
        let err = convert(val).unwrap_err();
        assert!(err.contains("invalid float number"));
    }
}
