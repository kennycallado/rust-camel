use camel_api::Exchange;
use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;

pub const CAMEL_CSV_HEADER_RECORD: &str = "CamelCsvHeaderRecord";

#[derive(Clone, Copy, Debug, Default)]
pub enum RecordSeparator {
    #[default]
    Crlf,
    Lf,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum QuoteMode {
    All,
    #[default]
    Minimal,
    NonNumeric,
    None,
}

#[derive(Clone, Debug)]
pub struct CsvConfig {
    pub delimiter: char,
    pub quote: char,
    pub quote_mode: QuoteMode,
    pub double_quote: bool,
    pub escape: Option<char>,
    pub record_separator: RecordSeparator,
    pub comment_marker: Option<char>,
    pub null_string: Option<String>,
    pub headers: Option<Vec<String>>,
    pub has_headers: bool,
    pub skip_header_record: bool,
    pub capture_header_record: bool,
    pub ignore_surrounding_spaces: bool,
    pub trim: bool,
    pub use_maps: bool,
}

impl Default for CsvConfig {
    fn default() -> Self {
        Self {
            delimiter: ',',
            quote: '"',
            quote_mode: QuoteMode::Minimal,
            double_quote: true,
            escape: None,
            record_separator: RecordSeparator::Crlf,
            comment_marker: None,
            null_string: None,
            headers: None,
            has_headers: true,
            skip_header_record: false,
            capture_header_record: false,
            ignore_surrounding_spaces: true,
            trim: false,
            use_maps: true,
        }
    }
}

impl CsvConfig {
    pub fn excel() -> Self {
        Self::default()
    }

    pub fn tdf() -> Self {
        Self {
            delimiter: '\t',
            record_separator: RecordSeparator::Lf,
            ..Self::default()
        }
    }

    pub fn mysql() -> Self {
        Self {
            delimiter: '\t',
            quote: '\0',
            record_separator: RecordSeparator::Lf,
            escape: Some('\\'),
            ..Self::default()
        }
    }

    pub fn delimiter(mut self, c: char) -> Self {
        self.delimiter = c;
        self
    }
    pub fn quote(mut self, c: char) -> Self {
        self.quote = c;
        self
    }
    pub fn quote_mode(mut self, m: QuoteMode) -> Self {
        self.quote_mode = m;
        self
    }
    pub fn double_quote(mut self, b: bool) -> Self {
        self.double_quote = b;
        self
    }
    pub fn escape(mut self, c: Option<char>) -> Self {
        self.escape = c;
        self
    }
    pub fn record_separator(mut self, s: RecordSeparator) -> Self {
        self.record_separator = s;
        self
    }
    pub fn comment_marker(mut self, c: Option<char>) -> Self {
        self.comment_marker = c;
        self
    }
    pub fn null_string(mut self, s: Option<String>) -> Self {
        self.null_string = s;
        self
    }
    pub fn headers(mut self, h: Vec<String>) -> Self {
        self.headers = Some(h);
        self
    }
    pub fn has_headers(mut self, b: bool) -> Self {
        self.has_headers = b;
        self
    }
    pub fn skip_header_record(mut self, b: bool) -> Self {
        self.skip_header_record = b;
        self
    }
    pub fn capture_header_record(mut self, b: bool) -> Self {
        self.capture_header_record = b;
        self
    }
    pub fn ignore_surrounding_spaces(mut self, b: bool) -> Self {
        self.ignore_surrounding_spaces = b;
        self
    }
    pub fn trim(mut self, b: bool) -> Self {
        self.trim = b;
        self
    }
    pub fn use_maps(mut self, b: bool) -> Self {
        self.use_maps = b;
        self
    }
}

#[derive(Clone, Default)]
pub struct CsvDataFormat {
    config: CsvConfig,
}

impl CsvDataFormat {
    pub fn new(config: CsvConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &CsvConfig {
        &self.config
    }

    /// Render a single JSON value as a CSV field string.
    fn field_to_string(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Null => String::new(),
            serde_json::Value::Array(arr) => arr
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(","),
            serde_json::Value::Object(_) => value.to_string(),
        }
    }

    fn write_err(e: csv::Error) -> CamelError {
        CamelError::TypeConversionFailed(format!("CSV write error: {e}"))
    }

