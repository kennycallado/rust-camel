use crate::stream_codec::{
    CAMEL_STREAM_BATCH_SIZE, CAMEL_STREAM_OFFSET, CAMEL_STREAM_ORIGIN,
    CAMEL_STREAM_SOURCE_CONTENT_TYPE, StreamSplitCodec, StreamSplitInput, fragment_stream_exchange,
};
use bytes::{Bytes, BytesMut};
use camel_api::{Body, CamelError, Exchange, StreamSplitConfig, Value};
use futures::{Stream, StreamExt};
use std::pin::Pin;

pub struct ChunksCodec;

impl StreamSplitCodec for ChunksCodec {
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
        let chunk_size = config.chunk_size.unwrap_or(8192);

        Box::pin(async_stream::try_stream! {
            let mut buffer = BytesMut::new();
            let mut offset = 0u64;
            let mut batch: Vec<Bytes> = Vec::new();
            let mut stream = stream;

            // Macro to flush the current batch as a single exchange
            macro_rules! flush_batch {
                () => {
                    if !batch.is_empty() {
                        let chunks = std::mem::take(&mut batch);
                        let batch_offset = offset - (chunks.len() as u64);
                        let body = if chunks.len() == 1 {
                            Body::Bytes(chunks.into_iter().next().unwrap())
                        } else {
                            let mut combined = BytesMut::new();
                            for c in &chunks {
                                combined.extend_from_slice(c);
                            }
                            Body::Bytes(combined.freeze())
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

                // Emit full chunks from the buffer
                while buffer.len() >= chunk_size {
                    let slice = buffer.split_to(chunk_size);
                    batch.push(slice.freeze());
                    offset += 1;

                    // Flush if batch is full
                    if batch.len() >= config.batch_size {
                        flush_batch!();
                    }
                }
            }

            // Handle remaining data after stream ends (last partial chunk)
            if !buffer.is_empty() {
                if buffer.len() > config.max_record_bytes {
                    flush_batch!();
                    Err(CamelError::StreamLimitExceeded(config.max_record_bytes))?;
                }
                let remaining = std::mem::take(&mut buffer);
                batch.push(remaining.freeze());
                offset += 1;

                if batch.len() >= config.batch_size {
                    flush_batch!();
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

    fn make_chunks_input(data: Vec<&[u8]>) -> StreamSplitInput {
        let chunks = data
            .into_iter()
            .map(|d| Ok(Bytes::from(d.to_vec())))
            .collect::<Vec<_>>();
        let stream = Box::pin(stream::iter(chunks));
        let parent = Exchange::new(Message::new(Body::Empty));
        StreamSplitInput {
            parent,
            stream,
            metadata: StreamMetadata {
                content_type: Some("application/octet-stream".into()),
                size_hint: None,
                origin: Some("test://chunks".into()),
            },
        }
    }

    #[tokio::test]
    async fn test_chunks_exact_split() {
        let input = make_chunks_input(vec![b"0123456789"]);
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(5),
            ..Default::default()
        };
        let codec = ChunksCodec;
        let fragments: Vec<_> = codec.split(input, config).collect::<Vec<_>>().await;
        assert_eq!(fragments.len(), 2);
        let f0 = fragments[0].as_ref().expect("ok");
        assert!(matches!(&f0.input.body, Body::Bytes(b) if b.len() == 5));
        assert!(matches!(&f0.input.body, Body::Bytes(b) if b == "01234"));
        let f1 = fragments[1].as_ref().expect("ok");
        assert!(matches!(&f1.input.body, Body::Bytes(b) if b == "56789"));
    }

    #[tokio::test]
    async fn test_chunks_last_chunk_smaller() {
        let input = make_chunks_input(vec![b"01234"]);
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(3),
            ..Default::default()
        };
        let codec = ChunksCodec;
        let fragments: Vec<_> = codec.split(input, config).collect::<Vec<_>>().await;
        assert_eq!(fragments.len(), 2);
        let f0 = fragments[0].as_ref().expect("ok");
        let f1 = fragments[1].as_ref().expect("ok");
        assert!(matches!(&f0.input.body, Body::Bytes(b) if b.len() == 3));
        assert!(matches!(&f0.input.body, Body::Bytes(b) if b == "012"));
        assert!(matches!(&f1.input.body, Body::Bytes(b) if b.len() == 2));
        assert!(matches!(&f1.input.body, Body::Bytes(b) if b == "34"));
    }

    #[tokio::test]
    async fn test_chunks_empty_stream() {
        let input = make_chunks_input(vec![]);
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(10),
            ..Default::default()
        };
        let codec = ChunksCodec;
        let count = codec.split(input, config).count().await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_chunks_auto_resolved_to_chunks_codec() {
        let metadata = StreamMetadata {
            content_type: Some("application/octet-stream".into()),
            size_hint: None,
            origin: None,
        };
        let result =
            crate::stream_codec::resolve_format(&camel_api::StreamSplitFormat::Auto, &metadata);
        assert_eq!(result.unwrap(), camel_api::StreamSplitFormat::Chunks);
    }

    #[tokio::test]
    async fn test_chunks_chunk_size_zero_rejected() {
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(0),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_chunks_chunk_size_exceeds_max_record_bytes() {
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(2000),
            max_record_bytes: 1000,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("chunk_size must be <= max_record_bytes"),
            "chunk exceeding max_record_bytes should be rejected by validate()"
        );
    }

    #[tokio::test]
    async fn test_chunks_sets_stream_properties() {
        let input = make_chunks_input(vec![b"hello"]);
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(5),
            include_origin: true,
            ..Default::default()
        };
        let codec = ChunksCodec;
        let ex = codec
            .split(input, config)
            .next()
            .await
            .unwrap()
            .expect("ok");
        assert_eq!(
            ex.property(CAMEL_STREAM_ORIGIN),
            Some(&Value::String("test://chunks".into()))
        );
    }

    #[tokio::test]
    async fn test_chunks_include_origin_false() {
        let input = make_chunks_input(vec![b"hello"]);
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(5),
            include_origin: false,
            ..Default::default()
        };
        let codec = ChunksCodec;
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
    async fn test_chunks_headers_excluded() {
        let mut parent = Exchange::new(Message::new(Body::Empty));
        parent.input.headers.insert(
            "Content-Type".into(),
            Value::String("application/octet-stream".into()),
        );
        parent
            .input
            .headers
            .insert("Content-Length".into(), Value::String("5".into()));
        parent
            .input
            .headers
            .insert("X-Custom".into(), Value::String("kept".into()));
        let data = vec![Ok(Bytes::from(b"hello" as &[u8]))];
        let stream = Box::pin(stream::iter(data));
        let input = StreamSplitInput {
            parent,
            stream,
            metadata: StreamMetadata {
                content_type: Some("application/octet-stream".into()),
                size_hint: None,
                origin: None,
            },
        };
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(5),
            ..Default::default()
        };
        let codec = ChunksCodec;
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
    async fn test_chunks_sets_offset_property() {
        let input = make_chunks_input(vec![b"0123456789"]);
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(5),
            ..Default::default()
        };
        let codec = ChunksCodec;
        let fragments: Vec<_> = codec.split(input, config).collect::<Vec<_>>().await;
        assert_eq!(fragments.len(), 2);
        for (i, frag) in fragments.iter().enumerate() {
            let ex = frag.as_ref().expect("should be ok");
            assert_eq!(
                ex.property(CAMEL_STREAM_OFFSET),
                Some(&Value::from(i as i64))
            );
        }
    }

    #[tokio::test]
    async fn test_chunks_batch_size_two() {
        let input = make_chunks_input(vec![b"0123456789"]);
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(3),
            batch_size: 2,
            ..Default::default()
        };
        let codec = ChunksCodec;
        let fragments: Vec<_> = codec.split(input, config).collect::<Vec<_>>().await;
        // 10 bytes / 3 = 4 chunks (3+3+3+1), batched as 2+2 => 2 fragments
        assert_eq!(fragments.len(), 2);

        // First batch: chunks 0+1 -> "012" + "345" = "012345", offset=0, chunk count=2
        let ex0 = fragments[0].as_ref().expect("should be ok");
        assert!(matches!(&ex0.input.body, Body::Bytes(b) if b == "012345"));
        assert_eq!(ex0.property(CAMEL_STREAM_OFFSET), Some(&Value::from(0i64)));
        assert_eq!(
            ex0.property(CAMEL_STREAM_BATCH_SIZE),
            Some(&Value::from(2i64))
        );

        // Second batch: chunks 2+3 -> "678" + "9" = "6789", offset=2, chunk count=2
        let ex1 = fragments[1].as_ref().expect("should be ok");
        assert!(matches!(&ex1.input.body, Body::Bytes(b) if b == "6789"));
        assert_eq!(ex1.property(CAMEL_STREAM_OFFSET), Some(&Value::from(2i64)));
        assert_eq!(
            ex1.property(CAMEL_STREAM_BATCH_SIZE),
            Some(&Value::from(2i64))
        );
    }

    #[tokio::test]
    async fn test_chunks_partial_chunk_flushed_at_end() {
        let input = make_chunks_input(vec![b"abcde"]);
        let config = StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Chunks,
            chunk_size: Some(3),
            ..Default::default()
        };
        let codec = ChunksCodec;
        let fragments: Vec<_> = codec.split(input, config).collect::<Vec<_>>().await;
        assert_eq!(fragments.len(), 2);
        assert!(matches!(
            &fragments[0].as_ref().expect("ok").input.body,
            Body::Bytes(b) if b == "abc"
        ));
        assert!(matches!(
            &fragments[1].as_ref().expect("ok").input.body,
            Body::Bytes(b) if b == "de"
        ));
    }
}
