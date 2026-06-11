use crate::stream_codec::{
    CAMEL_STREAM_BATCH_SIZE, CAMEL_STREAM_OFFSET, CAMEL_STREAM_ORIGIN,
    CAMEL_STREAM_SOURCE_CONTENT_TYPE, StreamSplitCodec, StreamSplitInput, fragment_stream_exchange,
};
use bytes::BytesMut;
use camel_api::{Body, CamelError, Exchange, StreamSplitConfig, Value};
use futures::{Stream, StreamExt};
use std::pin::Pin;

pub struct LinesCodec;

impl StreamSplitCodec for LinesCodec {
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
            let mut batch: Vec<String> = Vec::new();
            let mut stream = stream;

            // Macro to flush the current batch as a single exchange
            macro_rules! flush_batch {
                () => {
                    if !batch.is_empty() {
                        let lines = std::mem::take(&mut batch);
                        let batch_offset = offset - (lines.len() as u64);
                        let body = if lines.len() == 1 {
                            Body::Text(lines.into_iter().next().unwrap())
                        } else {
                            Body::Text(lines.join("\n"))
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

                    // Check max_record_bytes on the line (without newline)
                    if line.len() > config.max_record_bytes {
                        flush_batch!();
                        Err(CamelError::StreamLimitExceeded(config.max_record_bytes))?;
                    }

                    // Convert to string
                    let trimmed = match std::str::from_utf8(line) {
                        Ok(s) => s.trim(),
                        Err(_) => {
                            flush_batch!();
                            Err(CamelError::TypeConversionFailed(
                                "Line is not valid UTF-8".into(),
                            ))?;
                            unreachable!();
                        }
                    };

                    // Skip empty/blank lines
                    if trimmed.is_empty() {
                        continue;
                    }

                    batch.push(trimmed.to_string());
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
                            "Line is not valid UTF-8".into(),
                        ))?;
                        unreachable!();
                    }
                };