    fn unmarshal_internal(&self, body: Body) -> Result<(Body, Option<Vec<String>>), CamelError> {
        let input_text: String =
            match body {
                Body::Text(s) => s,
                Body::Bytes(b) => String::from_utf8(b.to_vec()).map_err(|e| {
                    CamelError::TypeConversionFailed(format!("CSV bytes not UTF-8: {e}"))
                })?,
                Body::Json(v) => return Ok((Body::Json(v), None)),
                Body::Stream(_) => return Err(CamelError::TypeConversionFailed(
                    "cannot unmarshal Body::Stream directly — add 'stream_cache' before this step"
                        .into(),
                )),
                Body::Empty => {
                    return Err(CamelError::TypeConversionFailed(
                        "CsvDataFormat::unmarshal expects Body::Text or Body::Bytes".into(),
                    ));
                }
                Body::Xml(_) => {
                    return Err(CamelError::TypeConversionFailed(
                        "CsvDataFormat::unmarshal does not accept Body::Xml".into(),
                    ));
                }
            };

        let mut rdr_builder = csv::ReaderBuilder::new();
        let effective_has_headers = if self.config.headers.is_some() {
            false
        } else {
            self.config.has_headers
        };
        rdr_builder
            .delimiter(self.config.delimiter as u8)
            .has_headers(effective_has_headers)
            .trim(if self.config.trim {
                csv::Trim::All
            } else if self.config.ignore_surrounding_spaces {
                csv::Trim::Fields
            } else {
                csv::Trim::None
            });
        if let Some(c) = self.config.comment_marker {
            rdr_builder.comment(Some(c as u8));
        }
        let mut rdr = rdr_builder.from_reader(input_text.as_bytes());

        let header_keys: Option<Vec<String>> = if let Some(h) = &self.config.headers {
            Some(h.clone())
        } else if self.config.has_headers {
            Some(
                rdr.headers()
                    .map_err(|e| CamelError::TypeConversionFailed(format!("CSV header read: {e}")))?
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            )
        } else {
            None
        };

        let mut records: Vec<serde_json::Value> = Vec::new();
        if self.config.use_maps {
            let keys = match &header_keys {
                Some(k) => k.clone(),
                None => {
                    return Err(CamelError::TypeConversionFailed(
                        "CsvDataFormat::unmarshal with use_maps=true requires has_headers=true or configured headers".into(),
                    ));
                }
            };
            let mut iter = rdr.records();
            if self.config.skip_header_record && self.config.headers.is_some() {
                iter.next();
            }
            for result in iter {
                let record = result
                    .map_err(|e| CamelError::TypeConversionFailed(format!("CSV parse: {e}")))?;
                let mut obj = serde_json::Map::new();
                for (i, key) in keys.iter().enumerate() {
                    let val = record.get(i).unwrap_or("");
                    let parsed = if let Some(ns) = &self.config.null_string {
                        if val == ns {
                            serde_json::Value::Null
                        } else {
                            serde_json::Value::String(val.to_string())
                        }
                    } else {
                        serde_json::Value::String(val.to_string())
                    };
                    obj.insert(key.clone(), parsed);
                }
                records.push(serde_json::Value::Object(obj));
            }
        } else {
            for result in rdr.records() {
                let record = result
                    .map_err(|e| CamelError::TypeConversionFailed(format!("CSV parse: {e}")))?;
                let arr: Vec<serde_json::Value> = record
                    .iter()
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect();
                records.push(serde_json::Value::Array(arr));
            }
        }

        Ok((Body::Json(serde_json::Value::Array(records)), header_keys))
    }
}

impl DataFormat for CsvDataFormat {
    fn name(&self) -> &str {
        "csv"
    }

