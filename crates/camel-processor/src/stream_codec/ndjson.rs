use crate::stream_codec::{
    CAMEL_STREAM_BATCH_SIZE, CAMEL_STREAM_OFFSET, CAMEL_STREAM_ORIGIN,
    CAMEL_STREAM_SOURCE_CONTENT_TYPE, StreamSplitCodec, StreamSplitInput, fragment_stream_exchange,
};
use bytes::BytesMut;
use camel_api::{Body, CamelError, Exchange, StreamSplitConfig, Value};
use futures::{Stream, StreamExt};
use std::pin::Pin;

pub struct NdjsonCodec;

impl StreamSplitCodec for NdjsonCodec {
    fn split(
        &self,
        input: StreamSplitInput,
        config: StreamSplitConfig,
    ) -> Pin<Box<dyn Stream<Item = Result<Exchange, CamelError>> + Send>> {
        let StreamSplitInput {
            parent,
            stream,
            metadata,
        } = input;
        let origin = metadata.origin;
        let content_type = metadata.content_type;

        Box::pin(async_stream::try_stream! {
            let mut buffer = BytesMut::new();
            let mut offset = 0u64;
            let mut batch: Vec<serde_json::Value> = Vec::new();
            let mut stream = stream;

            // Macro to flush the current batch as a single exchange
            macro_rules! flush_batch {
                () => {
                    if !batch.is_empty() {
                        let values = std::mem::take(&mut batch);
                        let batch_offset = offset - (values.len() as u64);
                        let body = if values.len() == 1 {
                            Body::Json(values.into_iter().next().unwrap()) // allow-unwrap: len==1 guaranteed by enclosing if
                        } else {
                            Body::Json(serde_json::Value::Array(values))
                        };
                        let mut ex = fragment_stream_exchange(&parent, body);
                        ex.set_property(CAMEL_STREAM_OFFSET, Value::from(batch_offset as i64));
                        if let Some(ref ct) = content_type {
                            ex.set_property(CAMEL_STREAM_SOURCE_CONTENT_TYPE, Value::String(ct.clone()));
                        }
                        if config.include_origin {
                            if let Some(ref o) = origin {
                                ex.set_property(CAMEL_STREAM_ORIGIN, Value::String(o.clone()));
                            }
                        }
                        if batch_offset != offset {
                            ex.set_property(CAMEL_STREAM_BATCH_SIZE, Value::from((offset - batch_offset) as i64));
                        }
                        yield ex;
                    }
                };
            }

            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                buffer.extend_from_slice(&chunk);

                // Process all complete lines in the buffer
                loop {
                    // Check for line-too-long before finding a newline
                    if buffer.len() > config.max_record_bytes {
                        // Check if there's actually a newline in the buffer
                        if !buffer.contains(&b'\n') {
                            flush_batch!();
                            Err(CamelError::StreamLimitExceeded(config.max_record_bytes))?;
                        }
                    }

                    let newline_pos = match buffer.iter().position(|&b| b == b'\n') {
                        Some(pos) => pos,
                        None => break, // need more data
                    };

                    let line_bytes = buffer.split_to(newline_pos + 1); // include '\n'
                    let line = &line_bytes[..newline_pos]; // exclude '\n'

                    // Skip empty/blank lines
                    let trimmed = match std::str::from_utf8(line) {
                        Ok(s) => s.trim(),
                        Err(_) => {
                            flush_batch!();
                            Err(CamelError::TypeConversionFailed(
                                "NDJSON line is not valid UTF-8".into(),
                            ))?;
                            unreachable!();
                        }
                    };
                    if trimmed.is_empty() {
                        continue;
                    }

                    // Check max_record_bytes on the line (without newline)
                    if line.len() > config.max_record_bytes {
                        flush_batch!();
                        Err(CamelError::StreamLimitExceeded(config.max_record_bytes))?;
                    }

                    // Parse as JSON
                    let value: serde_json::Value = match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        Err(e) => {
                            flush_batch!();
                            Err(CamelError::TypeConversionFailed(format!(
                                "NDJSON parse error: {}",
                                e
                            )))?;
                            unreachable!();
                        }
                    };

                    batch.push(value);
                    offset += 1;

                    // Flush if batch is full
                    if batch.len() >= config.batch_size {
                        flush_batch!();
                    }
                }
            }

            // Handle remaining data after stream ends (last line without trailing newline)
            if !buffer.is_empty() {
                let line = std::mem::take(&mut buffer);
                let trimmed = match std::str::from_utf8(&line) {
                    Ok(s) => s.trim(),
                    Err(_) => {
                        flush_batch!();
                        Err(CamelError::TypeConversionFailed(
                            "NDJSON line is not valid UTF-8".into(),
                        ))?;
                        unreachable!();
                    }
                };

                if !trimmed.is_empty() {
                    if line.len() > config.max_record_bytes {
                        flush_batch!();
                        Err(CamelError::StreamLimitExceeded(config.max_record_bytes))?;
                    }

                    let value: serde_json::Value = match serde_json::from_str(trimmed) {
                        Ok(v) => v,
                        Err(e) => {
                            flush_batch!();
                            Err(CamelError::TypeConversionFailed(format!(
                                "NDJSON parse error: {}",
                                e
                            )))?;
                            unreachable!();
                        }
                    };

                    batch.push(value);
                    offset += 1;

                    if batch.len() >= config.batch_size {
                        flush_batch!();
                    }
                }
            }

            // Final flush at end of stream
            flush_batch!();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use camel_api::{Message, StreamMetadata};
    use futures::stream;

    fn make_stream_input(lines: Vec<&str>) -> StreamSplitInput {
        let data = lines
            .iter()
            .map(|l| Ok(Bytes::from(format!("{}\n", l))))
            .collect::<Vec<_>>();
        let stream = Box::pin(stream::iter(data));
        let parent = Exchange::new(Message::new(Body::Empty));
        StreamSplitInput {
            parent,
            stream,
            metadata: StreamMetadata {
                content_type: Some("application/x-ndjson".into()),
                size_hint: None,
                origin: Some("test://stream".into()),
            },
        }
    }

    #[tokio::test]
    async fn test_ndjson_splits_three_rows() {
        let input = make_stream_input(vec![
            r#"{"id":1,"name":"a"}"#,
            r#"{"id":2,"name":"b"}"#,
            r#"{"id":3,"name":"c"}"#,
        ]);
        let config = StreamSplitConfig::default();
        let codec = NdjsonCodec;
        let fragments: Vec<_> = codec.split(input, config).collect::<Vec<_>>().await;
        assert_eq!(fragments.len(), 3);
        for frag in &fragments {
            let ex = frag.as_ref().expect("should be ok");
            assert!(matches!(ex.input.body, Body::Json(_)));
        }
    }

    #[tokio::test]
    async fn test_ndjson_empty_stream() {
        let input = make_stream_input(vec![]);
        let config = StreamSplitConfig::default();
        let codec = NdjsonCodec;
        let count = codec.split(input, config).count().await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_ndjson_exceeds_max_record_bytes() {
        let long_line = format!("{{\"id\":{}}}", "x".repeat(2000));
        let input = make_stream_input(vec![&long_line]);
        let config = StreamSplitConfig {
            max_record_bytes: 100,
            ..Default::default()
        };
        let codec = NdjsonCodec;
        let result = codec.split(input, config).next().await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ndjson_sets_stream_properties() {
        let input = make_stream_input(vec![r#"{"id":1}"#]);
        let config = StreamSplitConfig {
            include_origin: true,
            ..Default::default()
        };
        let codec = NdjsonCodec;
        let ex = codec
            .split(input, config)
            .next()
            .await
            .unwrap()
            .expect("ok");
        assert_eq!(
            ex.property(CAMEL_STREAM_ORIGIN),
            Some(&Value::String("test://stream".into()))
        );
    }

    #[tokio::test]
    async fn test_ndjson_invalid_json_returns_error() {
        let input = make_stream_input(vec!["not-json"]);
        let config = StreamSplitConfig::default();
        let codec = NdjsonCodec;
        let result = codec.split(input, config).next().await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ndjson_empty_lines_skipped() {
        let input = make_stream_input(vec![r#"{"id":1}"#, "", r#"{"id":2}"#]);
        let config = StreamSplitConfig::default();
        let codec = NdjsonCodec;
        let count = codec.split(input, config).count().await;
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_ndjson_headers_excluded() {
        let mut parent = Exchange::new(Message::new(Body::Empty));
        parent.input.headers.insert(
            "Content-Type".into(),
            Value::String("application/x-ndjson".into()),
        );
        parent
            .input
            .headers
            .insert("Content-Length".into(), Value::String("42".into()));
        parent
            .input
            .headers
            .insert("X-Custom".into(), Value::String("kept".into()));
        let data = vec![Ok(Bytes::from("{\"id\":1}\n"))];
        let stream = Box::pin(stream::iter(data));
        let input = StreamSplitInput {
            parent,
            stream,
            metadata: StreamMetadata {
                content_type: Some("application/x-ndjson".into()),
                size_hint: None,
                origin: None,
            },
        };
        let config = StreamSplitConfig::default();
        let codec = NdjsonCodec;
        let ex = codec
            .split(input, config)
            .next()
            .await
            .unwrap()
            .expect("ok");
        assert!(
            !ex.input.headers.contains_key("Content-Type"),
            "Content-Type should be excluded"
        );
        assert!(
            !ex.input.headers.contains_key("Content-Length"),
            "Content-Length should be excluded"
        );
        assert_eq!(
            ex.input.headers.get("X-Custom"),
            Some(&Value::String("kept".into()))
        );
    }

    #[tokio::test]
    async fn test_ndjson_sets_offset_property() {
        let input = make_stream_input(vec![r#"{"id":1}"#, r#"{"id":2}"#, r#"{"id":3}"#]);
        let config = StreamSplitConfig::default();
        let codec = NdjsonCodec;
        let fragments: Vec<_> = codec.split(input, config).collect::<Vec<_>>().await;
        assert_eq!(fragments.len(), 3);
        for (i, frag) in fragments.iter().enumerate() {
            let ex = frag.as_ref().expect("should be ok");
            assert_eq!(
                ex.property(CAMEL_STREAM_OFFSET),
                Some(&Value::from(i as i64))
            );
        }
    }

    #[tokio::test]
    async fn test_ndjson_batch_size_two() {
        let input = make_stream_input(vec![
            r#"{"id":1}"#,
            r#"{"id":2}"#,
            r#"{"id":3}"#,
            r#"{"id":4}"#,
        ]);
        let config = StreamSplitConfig {
            batch_size: 2,
            ..Default::default()
        };
        let codec = NdjsonCodec;
        let fragments: Vec<_> = codec.split(input, config).collect::<Vec<_>>().await;
        assert_eq!(fragments.len(), 2);
        for frag in &fragments {
            let ex = frag.as_ref().expect("should be ok");
            assert!(
                matches!(&ex.input.body, Body::Json(serde_json::Value::Array(arr)) if arr.len() == 2)
            );
        }
    }

    #[tokio::test]
    async fn test_ndjson_include_origin_false() {
        let input = make_stream_input(vec![r#"{"id":1}"#]);
        let config = StreamSplitConfig {
            include_origin: false,
            ..Default::default()
        };
        let codec = NdjsonCodec;
        let ex = codec
            .split(input, config)
            .next()
            .await
            .unwrap()
            .expect("ok");
        assert!(
            ex.property(CAMEL_STREAM_ORIGIN).is_none(),
            "Origin should not be set when include_origin=false"
        );
    }
}