                if !trimmed.is_empty() {
                    if line.len() > config.max_record_bytes {
                        flush_batch!();
                        Err(CamelError::StreamLimitExceeded(config.max_record_bytes))?;
                    }

                    batch.push(trimmed.to_string());
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

    fn make_lines_input(lines: Vec<&str>) -> StreamSplitInput {
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
                content_type: Some("text/plain".into()),
                size_hint: None,
                origin: Some("test://lines".into()),
            },
        }
    }

    #[tokio::test]
    async fn test_lines_splits_three_lines() {
        let input = make_lines_input(vec!["hello", "world", "foo"]);
        let config = StreamSplitConfig::default();
        let codec = LinesCodec;
        let fragments: Vec<_> = codec.split(input, config).collect::<Vec<_>>().await;
        assert_eq!(fragments.len(), 3);
        let bodies: Vec<_> = fragments.into_iter().map(|r| r.expect("ok")).collect();
        assert!(matches!(&bodies[0].input.body, Body::Text(s) if s == "hello"));
        assert!(matches!(&bodies[1].input.body, Body::Text(s) if s == "world"));
        assert!(matches!(&bodies[2].input.body, Body::Text(s) if s == "foo"));
    }

    #[tokio::test]
    async fn test_lines_empty_stream() {
        let input = make_lines_input(vec![]);
        let config = StreamSplitConfig::default();
        let codec = LinesCodec;
        let count = codec.split(input, config).count().await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_lines_empty_lines_skipped() {
        let input = make_lines_input(vec!["a", "", "b"]);
        let config = StreamSplitConfig::default();
        let codec = LinesCodec;
        let count = codec.split(input, config).count().await;
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_lines_trailing_newline_no_extra_fragment() {
        let data = vec![Ok(Bytes::from("hello\nworld\n"))];
        let stream = Box::pin(stream::iter(data));
        let parent = Exchange::new(Message::new(Body::Empty));
        let input = StreamSplitInput {
            parent,
            stream,
            metadata: StreamMetadata {
                content_type: Some("text/plain".into()),
                size_hint: None,
                origin: None,
            },
        };
        let config = StreamSplitConfig::default();
        let codec = LinesCodec;
        let count = codec.split(input, config).count().await;
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_lines_exceeds_max_record_bytes() {
        let long_line = "x".repeat(2000);
        let input = make_lines_input(vec![&long_line]);
        let config = StreamSplitConfig {
            max_record_bytes: 100,
            ..Default::default()
        };
        let codec = LinesCodec;
        let result = codec.split(input, config).next().await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_lines_sets_stream_properties() {
        let input = make_lines_input(vec!["hello"]);
        let config = StreamSplitConfig {
            include_origin: true,
            ..Default::default()
        };
        let codec = LinesCodec;
        let ex = codec
            .split(input, config)
            .next()
            .await
            .unwrap()
            .expect("ok");
        assert_eq!(
            ex.property(CAMEL_STREAM_ORIGIN),
            Some(&Value::String("test://lines".into()))
        );
    }

    #[tokio::test]
    async fn test_lines_sets_offset_property() {
        let input = make_lines_input(vec!["alpha", "beta", "gamma"]);
        let config = StreamSplitConfig::default();
        let codec = LinesCodec;
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
    async fn test_lines_headers_excluded() {
        let mut parent = Exchange::new(Message::new(Body::Empty));
        parent
            .input
            .headers
            .insert("Content-Type".into(), Value::String("text/plain".into()));
        parent
            .input
            .headers
            .insert("Content-Length".into(), Value::String("42".into()));
        parent
            .input
            .headers
            .insert("X-Custom".into(), Value::String("kept".into()));
        let data = vec![Ok(Bytes::from("hello\n"))];
        let stream = Box::pin(stream::iter(data));
        let input = StreamSplitInput {
            parent,
            stream,
            metadata: StreamMetadata {
                content_type: Some("text/plain".into()),
                size_hint: None,
                origin: None,
            },
        };
        let config = StreamSplitConfig::default();
        let codec = LinesCodec;
        let ex = codec
            .split(input, config)
            .next()
            .await
            .unwrap()
            .expect("ok");
        assert!(
            ex.input.headers.get("Content-Type").is_none(),
            "Content-Type should be excluded"
        );
        assert!(
            ex.input.headers.get("Content-Length").is_none(),
            "Content-Length should be excluded"
        );
        assert_eq!(
            ex.input.headers.get("X-Custom"),
            Some(&Value::String("kept".into()))
        );
    }

    #[tokio::test]
    async fn test_lines_include_origin_false() {
        let input = make_lines_input(vec!["hello"]);
        let config = StreamSplitConfig {
            include_origin: false,
            ..Default::default()
        };
        let codec = LinesCodec;
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

    #[tokio::test]
    async fn test_lines_batch_size_two() {
        let input = make_lines_input(vec!["a", "b", "c", "d"]);
        let config = StreamSplitConfig {
            batch_size: 2,
            ..Default::default()
        };
        let codec = LinesCodec;
        let fragments: Vec<_> = codec.split(input, config).collect::<Vec<_>>().await;
        assert_eq!(fragments.len(), 2);
        for frag in &fragments {
            let ex = frag.as_ref().expect("should be ok");
            assert!(
                matches!(&ex.input.body, Body::Text(s) if s.len() > 0),
                "batch should contain lines"
            );
        }
    }

    #[tokio::test]
    async fn test_lines_non_utf8_returns_error() {
        // Invalid UTF-8 bytes
        let data = vec![Ok(Bytes::from(b"\xff\xfe\n".as_slice()))];
        let stream = Box::pin(stream::iter(data));
        let parent = Exchange::new(Message::new(Body::Empty));
        let input = StreamSplitInput {
            parent,
            stream,
            metadata: StreamMetadata {
                content_type: Some("text/plain".into()),
                size_hint: None,
                origin: None,
            },
        };
        let config = StreamSplitConfig::default();
        let codec = LinesCodec;
        let result = codec.split(input, config).next().await.unwrap();
        assert!(result.is_err());
    }
}