    fn marshal(&self, body: Body) -> Result<Body, CamelError> {
        let json = match body {
            Body::Json(v) => v,
            Body::Text(_) => {
                return Err(CamelError::TypeConversionFailed(
                    "CsvDataFormat::marshal expects Body::Json; use convert_body_to first".into(),
                ));
            }
            Body::Bytes(_) => {
                return Err(CamelError::TypeConversionFailed(
                    "CsvDataFormat::marshal expects Body::Json; use convert_body_to first".into(),
                ));
            }
            Body::Stream(_) => {
                return Err(CamelError::TypeConversionFailed(
                    "cannot marshal Body::Stream — add 'stream_cache' before this step".into(),
                ));
            }
            Body::Empty => {
                return Err(CamelError::TypeConversionFailed(
                    "CsvDataFormat::marshal expects Body::Json".into(),
                ));
            }
            Body::Xml(_) => return Err(CamelError::TypeConversionFailed(
                "CsvDataFormat::marshal does not accept Body::Xml — use unmarshal(\"xml\") first"
                    .into(),
            )),
        };

        let mut wtr_builder = csv::WriterBuilder::new();
        wtr_builder
            .delimiter(self.config.delimiter as u8)
            .quote(self.config.quote as u8)
            .double_quote(self.config.double_quote);
        if let Some(esc) = self.config.escape {
            wtr_builder.escape(esc as u8);
        }
        let quote_style = match self.config.quote_mode {
            QuoteMode::All => csv::QuoteStyle::Always,
            QuoteMode::Minimal => csv::QuoteStyle::Necessary,
            QuoteMode::NonNumeric => csv::QuoteStyle::NonNumeric,
            QuoteMode::None => csv::QuoteStyle::Never,
        };
        wtr_builder.quote_style(quote_style);
        let terminator = match self.config.record_separator {
            RecordSeparator::Crlf => csv::Terminator::CRLF,
            RecordSeparator::Lf => csv::Terminator::Any(b'\n'),
        };
        wtr_builder.terminator(terminator);

        let mut output = Vec::new();
        {
            let mut wtr = wtr_builder.from_writer(&mut output);
            match &json {
                serde_json::Value::Array(arr) => {
                    if arr.is_empty() {
                        if let (true, Some(h)) = (self.config.has_headers, &self.config.headers) {
                            wtr.write_record(h.iter().map(String::as_str))
                                .map_err(Self::write_err)?;
                        }
                    } else {
                        match &arr[0] {
                            serde_json::Value::Object(first_obj) => {
                                let header_keys: Vec<String> = self
                                    .config
                                    .headers
                                    .clone()
                                    .unwrap_or_else(|| first_obj.keys().cloned().collect());
                                if self.config.has_headers {
                                    wtr.write_record(header_keys.iter().map(String::as_str))
                                        .map_err(Self::write_err)?;
                                }
                                for row in arr {
                                    if let Some(obj) = row.as_object() {
                                        let record: Vec<String> = header_keys
                                            .iter()
                                            .map(|k| {
                                                Self::field_to_string(
                                                    obj.get(k).unwrap_or(&serde_json::Value::Null),
                                                )
                                            })
                                            .collect();
                                        wtr.write_record(record.iter().map(String::as_str))
                                            .map_err(Self::write_err)?;
                                    } else {
                                        return Err(CamelError::TypeConversionFailed(format!(
                                            "CsvDataFormat::marshal expected all array elements to be objects, got {:?}",
                                            row
                                        )));
                                    }
                                }
                            }
                            _ => {
                                for row in arr {
                                    if let Some(items) = row.as_array() {
                                        let record: Vec<String> =
                                            items.iter().map(Self::field_to_string).collect();
                                        wtr.write_record(record.iter().map(String::as_str))
                                            .map_err(Self::write_err)?;
                                    } else {
                                        return Err(CamelError::TypeConversionFailed(format!(
                                            "CsvDataFormat::marshal expected all array elements to be arrays, got {:?}",
                                            row
                                        )));
                                    }
                                }
                            }
                        }
                    }
                }
                serde_json::Value::Object(obj) => {
                    let header_keys: Vec<String> = self
                        .config
                        .headers
                        .clone()
                        .unwrap_or_else(|| obj.keys().cloned().collect());
                    if self.config.has_headers {
                        wtr.write_record(header_keys.iter().map(String::as_str))
                            .map_err(Self::write_err)?;
                    }
                    let record: Vec<String> = header_keys
                        .iter()
                        .map(|k| {
                            Self::field_to_string(obj.get(k).unwrap_or(&serde_json::Value::Null))
                        })
                        .collect();
                    wtr.write_record(record.iter().map(String::as_str))
                        .map_err(Self::write_err)?;
                }
                _ => {
                    return Err(CamelError::TypeConversionFailed(format!(
                        "CsvDataFormat::marshal only supports Json Array or Object, got {json:?}"
                    )));
                }
            }
            wtr.flush()
                .map_err(|e| CamelError::TypeConversionFailed(format!("CSV flush: {e}")))?;
        }

        let text = String::from_utf8(output)
            .map_err(|e| CamelError::TypeConversionFailed(format!("CSV output not UTF-8: {e}")))?;
        Ok(Body::Text(text))
    }

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        self.unmarshal_internal(body).map(|(b, _)| b)
    }

