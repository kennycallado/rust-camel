use futures::{StreamExt, pin_mut};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_util::sync::CancellationToken;
use tower::Service;

use camel_api::{
    AggregationStrategy, Body, BoxProcessor, CamelError, Exchange, StreamingSplitExpression, Value,
};

pub const CAMEL_SPLIT_INDEX: &str = "CamelSplitIndex";
pub const CAMEL_SPLIT_COMPLETE: &str = "CamelSplitComplete";

#[derive(Clone)]
pub struct StreamingSplitterService {
    expression: StreamingSplitExpression,
    sub_pipeline: BoxProcessor,
    aggregation: AggregationStrategy,
    stop_on_exception: bool,
    cancel_token: CancellationToken,
}

impl StreamingSplitterService {
    pub fn new(
        expression: StreamingSplitExpression,
        sub_pipeline: BoxProcessor,
        aggregation: AggregationStrategy,
        stop_on_exception: bool,
    ) -> Self {
        Self {
            expression,
            sub_pipeline,
            aggregation,
            stop_on_exception,
            cancel_token: CancellationToken::new(),
        }
    }

    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }
}

impl Service<Exchange> for StreamingSplitterService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.sub_pipeline.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let mut original = exchange.clone();
        if matches!(original.input.body, Body::Stream(_)) {
            original.input.body = Body::Empty;
        }
        let expression = self.expression.clone();
        let sub_pipeline = self.sub_pipeline.clone();
        let aggregation = self.aggregation.clone();
        let stop_on_exception = self.stop_on_exception;
        let cancel_token = self.cancel_token.clone();

        Box::pin(async move {
            let stream = expression(exchange);
            pin_mut!(stream);

            let mut acc: Option<Exchange> = None;
            let mut acc_bodies: Vec<Value> = Vec::new();
            let mut index: u64 = 0;

            // One-entry lookahead for CamelSplitComplete
            let mut current = stream.next().await;

            while let Some(fragment_result) = current.take() {
                if cancel_token.is_cancelled() {
                    return Err(CamelError::ProcessorError(
                        "StreamingSplitter cancelled".to_string(),
                    ));
                }

                let fragment = fragment_result?;

                // Peek next to know if this is the last entry
                let next = stream.next().await;
                let is_last = next.is_none();

                let mut fragment = fragment;
                fragment.set_property(CAMEL_SPLIT_INDEX, Value::from(index));
                fragment.set_property(CAMEL_SPLIT_COMPLETE, Value::Bool(is_last));

                let mut pipeline = sub_pipeline.clone();
                let ready = tower::ServiceExt::ready(&mut pipeline).await;
                let result = match ready {
                    Ok(svc) => svc.call(fragment).await,
                    Err(e) => Err(e),
                };

                match result {
                    Ok(processed) => {
                        match &aggregation {
                            AggregationStrategy::CollectAll => {
                                let v = match &processed.input.body {
                                    Body::Text(s) => Value::String(s.clone()),
                                    Body::Json(v) => v.clone(),
                                    Body::Xml(s) => Value::String(s.clone()),
                                    Body::Bytes(b) => {
                                        Value::String(String::from_utf8_lossy(b).into_owned())
                                    }
                                    Body::Empty => Value::Null,
                                    Body::Stream(_) => {
                                        return Err(CamelError::TypeConversionFailed(
                                            "StreamingSplitter CollectAll cannot aggregate Body::Stream — use 'stream_cache' or 'convert_body_to' before this step".to_string(),
                                        ));
                                    }
                                };
                                acc_bodies.push(v);
                            }
                            AggregationStrategy::Custom(fold_fn) => {
                                acc = Some(match acc {
                                    Some(prev) => fold_fn(prev, processed),
                                    None => processed,
                                });
                            }
                            _ => {
                                acc = Some(processed);
                            }
                        }
                        index += 1;
                    }
                    Err(e) => {
                        if stop_on_exception {
                            return Err(e);
                        }
                        index += 1;
                    }
                }

                current = next;
            }

            match &aggregation {
                AggregationStrategy::LastWins => Ok(acc.unwrap_or(original)),
                AggregationStrategy::Original => Ok(original),
                AggregationStrategy::CollectAll => {
                    let mut out = original;
                    out.input.body = Body::Json(Value::Array(acc_bodies));
                    Ok(out)
                }
                AggregationStrategy::Custom(_) => Ok(acc.unwrap_or(original)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use camel_api::{BoxProcessorExt, Message, StreamBody, StreamMetadata};
    use futures::stream;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    use crate::stream_codec::{StreamSplitInput, resolve_format, resolve_incremental_codec};

    fn passthrough_pipeline() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    fn uppercase_pipeline() -> BoxProcessor {
        BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                if let Body::Text(s) = &ex.input.body {
                    ex.input.body = Body::Text(s.to_uppercase());
                }
                Ok(ex)
            })
        })
    }

    fn make_exchange(text: &str) -> Exchange {
        Exchange::new(Message::new(text))
    }

    fn test_expression(fragments: Vec<Exchange>) -> StreamingSplitExpression {
        Arc::new(move |_| {
            let frags = fragments.clone();
            Box::pin(stream::iter(frags.into_iter().map(Ok)))
        })
    }

    fn error_expression() -> StreamingSplitExpression {
        Arc::new(|_| {
            Box::pin(stream::iter(vec![Err(CamelError::ProcessorError(
                "stream error".to_string(),
            ))]))
        })
    }

    /// Build a `StreamingSplitExpression` that reads from `Body::Stream` and
    /// splits using the NdjsonCodec. Mirrors the resolution logic in
    /// `step_resolution.rs`.
    fn ndjson_stream_expression(config: camel_api::StreamSplitConfig) -> StreamingSplitExpression {
        Arc::new(move |exchange: Exchange| {
            let config = config.clone();
            let (stream_body, parent) = match &exchange.input.body {
                Body::Stream(sb) => (sb.clone(), {
                    let mut p = exchange.clone();
                    p.input.body = Body::Empty;
                    p
                }),
                _ => {
                    return Box::pin(futures::stream::once(async {
                        Err(CamelError::ProcessorError(
                            "streaming split requires Body::Stream".into(),
                        ))
                    }));
                }
            };

            let stream = match stream_body.stream.try_lock() {
                Ok(mut guard) => match guard.take() {
                    Some(s) => s,
                    None => {
                        return Box::pin(futures::stream::once(async {
                            Err(CamelError::ProcessorError(
                                "stream body already consumed".into(),
                            ))
                        }));
                    }
                },
                Err(_) => {
                    return Box::pin(futures::stream::once(async {
                        Err(CamelError::ProcessorError("stream body locked".into()))
                    }));
                }
            };

            let input = StreamSplitInput {
                parent,
                stream,
                metadata: stream_body.metadata,
            };

            match resolve_format(&config.format, &input.metadata) {
                Ok(f) => {
                    let codec = resolve_incremental_codec(&f);
                    let codec = match codec {
                        Ok(c) => c,
                        Err(e) => return Box::pin(futures::stream::once(async { Err(e) })),
                    };
                    codec.split(input, config)
                }
                Err(e) => Box::pin(futures::stream::once(async { Err(e) })),
            }
        })
    }

    // ---------------------------------------------------------------------------
    // Integration test: Body::Stream NDJSON → streaming split → Body::Json fragments
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_ndjson_body_stream_streaming_split() {
        // ── Arrange ────────────────────────────────────────────────────────────
        // 3 lines of NDJSON as a byte stream
        let ndjson_lines: Vec<Result<Bytes, CamelError>> = vec![
            Ok(Bytes::from("{\"id\":1,\"name\":\"a\"}\n")),
            Ok(Bytes::from("{\"id\":2,\"name\":\"b\"}\n")),
            Ok(Bytes::from("{\"id\":3,\"name\":\"c\"}\n")),
        ];
        let byte_stream = futures::stream::iter(ndjson_lines);

        let stream_body = StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(byte_stream)))),
            metadata: StreamMetadata {
                content_type: Some("application/x-ndjson".into()),
                size_hint: None,
                origin: Some("test://ndjson".into()),
            },
        };

        let ex = Exchange::new(Message::new(Body::Stream(stream_body)));

        // Streaming split config — Ndjson format
        let split_config = camel_api::StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Ndjson,
            ..Default::default()
        };

        // Recorder sub-pipeline: captures per-fragment body + properties
        #[allow(clippy::type_complexity)]
        let fragments: Arc<
            Mutex<Vec<(Option<serde_json::Value>, Option<Value>, Option<Value>)>>,
        > = Arc::new(Mutex::new(Vec::new()));
        let fragments_clone = Arc::clone(&fragments);
        let recorder = BoxProcessor::from_fn(move |ex: Exchange| {
            let frags = Arc::clone(&fragments_clone);
            Box::pin(async move {
                let body_json = match &ex.input.body {
                    Body::Json(v) => Some(v.clone()),
                    _ => None,
                };
                let split_index = ex.property(CAMEL_SPLIT_INDEX).cloned();
                let split_complete = ex.property(CAMEL_SPLIT_COMPLETE).cloned();
                let mut guard = frags.lock().await;
                guard.push((body_json, split_index, split_complete));
                Ok(ex)
            })
        });

        let expression = ndjson_stream_expression(split_config);

        // ── Act ────────────────────────────────────────────────────────────────
        let mut splitter = StreamingSplitterService::new(
            expression,
            recorder,
            AggregationStrategy::CollectAll,
            true, // stop_on_exception
        );

        let result = splitter
            .ready()
            .await
            .expect("splitter ready")
            .call(ex)
            .await
            .expect("splitter call");

        // ── Assert ─────────────────────────────────────────────────────────────
        let guard = fragments.lock().await;

        // 1. Three fragments were produced
        assert_eq!(guard.len(), 3, "expected 3 NDJSON fragments");

        // 2. Each fragment has Body::Json
        for (i, (body_json, _idx, _complete)) in guard.iter().enumerate() {
            assert!(
                body_json.is_some(),
                "fragment {i}: expected Body::Json body, got non-Json"
            );
        }

        // 3. Each fragment has CamelSplitIndex property (0, 1, 2)
        for (i, (_body, idx, _complete)) in guard.iter().enumerate() {
            assert_eq!(
                *idx,
                Some(Value::Number(serde_json::Number::from(i as u64))),
                "fragment {i}: CamelSplitIndex mismatch"
            );
        }

        // 4. CamelSplitComplete: first two false, last one true
        assert_eq!(
            guard[0].2,
            Some(Value::Bool(false)),
            "first fragment: CamelSplitComplete should be false"
        );
        assert_eq!(
            guard[1].2,
            Some(Value::Bool(false)),
            "second fragment: CamelSplitComplete should be false"
        );
        assert_eq!(
            guard[2].2,
            Some(Value::Bool(true)),
            "last fragment: CamelSplitComplete should be true"
        );

        // 5. CollectAll aggregated into JSON array with correct values
        match &result.input.body {
            Body::Json(v) => {
                let arr = v.as_array().expect("CollectAll result should be array");
                assert_eq!(arr.len(), 3);
                assert_eq!(arr[0], serde_json::json!({"id":1,"name":"a"}));
                assert_eq!(arr[1], serde_json::json!({"id":2,"name":"b"}));
                assert_eq!(arr[2], serde_json::json!({"id":3,"name":"c"}));
            }
            other => panic!("expected Body::Json from CollectAll, got {other:?}"),
        }

        // 6. Original stream body sanitized (already Empty, becomes part of aggregate)
        //    The aggregate exchange's body is Json, not Stream
        assert!(
            matches!(result.input.body, Body::Json(_)),
            "aggregate body should be Json, not Stream"
        );
    }

    // ---------------------------------------------------------------------------
    // Integration test: Empty Body::Stream → aggregate result is empty
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_ndjson_body_stream_empty_stream() {
        // ── Arrange ────────────────────────────────────────────────────────────
        // Empty byte stream
        let byte_stream = futures::stream::iter(Vec::<Result<Bytes, CamelError>>::new());

        let stream_body = StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(byte_stream)))),
            metadata: StreamMetadata {
                content_type: Some("application/x-ndjson".into()),
                size_hint: None,
                origin: None,
            },
        };

        let mut ex = Exchange::new(Message::new(Body::Stream(stream_body)));
        ex.set_property("trace_id", Value::String("empty-test".into()));

        let split_config = camel_api::StreamSplitConfig {
            format: camel_api::StreamSplitFormat::Ndjson,
            ..Default::default()
        };

        let expression = ndjson_stream_expression(split_config);

        // ── Act ────────────────────────────────────────────────────────────────
        let mut splitter = StreamingSplitterService::new(
            expression,
            passthrough_pipeline(),
            AggregationStrategy::CollectAll,
            true,
        );

        let result = splitter
            .ready()
            .await
            .expect("splitter ready")
            .call(ex)
            .await
            .expect("splitter call");

        // ── Assert ─────────────────────────────────────────────────────────────
        // Empty stream → CollectAll produces Body::Json([])
        match &result.input.body {
            Body::Json(v) => {
                let arr = v.as_array().expect("CollectAll result should be array");
                assert!(
                    arr.is_empty(),
                    "empty stream should produce empty array, got {arr:?}"
                );
            }
            other => {
                panic!("expected Body::Json([]) from CollectAll on empty stream, got {other:?}")
            }
        }

        // Properties preserved
        assert_eq!(
            result.property("trace_id"),
            Some(&Value::String("empty-test".into()))
        );
    }

    #[tokio::test]
    async fn test_streaming_sequential_last_wins() {
        let expr = test_expression(vec![
            make_exchange("a"),
            make_exchange("b"),
            make_exchange("c"),
        ]);
        let mut svc = StreamingSplitterService::new(
            expr,
            uppercase_pipeline(),
            AggregationStrategy::LastWins,
            true,
        );

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("original"))
            .await
            .unwrap();
        assert_eq!(result.input.body.as_text(), Some("C"));
    }

    #[tokio::test]
    async fn test_streaming_sequential_original() {
        let expr = test_expression(vec![make_exchange("a"), make_exchange("b")]);
        let mut svc = StreamingSplitterService::new(
            expr,
            uppercase_pipeline(),
            AggregationStrategy::Original,
            true,
        );

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("original"))
            .await
            .unwrap();
        assert_eq!(result.input.body.as_text(), Some("original"));
    }

    #[tokio::test]
    async fn test_streaming_stop_on_exception() {
        let expr = test_expression(vec![make_exchange("a"), make_exchange("b")]);
        let fail_pipeline = BoxProcessor::from_fn(|_| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        });
        let mut svc =
            StreamingSplitterService::new(expr, fail_pipeline, AggregationStrategy::LastWins, true);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("original"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_streaming_empty_stream() {
        let expr: StreamingSplitExpression = Arc::new(|_| Box::pin(futures::stream::empty()));
        let mut svc = StreamingSplitterService::new(
            expr,
            passthrough_pipeline(),
            AggregationStrategy::LastWins,
            true,
        );

        let mut ex = make_exchange("original");
        ex.set_property("marker", Value::Bool(true));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("original"));
        assert_eq!(result.property("marker"), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_streaming_error_in_expression() {
        let mut svc = StreamingSplitterService::new(
            error_expression(),
            passthrough_pipeline(),
            AggregationStrategy::LastWins,
            true,
        );

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("original"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_streaming_cancellation() {
        let expr = test_expression(vec![make_exchange("a"), make_exchange("b")]);
        let slow_pipeline = BoxProcessor::from_fn(|ex| {
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Ok(ex)
            })
        });
        let svc =
            StreamingSplitterService::new(expr, slow_pipeline, AggregationStrategy::LastWins, true);
        svc.cancel();

        let mut svc_clone = svc.clone();
        let result = svc_clone
            .ready()
            .await
            .unwrap()
            .call(make_exchange("original"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_streaming_sequential_collect_all() {
        let expr = test_expression(vec![
            make_exchange("a"),
            make_exchange("b"),
            make_exchange("c"),
        ]);
        let mut svc = StreamingSplitterService::new(
            expr,
            uppercase_pipeline(),
            AggregationStrategy::CollectAll,
            true,
        );

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("original"))
            .await
            .unwrap();
        let expected = serde_json::json!(["A", "B", "C"]);
        match &result.input.body {
            Body::Json(v) => assert_eq!(*v, expected),
            other => panic!("expected JSON body, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_streaming_sequential_custom_aggregation() {
        let joiner: Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync> =
            Arc::new(|mut acc: Exchange, next: Exchange| {
                let acc_text = acc.input.body.as_text().unwrap_or("").to_string();
                let next_text = next.input.body.as_text().unwrap_or("").to_string();
                acc.input.body = Body::Text(format!("{acc_text}+{next_text}"));
                acc
            });

        let expr = test_expression(vec![
            make_exchange("a"),
            make_exchange("b"),
            make_exchange("c"),
        ]);
        let mut svc = StreamingSplitterService::new(
            expr,
            uppercase_pipeline(),
            AggregationStrategy::Custom(joiner),
            true,
        );

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("original"))
            .await
            .unwrap();
        assert_eq!(result.input.body.as_text(), Some("A+B+C"));
    }

    #[tokio::test]
    async fn test_streaming_error_continue_on_exception() {
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_clone = call_count.clone();
        let fail_on_first = BoxProcessor::from_fn(move |ex: Exchange| {
            let count = count_clone.clone();
            Box::pin(async move {
                let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    Err(CamelError::ProcessorError("first fails".into()))
                } else {
                    Ok(ex)
                }
            })
        });

        let expr = test_expression(vec![make_exchange("a"), make_exchange("b")]);
        let mut svc = StreamingSplitterService::new(
            expr,
            fail_on_first,
            AggregationStrategy::LastWins,
            false,
        );

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("original"))
            .await
            .unwrap();
        assert_eq!(result.input.body.as_text(), Some("b"));
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_streaming_metadata_lookahead() {
        let recorder = BoxProcessor::from_fn(|ex: Exchange| {
            Box::pin(async move {
                let idx = ex.property(CAMEL_SPLIT_INDEX).cloned();
                let complete = ex.property(CAMEL_SPLIT_COMPLETE).cloned();
                let body = serde_json::json!({
                    "index": idx,
                    "complete": complete,
                });
                let mut out = ex;
                out.input.body = Body::Json(body);
                Ok(out)
            })
        });

        let expr = test_expression(vec![
            make_exchange("x"),
            make_exchange("y"),
            make_exchange("z"),
        ]);
        let mut svc =
            StreamingSplitterService::new(expr, recorder, AggregationStrategy::CollectAll, true);

        let result = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange("original"))
            .await
            .unwrap();
        let expected = serde_json::json!([
            {"index": 0, "complete": false},
            {"index": 1, "complete": false},
            {"index": 2, "complete": true},
        ]);
        match &result.input.body {
            Body::Json(v) => assert_eq!(*v, expected),
            other => panic!("expected JSON body, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_streaming_split_sanitizes_stream_body_in_original() {
        let chunks = vec![Ok(Bytes::from("line1\n"))];
        let stream = futures::stream::iter(chunks);
        let sb = StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: Default::default(),
        };
        let ex = Exchange::new(Message::new(Body::Stream(sb)));

        let expression =
            test_expression(vec![Exchange::new(Message::new(Body::Text("frag".into())))]);
        let sub_pipeline = passthrough_pipeline();
        let mut splitter = StreamingSplitterService::new(
            expression,
            sub_pipeline,
            AggregationStrategy::Original,
            true,
        );

        let result = splitter
            .ready()
            .await
            .expect("ready")
            .call(ex)
            .await
            .expect("call");
        assert!(
            matches!(result.input.body, Body::Empty),
            "original body should be sanitized to Empty"
        );
    }
}
