mod csv;
mod json;
mod xml;
mod zip;

pub use csv::{CAMEL_CSV_HEADER_RECORD, CsvConfig, CsvDataFormat, QuoteMode, RecordSeparator};
pub use json::{JsonConfig, JsonDataFormat};
pub use xml::{XmlConfig, XmlDataFormat};
pub use zip::{ZipConfig, ZipDataFormat};

use camel_api::DataFormat;
use camel_api::error::CamelError;
use std::sync::Arc;

/// Config-aware factory. `config` is the raw `config:` block from the DSL
/// (or `Null` for defaults). Each arm deserializes into its own typed config
/// with `deny_unknown_fields`, so a stray key fails closed with a precise message.
pub fn builtin_data_format_with_config(
    name: &str,
    config: &serde_json::Value,
) -> Result<Option<Arc<dyn DataFormat>>, CamelError> {
    let df: Arc<dyn DataFormat> = match name {
        "json" => Arc::new(JsonDataFormat::new(parse_cfg::<JsonConfig>(name, config)?)),
        "csv" => Arc::new(CsvDataFormat::new(parse_cfg::<CsvConfig>(name, config)?)),
        "xml" => Arc::new(XmlDataFormat::new(parse_cfg::<XmlConfig>(name, config)?)),
        "zip" => Arc::new(ZipDataFormat::new(parse_cfg::<ZipConfig>(name, config)?)),
        _ => return Ok(None),
    };
    Ok(Some(df))
}

/// Back-compat shim: existing callers keep working (config = Null → defaults).
pub fn builtin_data_format(name: &str) -> Option<Arc<dyn DataFormat>> {
    builtin_data_format_with_config(name, &serde_json::Value::Null)
        .ok()
        .flatten()
}

fn parse_cfg<T>(name: &str, v: &serde_json::Value) -> Result<T, CamelError>
where
    T: serde::de::DeserializeOwned + Default,
{
    if v.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value::<T>(v.clone()).map_err(|e| {
        CamelError::RouteError(format!("invalid config for data format '{name}': {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_json() {
        let df = builtin_data_format("json").unwrap();
        assert_eq!(df.name(), "json");
    }

    #[test]
    fn test_builtin_xml() {
        let df = builtin_data_format("xml").unwrap();
        assert_eq!(df.name(), "xml");
    }

    #[test]
    fn test_builtin_csv() {
        let csv_df = builtin_data_format("csv").unwrap();
        assert_eq!(csv_df.name(), "csv");
    }

    #[test]
    fn test_builtin_unknown_returns_none() {
        assert!(builtin_data_format("protobuf").is_none());
        assert!(builtin_data_format("").is_none());
    }

    #[test]
    fn test_builtin_json_with_config() {
        let config = serde_json::json!({"max_bytes": 67108864});
        let df = builtin_data_format_with_config("json", &config)
            .unwrap()
            .unwrap();
        assert_eq!(df.name(), "json");
    }

    #[test]
    fn test_builtin_json_with_null_config_returns_default() {
        let df = builtin_data_format_with_config("json", &serde_json::Value::Null)
            .unwrap()
            .unwrap();
        assert_eq!(df.name(), "json");
    }

    #[test]
    fn test_builtin_json_with_unknown_key_fails() {
        let config = serde_json::json!({"max_byte": 100});
        let result = builtin_data_format_with_config("json", &config);
        match result {
            Err(CamelError::RouteError(msg)) => {
                assert!(msg.contains("invalid config"), "msg: {msg}");
            }
            Err(other) => panic!("expected RouteError, got: {other:?}"),
            Ok(_) => panic!("typo should fail closed"),
        }
    }

    #[test]
    fn test_builtin_shim_still_works() {
        let df = builtin_data_format("json").unwrap();
        assert_eq!(df.name(), "json");
    }
}