    fn unmarshal_in_exchange(
        &self,
        exchange: &mut Exchange,
        body: Body,
    ) -> Result<Body, CamelError> {
        let (result, header_keys) = self.unmarshal_internal(body)?;
        if let Some(keys) = header_keys
            .filter(|_| self.config.capture_header_record)
            .filter(|k| !k.is_empty())
        {
            exchange.input.headers.insert(
                CAMEL_CSV_HEADER_RECORD.to_string(),
                serde_json::Value::Array(keys.into_iter().map(serde_json::Value::String).collect()),
            );
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_default_config_values() {
        let cfg = CsvConfig::default();
        assert_eq!(cfg.delimiter, ',');
        assert_eq!(cfg.quote, '"');
        assert!(matches!(cfg.quote_mode, QuoteMode::Minimal));
        assert!(cfg.double_quote);
        assert!(cfg.escape.is_none());
        assert!(matches!(cfg.record_separator, RecordSeparator::Crlf));
        assert!(cfg.comment_marker.is_none());
        assert!(cfg.null_string.is_none());
        assert!(cfg.headers.is_none());
        assert!(cfg.has_headers);
        assert!(!cfg.skip_header_record);
        assert!(!cfg.capture_header_record);
        assert!(cfg.ignore_surrounding_spaces);
        assert!(!cfg.trim);
        assert!(cfg.use_maps);
    }

    #[test]
    fn test_tdf_preset_uses_tab() {
        let cfg = CsvConfig::tdf();
        assert_eq!(cfg.delimiter, '\t');
        assert!(matches!(cfg.record_separator, RecordSeparator::Lf));
    }

    #[test]
    fn test_mysql_preset_uses_tab_and_escape() {
        let cfg = CsvConfig::mysql();
        assert_eq!(cfg.delimiter, '\t');
        assert_eq!(cfg.escape, Some('\\'));
    }

    #[test]
    fn test_builder_methods() {
        let cfg = CsvConfig::default()
            .delimiter('|')
            .headers(vec!["a".into(), "b".into()])
            .use_maps(false);
        assert_eq!(cfg.delimiter, '|');
        assert_eq!(cfg.headers, Some(vec!["a".into(), "b".into()]));
        assert!(!cfg.use_maps);
    }

    #[test]
    fn test_marshal_array_of_objects_with_header() {
        let df = CsvDataFormat::default();
        let body = Body::Json(json!([
            { "name": "Alice", "age": 30 },
            { "name": "Bob", "age": 25 }
        ]));
        let result = df.marshal(body).unwrap();
        match result {
            Body::Text(s) => {
                let lines: Vec<&str> = s.lines().collect();
                // serde_json key order is not guaranteed (depends on preserve_order
                // feature); assert the header carries both keys in any order.
                let header_fields: Vec<&str> = lines[0].split(',').collect();
                assert_eq!(header_fields.len(), 2);
                assert!(header_fields.contains(&"name"));
                assert!(header_fields.contains(&"age"));
                assert!(lines[1].contains("Alice"));
                assert!(lines[2].contains("Bob"));
            }
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_marshal_array_of_arrays_no_header() {
        let df = CsvDataFormat::default();
        let body = Body::Json(json!([["a", "b", "c"], [1, 2, 3]]));
        let result = df.marshal(body).unwrap();
        match result {
            Body::Text(s) => {
                let lines: Vec<&str> = s.lines().collect();
                assert_eq!(lines.len(), 2);
                assert!(lines[0].contains("a"));
            }
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_marshal_single_object() {
        let df = CsvDataFormat::default();
        let body = Body::Json(json!({ "x": 1, "y": 2 }));
        let result = df.marshal(body).unwrap();
        match result {
            Body::Text(s) => {
                let lines: Vec<&str> = s.lines().collect();
                assert_eq!(lines.len(), 2);
            }
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_marshal_empty_array_with_headers() {
        let cfg = CsvConfig::default().headers(vec!["h1".into(), "h2".into()]);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Json(json!([]));
        let result = df.marshal(body).unwrap();
        match result {
            Body::Text(s) => {
                assert_eq!(s.trim(), "h1,h2");
            }
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_marshal_empty_array_no_headers() {
        let cfg = CsvConfig::default().has_headers(false);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Json(json!([]));
        let result = df.marshal(body).unwrap();
        match result {
            Body::Text(s) => assert!(s.is_empty()),
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_marshal_quoted_fields_with_commas() {
        let df = CsvDataFormat::default();
        let body = Body::Json(json!([
            { "name": "Doe, John", "age": 30 }
        ]));
        let result = df.marshal(body).unwrap();
        match result {
            Body::Text(s) => assert!(s.contains("\"Doe, John\"")),
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_marshal_text_returns_error() {
        let df = CsvDataFormat::default();
        let result = df.marshal(Body::Text("already text".into()));
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_marshal_stream_returns_error() {
        use bytes::Bytes;
        use camel_api::body::{StreamBody, StreamMetadata};
        let df = CsvDataFormat::default();
        let empty_stream = futures::stream::empty::<Result<Bytes, CamelError>>();
        let stream_body = StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(Some(Box::pin(empty_stream)))),
            metadata: StreamMetadata::default(),
        };
        let result = df.marshal(Body::Stream(stream_body));
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_marshal_quote_mode_all() {
        let cfg = CsvConfig::default().quote_mode(QuoteMode::All);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Json(json!([{ "x": 1 }]));
        let result = df.marshal(body).unwrap();
        match result {
            Body::Text(s) => {
                assert!(s.contains("\"x\""));
            }
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_marshal_mysql_preset_quote_char_null() {
        let cfg = CsvConfig::mysql();
        let df = CsvDataFormat::new(cfg);
        let body = Body::Json(json!([["a", "b"]]));
        let result = df.marshal(body);
        assert!(result.is_ok(), "mysql preset should marshal: {result:?}");
    }

    #[test]
    fn test_unmarshal_text_to_json_maps() {
        let df = CsvDataFormat::default();
        let body = Body::Text("name,age\nAlice,30\nBob,25".into());
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => {
                let arr = v.as_array().expect("expected array");
                assert_eq!(arr.len(), 2);
                assert_eq!(arr[0]["name"], json!("Alice"));
                assert_eq!(arr[0]["age"], json!("30"));
            }
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_text_to_json_lists() {
        let cfg = CsvConfig::default().use_maps(false);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Text("a,b,c\n1,2,3".into());
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => {
                let arr = v.as_array().expect("expected array");
                assert_eq!(arr.len(), 1);
                assert_eq!(arr[0][0], json!("1"));
            }
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_bytes_to_json() {
        let df = CsvDataFormat::default();
        let body = Body::Bytes(bytes::Bytes::from_static(b"x,y\n1,2"));
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => {
                let arr = v.as_array().unwrap();
                assert_eq!(arr[0]["x"], json!("1"));
            }
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_configured_headers() {
        let cfg = CsvConfig::default().headers(vec!["col1".into(), "col2".into()]);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Text("val1,val2".into());
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => {
                let arr = v.as_array().unwrap();
                assert_eq!(arr[0]["col1"], json!("val1"));
            }
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_skip_header_record_with_configured_headers() {
        let cfg = CsvConfig::default()
            .headers(vec!["col1".into(), "col2".into()])
            .skip_header_record(true);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Text("ignored1,ignored2\nval1,val2".into());
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => {
                let arr = v.as_array().unwrap();
                assert_eq!(arr.len(), 1);
                assert_eq!(arr[0]["col1"], json!("val1"));
            }
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_quoted_fields() {
        let df = CsvDataFormat::default();
        let body = Body::Text("name,note\n\"Doe, John\",hi".into());
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => {
                let arr = v.as_array().unwrap();
                assert_eq!(arr[0]["name"], json!("Doe, John"));
            }
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_delimiter_tdf() {
        let cfg = CsvConfig::tdf();
        let df = CsvDataFormat::new(cfg);
        let body = Body::Text("a\tb\n1\t2".into());
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => {
                let arr = v.as_array().unwrap();
                assert_eq!(arr[0]["a"], json!("1"));
            }
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_comment_marker() {
        let cfg = CsvConfig::default().comment_marker(Some('#'));
        let df = CsvDataFormat::new(cfg);
        let body = Body::Text("# this is a comment\na,b\n1,2".into());
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => {
                let arr = v.as_array().unwrap();
                assert_eq!(arr.len(), 1);
            }
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_unmarshal_empty_returns_error() {
        let df = CsvDataFormat::default();
        let result = df.unmarshal(Body::Empty);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_unmarshal_stream_returns_error() {
        use bytes::Bytes;
        use camel_api::body::{StreamBody, StreamMetadata};
        let df = CsvDataFormat::default();
        let empty_stream = futures::stream::empty::<Result<Bytes, CamelError>>();
        let stream_body = StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(Some(Box::pin(empty_stream)))),
            metadata: StreamMetadata::default(),
        };
        let result = df.unmarshal(Body::Stream(stream_body));
        let msg = match result {
            Err(CamelError::TypeConversionFailed(m)) => m,
            _ => panic!("expected TypeConversionFailed"),
        };
        assert!(msg.contains("stream_cache"));
    }

    #[test]
    fn test_unmarshal_no_header_map_mode_error() {
        let cfg = CsvConfig::default().has_headers(false);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Text("1,2,3".into());
        let result = df.unmarshal(body);
        let msg = match result {
            Err(CamelError::TypeConversionFailed(m)) => m,
            _ => panic!("expected TypeConversionFailed"),
        };
        assert!(msg.contains("has_headers") || msg.contains("headers"));
    }

    #[test]
    fn test_unmarshal_json_identity() {
        let df = CsvDataFormat::default();
        let input = json!([{"x": 1}]);
        let body = Body::Json(input.clone());
        let result = df.unmarshal(body).unwrap();
        assert!(matches!(result, Body::Json(_)));
    }

    #[test]
    fn test_unmarshal_null_string() {
        let cfg = CsvConfig::default().null_string(Some("NULL".into()));
        let df = CsvDataFormat::new(cfg);
        let body = Body::Text("a,b\n1,NULL".into());
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Json(v) => {
                let arr = v.as_array().unwrap();
                assert_eq!(arr[0]["b"], serde_json::Value::Null);
            }
            _ => panic!("expected Body::Json"),
        }
    }

    #[test]
    fn test_marshal_roundtrip() {
        let df = CsvDataFormat::default();
        let original = "name,age\nAlice,30\nBob,25";
        let json = df.unmarshal(Body::Text(original.into())).unwrap();
        let back = df.marshal(json).unwrap();
        match back {
            Body::Text(s) => {
                assert!(s.contains("Alice"));
                assert!(s.contains("name"));
            }
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_unmarshal_capture_header_maps_mode() {
        let cfg = CsvConfig::default().capture_header_record(true);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Text("name,age\nAlice,30".into());

        let mut ex = Exchange::default();
        let result = df.unmarshal_in_exchange(&mut ex, body).unwrap();
        assert!(matches!(result, Body::Json(_)));

        let captured = ex.input.headers.get(CAMEL_CSV_HEADER_RECORD);
        assert_eq!(captured, Some(&serde_json::json!(["name", "age"])));
    }

    #[test]
    fn test_unmarshal_capture_header_lists_mode() {
        let cfg = CsvConfig::default()
            .capture_header_record(true)
            .use_maps(false);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Text("a,b\n1,2".into());

        let mut ex = Exchange::default();
        let result = df.unmarshal_in_exchange(&mut ex, body).unwrap();
        assert!(matches!(result, Body::Json(_)));

        let captured = ex.input.headers.get(CAMEL_CSV_HEADER_RECORD);
        assert_eq!(captured, Some(&serde_json::json!(["a", "b"])));
    }

    #[test]
    fn test_unmarshal_capture_header_configured_headers() {
        let cfg = CsvConfig::default()
            .capture_header_record(true)
            .headers(vec!["col1".into(), "col2".into()])
            .skip_header_record(true);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Text("ignored1,ignored2\nval1,val2".into());

        let mut ex = Exchange::default();
        let _ = df.unmarshal_in_exchange(&mut ex, body).unwrap();

        let captured = ex.input.headers.get(CAMEL_CSV_HEADER_RECORD);
        assert_eq!(captured, Some(&serde_json::json!(["col1", "col2"])));
    }

    #[test]
    fn test_unmarshal_no_capture_default() {
        let df = CsvDataFormat::default();
        let body = Body::Text("a,b\n1,2".into());

        let mut ex = Exchange::default();
        let _ = df.unmarshal_in_exchange(&mut ex, body).unwrap();

        assert!(ex.input.headers.get(CAMEL_CSV_HEADER_RECORD).is_none());
    }

    #[test]
    fn test_marshal_mixed_array_objects_returns_error() {
        let df = CsvDataFormat::default();
        let body = Body::Json(json!([
            { "name": "Alice" },
            "scalar string",
            { "name": "Bob" }
        ]));
        let result = df.marshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_marshal_mixed_array_lists_returns_error() {
        let cfg = CsvConfig::default().use_maps(false);
        let df = CsvDataFormat::new(cfg);
        let body = Body::Json(json!([
            ["a", "b"],
            { "not": "array" },
            ["c", "d"]
        ]));
        let result = df.marshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }
}
