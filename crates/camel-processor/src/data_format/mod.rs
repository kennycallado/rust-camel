mod csv;
// `pub(crate)` so the TAR.GZ stream splitter can reuse the bounded
// single-member GZIP decoder without widening the public data-format API.
pub(crate) mod gzip;
mod json;
mod tar;
mod tar_gz;
#[cfg(test)]
mod test_util;
mod xml;
mod zip;

pub use csv::{CAMEL_CSV_HEADER_RECORD, CsvConfig, CsvDataFormat, QuoteMode, RecordSeparator};
pub use gzip::{GzipConfig, GzipDataFormat};
pub use json::{JsonConfig, JsonDataFormat};
pub use tar::{TarConfig, TarDataFormat};
pub use tar_gz::{TarGzConfig, TarGzDataFormat};
pub use xml::{XmlConfig, XmlDataFormat};
pub use zip::{ZipConfig, ZipDataFormat};

use camel_api::DataFormat;
use camel_api::body::Body;
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
        "tar" => Arc::new(TarDataFormat::new(parse_cfg::<TarConfig>(name, config)?)),
        "gzip" => Arc::new(GzipDataFormat::new(parse_cfg::<GzipConfig>(name, config)?)),
        "tar.gz" => Arc::new(TarGzDataFormat::new(parse_cfg::<TarGzConfig>(
            name, config,
        )?)),
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

/// Materializes the `marshal` input body into owned bytes, enforcing
/// `max_input_size` (DoS cap, R3-L1) after materialization. `df` names the
/// calling data format for error messages (e.g. `TarDataFormat`). `Text` and
/// `Xml` use their UTF-8 bytes, `Json` is serialized, `Bytes` is copied;
/// `Empty` and `Stream` fail closed, and streams are never consumed.
fn materialize_marshal_input(
    df: &str,
    body: &Body,
    max_input_size: u64,
) -> Result<Vec<u8>, CamelError> {
    let content: Vec<u8> = match body {
        Body::Text(s) => s.as_bytes().to_vec(),
        Body::Json(v) => serde_json::to_vec(v).map_err(|e| {
            CamelError::TypeConversionFailed(format!("{df}::marshal cannot serialize JSON: {e}"))
        })?,
        Body::Bytes(b) => b.to_vec(),
        Body::Xml(s) => s.as_bytes().to_vec(),
        Body::Empty => {
            return Err(CamelError::TypeConversionFailed(format!(
                "{df}::marshal requires non-empty body"
            )));
        }
        Body::Stream(_) => {
            return Err(CamelError::TypeConversionFailed(
                "cannot marshal Body::Stream — add 'stream_cache' or 'convert_body_to' before this step"
                    .to_string(),
            ));
        }
        _ => {
            return Err(CamelError::TypeConversionFailed(format!(
                "{df}::marshal does not support this body type"
            )));
        }
    };

    if content.len() as u64 > max_input_size {
        return Err(CamelError::TypeConversionFailed(format!(
            "{df}::marshal input {} bytes exceeds max_input_size {}",
            content.len(),
            max_input_size
        )));
    }
    Ok(content)
}

/// Extracts the raw wire bytes for `unmarshal` from a materialized body.
/// `df` names the calling data format and `wire` labels the expected payload
/// shape (e.g. `TAR data`) for error messages. `Bytes` and `Text` yield their
/// bytes; `Json` and `Xml` fail closed because they cannot be archive data;
/// `Empty` fails; `Stream` fails without being consumed.
fn raw_unmarshal_body(df: &str, wire: &str, body: &Body) -> Result<Vec<u8>, CamelError> {
    match body {
        Body::Bytes(b) => Ok(b.to_vec()),
        Body::Text(s) => Ok(s.as_bytes().to_vec()),
        Body::Empty => Err(CamelError::TypeConversionFailed(format!(
            "{df}::unmarshal requires non-empty body"
        ))),
        Body::Stream(_) => Err(CamelError::TypeConversionFailed(
            "cannot unmarshal Body::Stream — use UnmarshalService which auto-materializes"
                .to_string(),
        )),
        Body::Json(_) | Body::Xml(_) => Err(CamelError::TypeConversionFailed(format!(
            "{df}::unmarshal only supports Body::Bytes and Body::Text ({wire})"
        ))),
        _ => Err(CamelError::TypeConversionFailed(format!(
            "{df}::unmarshal does not support this body type"
        ))),
    }
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

    #[test]
    fn builtin_archive_formats_resolve() {
        for (name, expected) in [("tar", "tar"), ("gzip", "gzip"), ("tar.gz", "tar.gz")] {
            let df = builtin_data_format_with_config(name, &serde_json::Value::Null)
                .unwrap()
                .unwrap_or_else(|| panic!("'{name}' should resolve to a built-in format"));
            assert_eq!(df.name(), expected, "format name mismatch for '{name}'");
        }
    }

    #[test]
    fn builtin_archive_config_rejects_unknown_fields() {
        for name in ["tar", "gzip", "tar.gz"] {
            let config = serde_json::json!({"unknown_field": true});
            match builtin_data_format_with_config(name, &config) {
                Err(CamelError::RouteError(msg)) => {
                    assert!(
                        msg.contains("invalid config") && msg.contains(name),
                        "unexpected message for '{name}': {msg}"
                    );
                }
                Err(other) => panic!("expected RouteError for '{name}', got: {other:?}"),
                Ok(_) => panic!("unknown key for '{name}' should fail closed"),
            }
        }
    }

    #[test]
    fn archive_marshal_serializes_json_bodies() {
        // The shared marshal materialization serializes `Body::Json` instead
        // of rejecting it; every archive format must round trip it as bytes.
        let json = serde_json::json!({"payload": "text"});
        let expected = serde_json::to_vec(&json).unwrap();
        for name in ["tar", "gzip", "tar.gz", "zip"] {
            let df = builtin_data_format(name).unwrap();
            let restored = df
                .unmarshal(df.marshal(Body::Json(json.clone())).unwrap())
                .unwrap();
            match restored {
                Body::Bytes(b) => assert_eq!(&b[..], &expected[..], "format '{name}'"),
                other => panic!("format '{name}' must return bytes, got: {other:?}"),
            }
        }
    }

    #[test]
    fn archive_unmarshal_rejects_structured_bodies_with_format_named() {
        // The shared raw-body extraction keeps the calling format's name in
        // the rejection message for `Json`/`Xml` unmarshal input.
        for name in ["tar", "gzip", "tar.gz", "zip"] {
            let df = builtin_data_format(name).unwrap();
            let msg = format!(
                "{}",
                df.unmarshal(Body::Json(serde_json::json!({}))).unwrap_err()
            );
            assert!(
                msg.contains("::unmarshal only supports Body::Bytes and Body::Text"),
                "format '{name}' should name the supported body kinds: {msg}"
            );
        }
    }
}
