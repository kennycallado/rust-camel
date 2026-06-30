mod csv;
mod json;
mod xml;
mod zip;

pub use csv::{CAMEL_CSV_HEADER_RECORD, CsvConfig, CsvDataFormat, QuoteMode, RecordSeparator};
pub use json::JsonDataFormat;
pub use xml::XmlDataFormat;
pub use zip::ZipDataFormat;

use camel_api::DataFormat;
use std::sync::Arc;

pub fn builtin_data_format(name: &str) -> Option<Arc<dyn DataFormat>> {
    match name {
        "csv" => Some(Arc::new(CsvDataFormat::default())),
        "json" => Some(Arc::new(JsonDataFormat)),
        "xml" => Some(Arc::new(XmlDataFormat)),
        "zip" => Some(Arc::new(ZipDataFormat::default())),
        _ => None,
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
}
