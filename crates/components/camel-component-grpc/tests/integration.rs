use std::path::PathBuf;

use camel_component_api::{Body, ConcurrencyModel, Consumer, ConsumerContext, Exchange, Message};
use camel_component_grpc::GrpcMode;
use camel_component_grpc::consumer::GrpcConsumer;
use camel_component_grpc::consumer::take_stream_observer;
use camel_component_grpc::producer::GrpcProducer;
use futures::StreamExt;
use http::uri::PathAndQuery;
use prost::Message as ProstMessage;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use tower::Service;

mod helloworld {
    tonic::include_proto!("helloworld");
}

mod streaming {
    tonic::include_proto!("streaming");
}

use helloworld::greeter_server::{Greeter, GreeterServer};
use helloworld::{HelloReply, HelloRequest};
use streaming::stream_service_client::StreamServiceClient;
use streaming::stream_service_server::{StreamService, StreamServiceServer};
use streaming::*;

struct GreeterImpl;

#[tonic::async_trait]
impl Greeter for GreeterImpl {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        let name = request.into_inner().name;
        Ok(Response::new(HelloReply {
            message: format!("Hello {name}"),
        }))
    }
}

// ── Mock streaming server ──────────────────────────────────────────────────

struct StreamServiceImpl;

#[tonic::async_trait]
impl StreamService for StreamServiceImpl {
    type ServerListStream = tokio_stream::wrappers::ReceiverStream<Result<ItemResponse, Status>>;
    type BidiEchoStream = tokio_stream::wrappers::ReceiverStream<Result<EchoResponse, Status>>;

    async fn server_list(
        &self,
        request: tonic::Request<ListRequest>,
    ) -> Result<tonic::Response<Self::ServerListStream>, Status> {
        let count = request.into_inner().count;
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        tokio::spawn(async move {
            for i in 0..count {
                let item = ItemResponse {
                    index: i,
                    name: format!("item-{i}"),
                };
                let _ = tx.send(Ok(item)).await;
            }
        });
        Ok(tonic::Response::new(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
    }

    async fn client_sum(
        &self,
        request: tonic::Request<tonic::Streaming<NumberRequest>>,
    ) -> Result<tonic::Response<SumResponse>, Status> {
        let mut stream = request.into_inner();
        let mut total: i32 = 0;
        while let Some(item) = stream.next().await {
            total += item?.value;
        }
        Ok(tonic::Response::new(SumResponse { total }))
    }

    async fn bidi_echo(
        &self,
        request: tonic::Request<tonic::Streaming<EchoRequest>>,
    ) -> Result<tonic::Response<Self::BidiEchoStream>, Status> {
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        tokio::spawn(async move {
            let mut stream = request.into_inner();
            let mut seq = 0;
            while let Some(item) = stream.next().await {
                match item {
                    Ok(req) => {
                        seq += 1;
                        let resp = EchoResponse {
                            message: req.message,
                            sequence: seq,
                        };
                        if tx.send(Ok(resp)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(tonic::Response::new(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
    }
}

// ── Existing unary tests (with GrpcMode::Unary) ────────────────────────────

#[tokio::test]
async fn grpc_producer_roundtrip_json() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let incoming = TcpListenerStream::new(listener);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(GreeterServer::new(GreeterImpl))
            .serve_with_incoming(incoming)
            .await
            .expect("serve");
    });

    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
    let mut producer = GrpcProducer::new(
        format!("http://127.0.0.1:{}", addr.port()),
        proto_path,
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    )
    .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Json(
        serde_json::json!({"name": "World"}),
    )));
    let out = producer.call(exchange).await.expect("call");
    match out.input.body {
        Body::Json(v) => assert_eq!(v["message"], "Hello World"),
        other => panic!("expected json body, got {other:?}"),
    }
}

#[tokio::test]
async fn grpc_consumer_roundtrip_json() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let port = addr.port();

    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");

    let mut consumer = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/helloworld.Greeter/SayHello".to_string(),
        proto_path,
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    );

    assert_eq!(
        consumer.concurrency_model(),
        ConcurrencyModel::Concurrent { max: None }
    );

    let (route_tx, mut route_rx) = tokio::sync::mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(route_tx, cancel_token.clone());

    let consumer_task = tokio::spawn(async move {
        consumer
            .start_with_listener(ctx, listener)
            .await
            .expect("consumer start");
    });

    let pipeline_task = tokio::spawn(async move {
        if let Some(envelope) = route_rx.recv().await {
            let resp = Exchange::new(Message::new(Body::Json(
                serde_json::json!({"message": "Hello World"}),
            )));
            let _ = envelope.reply_tx.unwrap().send(Ok(resp));
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();
    let mut client = helloworld::greeter_client::GreeterClient::new(channel);

    let response = client
        .say_hello(helloworld::HelloRequest {
            name: "World".to_string(),
        })
        .await
        .expect("call");

    assert_eq!(response.into_inner().message, "Hello World");

    cancel_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        consumer_task.await.unwrap();
        pipeline_task.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn grpc_consumer_bad_proto_startup_fails() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let port = addr.port();

    let mut consumer = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/helloworld.Greeter/SayHello".to_string(),
        PathBuf::from("/nonexistent/path.proto"),
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    );

    let (route_tx, _route_rx) = tokio::sync::mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(route_tx, cancel_token.clone());

    let result = consumer.start_with_listener(ctx, listener).await;
    assert!(result.is_err(), "startup should fail with bad proto path");

    // Verify no stale dispatch entry: a request to the path should return Unimplemented
    // (not UNAVAILABLE from a stale sender). The server may still be running on the port
    // (OnceCell was initialized), but the dispatch table should NOT have the path.
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();

    let path = http::uri::PathAndQuery::from_static("/helloworld.Greeter/SayHello");
    let mut grpc = tonic::client::Grpc::new(channel);
    let request = Request::new(vec![0u8]);

    grpc.ready().await.expect("ready");
    let result = grpc
        .unary(request, path, camel_component_grpc::codec::RawBytesCodec)
        .await;

    assert!(
        result.is_err(),
        "expected error for path with no dispatch entry"
    );
    let status = result.unwrap_err();
    assert_eq!(
        status.code(),
        tonic::Code::Unimplemented,
        "should be Unimplemented (no dispatch entry), not {:?} (stale sender)",
        status.code()
    );
}

#[tokio::test]
async fn grpc_consumer_unknown_path_returns_unimplemented() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");

    let mut consumer = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/helloworld.Greeter/SayHello".to_string(),
        proto_path,
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    );

    let (route_tx, mut route_rx) = tokio::sync::mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(route_tx, cancel_token.clone());

    let consumer_task = tokio::spawn(async move {
        consumer
            .start_with_listener(ctx, listener)
            .await
            .expect("start");
    });

    let pipeline_task =
        tokio::spawn(async move { while let Some(_envelope) = route_rx.recv().await {} });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();

    let path = PathAndQuery::from_static("/unknown.Service/Method");
    let mut grpc = tonic::client::Grpc::new(channel);
    let request = Request::new(vec![0u8]);

    grpc.ready().await.expect("ready");
    let result = grpc
        .unary(request, path, camel_component_grpc::codec::RawBytesCodec)
        .await;

    assert!(result.is_err(), "expected error for unknown path");
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unimplemented);

    cancel_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        consumer_task.await.unwrap();
        pipeline_task.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn grpc_consumer_stop_then_request_returns_unimplemented() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");

    let mut consumer = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/helloworld.Greeter/SayHello".to_string(),
        proto_path.clone(),
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    );

    let (route_tx, mut route_rx) = tokio::sync::mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(route_tx, cancel_token.clone());

    let consumer_task = tokio::spawn(async move {
        consumer
            .start_with_listener(ctx, listener)
            .await
            .expect("start");
    });

    let pipeline_task = tokio::spawn(async move {
        if let Some(envelope) = route_rx.recv().await {
            let resp = Exchange::new(Message::new(Body::Json(
                serde_json::json!({"message": "ok"}),
            )));
            let _ = envelope.reply_tx.unwrap().send(Ok(resp));
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();
    let mut client = helloworld::greeter_client::GreeterClient::new(channel.clone());

    let resp = client
        .say_hello(helloworld::HelloRequest {
            name: "test".to_string(),
        })
        .await;
    assert!(resp.is_ok(), "consumer should work before stop");

    cancel_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        consumer_task.await.unwrap();
        pipeline_task.await.unwrap();
    })
    .await;

    let mut client2 = helloworld::greeter_client::GreeterClient::new(channel);
    let result = client2
        .say_hello(helloworld::HelloRequest {
            name: "test".to_string(),
        })
        .await;
    assert!(result.is_err(), "expected error after stop");
    assert_eq!(result.unwrap_err().code(), tonic::Code::Unimplemented);
}

#[tokio::test]
async fn grpc_consumer_multiple_paths_same_port() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");

    let mut consumer1 = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/helloworld.Greeter/SayHello".to_string(),
        proto_path.clone(),
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    );

    let (route_tx1, mut route_rx1) = tokio::sync::mpsc::channel(16);
    let cancel_token1 = CancellationToken::new();
    let ctx1 = ConsumerContext::new(route_tx1, cancel_token1.clone());

    let consumer1_task = tokio::spawn(async move {
        consumer1
            .start_with_listener(ctx1, listener)
            .await
            .expect("start consumer1");
    });

    let pipeline1_task = tokio::spawn(async move {
        while let Some(envelope) = route_rx1.recv().await {
            let resp = Exchange::new(Message::new(Body::Json(
                serde_json::json!({"message": "hello from path1"}),
            )));
            let _ = envelope.reply_tx.unwrap().send(Ok(resp));
        }
    });

    let mut consumer2 = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/custom.Service/CustomMethod".to_string(),
        proto_path,
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    );

    let (route_tx2, mut route_rx2) = tokio::sync::mpsc::channel(16);
    let cancel_token2 = CancellationToken::new();
    let ctx2 = ConsumerContext::new(route_tx2, cancel_token2.clone());

    let consumer2_task = tokio::spawn(async move {
        consumer2.start(ctx2).await.expect("start consumer2");
    });

    let pipeline2_task = tokio::spawn(async move {
        while let Some(envelope) = route_rx2.recv().await {
            let resp = Exchange::new(Message::new(Body::Json(
                serde_json::json!({"message": "hello from path2"}),
            )));
            let _ = envelope.reply_tx.unwrap().send(Ok(resp));
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();

    let mut client1 = helloworld::greeter_client::GreeterClient::new(channel.clone());
    let resp1 = client1
        .say_hello(helloworld::HelloRequest {
            name: "path1".to_string(),
        })
        .await
        .expect("call path1");
    assert_eq!(resp1.into_inner().message, "hello from path1");

    let req_body = {
        let req = helloworld::HelloRequest {
            name: "path2".to_string(),
        };
        let mut buf = Vec::new();
        ProstMessage::encode(&req, &mut buf).unwrap();
        buf
    };
    let path2 = PathAndQuery::from_static("/custom.Service/CustomMethod");
    let mut grpc = tonic::client::Grpc::new(channel);
    let request = Request::new(req_body);
    grpc.ready().await.expect("ready");
    let resp2 = grpc
        .unary(request, path2, camel_component_grpc::codec::RawBytesCodec)
        .await
        .expect("call path2");
    let resp2_msg: helloworld::HelloReply =
        ProstMessage::decode(resp2.into_inner().as_slice()).expect("decode");
    assert_eq!(resp2_msg.message, "hello from path2");

    cancel_token1.cancel();
    cancel_token2.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        consumer1_task.await.unwrap();
        consumer2_task.await.unwrap();
        pipeline1_task.await.unwrap();
        pipeline2_task.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn grpc_consumer_invalid_body_returns_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");

    let mut consumer = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/helloworld.Greeter/SayHello".to_string(),
        proto_path,
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    );

    let (route_tx, mut route_rx) = tokio::sync::mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(route_tx, cancel_token.clone());

    let consumer_task = tokio::spawn(async move {
        consumer
            .start_with_listener(ctx, listener)
            .await
            .expect("start");
    });

    let pipeline_task =
        tokio::spawn(async move { while let Some(_envelope) = route_rx.recv().await {} });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Send raw garbage bytes (not valid protobuf) via raw gRPC client
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();
    let path = PathAndQuery::from_static("/helloworld.Greeter/SayHello");
    let mut grpc = tonic::client::Grpc::new(channel);
    let garbage_bytes = vec![0xFF, 0xFE, 0xFD, 0xFC, 0xFB, 0xFA];
    let request = Request::new(garbage_bytes);

    grpc.ready().await.expect("ready");
    let result = grpc
        .unary(request, path, camel_component_grpc::codec::RawBytesCodec)
        .await;

    assert!(result.is_err(), "expected error for invalid protobuf body");
    let status = result.unwrap_err();
    assert_eq!(
        status.code(),
        tonic::Code::InvalidArgument,
        "should be InvalidArgument for decode failure, got {:?}",
        status.code()
    );

    cancel_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        consumer_task.await.unwrap();
        pipeline_task.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn grpc_consumer_duplicate_path_fails() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");

    // Start first consumer
    let mut consumer1 = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/helloworld.Greeter/SayHello".to_string(),
        proto_path.clone(),
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    );

    let (route_tx1, mut route_rx1) = tokio::sync::mpsc::channel(16);
    let cancel_token1 = CancellationToken::new();
    let ctx1 = ConsumerContext::new(route_tx1, cancel_token1.clone());

    let consumer1_task = tokio::spawn(async move {
        consumer1
            .start_with_listener(ctx1, listener)
            .await
            .expect("start consumer1");
    });

    let pipeline1_task =
        tokio::spawn(async move { while let Some(_envelope) = route_rx1.recv().await {} });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Try to start a second consumer on the same path and port — should fail
    let mut consumer2 = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/helloworld.Greeter/SayHello".to_string(),
        proto_path,
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    );

    let (route_tx2, _route_rx2) = tokio::sync::mpsc::channel(16);
    let cancel_token2 = CancellationToken::new();
    let ctx2 = ConsumerContext::new(route_tx2, cancel_token2.clone());

    // Need a new listener bound to the same port — this will fail since port is already in use
    // But the duplicate path check happens before binding, so we use start() which uses get_or_spawn
    // Actually, start_with_listener needs a listener. Let's use start() instead which binds internally.
    // But start() also uses get_or_spawn which shares the dispatch table.
    // The duplicate check is in start_inner which checks dispatch table for the path.
    // So we need to call start() on consumer2 — it will get the same dispatch table,
    // then try to insert the same path, and fail.
    let result = consumer2.start(ctx2).await;
    assert!(result.is_err(), "second consumer on same path should fail");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("duplicate"),
        "error should mention duplicate path, got: {err}"
    );

    cancel_token1.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        consumer1_task.await.unwrap();
        pipeline1_task.await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn grpc_consumer_pipeline_error_returns_internal() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");

    let mut consumer = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/helloworld.Greeter/SayHello".to_string(),
        proto_path,
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
    );

    let (route_tx, mut route_rx) = tokio::sync::mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(route_tx, cancel_token.clone());

    let consumer_task = tokio::spawn(async move {
        consumer
            .start_with_listener(ctx, listener)
            .await
            .expect("start");
    });

    // Pipeline drops the reply (simulates error) — receive envelope but don't send reply
    let pipeline_task = tokio::spawn(async move {
        if let Some(_envelope) = route_rx.recv().await {
            // Drop the envelope without sending a reply — simulates pipeline error
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();
    let mut client = helloworld::greeter_client::GreeterClient::new(channel);

    let result = client
        .say_hello(helloworld::HelloRequest {
            name: "World".to_string(),
        })
        .await;

    assert!(result.is_err(), "expected error when pipeline drops reply");
    let status = result.unwrap_err();
    assert_eq!(
        status.code(),
        tonic::Code::Internal,
        "should be Internal for pipeline error, got {:?}",
        status.code()
    );

    cancel_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        consumer_task.await.unwrap();
        pipeline_task.await.unwrap();
    })
    .await;
}

// ── Consumer streaming tests ───────────────────────────────────────────────

#[tokio::test]
async fn grpc_consumer_server_streaming_roundtrip() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/streaming.proto");

    let mut consumer = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/streaming.StreamService/ServerList".to_string(),
        proto_path,
        "streaming.StreamService".to_string(),
        "ServerList".to_string(),
        GrpcMode::ServerStreaming,
    );

    let (route_tx, mut route_rx) = tokio::sync::mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(route_tx, cancel_token.clone());

    let consumer_task = tokio::spawn(async move {
        consumer
            .start_with_listener(ctx, listener)
            .await
            .expect("consumer start");
    });

    // Pipeline: receive exchange, take observer, send items via on_next, then on_completed
    let pipeline_task = tokio::spawn(async move {
        if let Some(envelope) = route_rx.recv().await {
            let exchange = &envelope.exchange;
            if let Some(observer) = take_stream_observer(exchange) {
                // Send 5 items
                for i in 0..5 {
                    let item =
                        serde_json::json!({ "index": i, "name": format!("pipeline-item-{i}") });
                    if let Err(e) = observer.on_next(item).await {
                        eprintln!("on_next error: {e}");
                        break;
                    }
                }
                observer.on_completed().await;
            }
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Client calls ServerList via generated tonic client
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();
    let mut client = StreamServiceClient::new(channel);

    let mut stream = client
        .server_list(ListRequest { count: 5 })
        .await
        .expect("call")
        .into_inner();

    let mut items = Vec::new();
    while let Some(item) = stream.next().await {
        items.push(item.expect("item"));
    }

    assert_eq!(items.len(), 5, "should receive 5 items");
    for (i, item) in items.iter().enumerate() {
        assert_eq!(item.index as usize, i);
        assert_eq!(item.name, format!("pipeline-item-{i}"));
    }

    cancel_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        consumer_task.await.unwrap();
        pipeline_task.await.unwrap();
    })
    .await;
}

// ── Producer streaming tests ───────────────────────────────────────────────

/// Start the mock streaming server on a random port, return the port.
async fn start_streaming_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let incoming = TcpListenerStream::new(listener);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(StreamServiceServer::new(StreamServiceImpl))
            .serve_with_incoming(incoming)
            .await
            .expect("serve");
    });
    port
}

#[tokio::test]
async fn grpc_producer_server_streaming() {
    let port = start_streaming_server().await;
    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/streaming.proto");

    let mut producer = GrpcProducer::new(
        format!("http://127.0.0.1:{port}"),
        proto_path,
        "streaming.StreamService".to_string(),
        "ServerList".to_string(),
        GrpcMode::ServerStreaming,
    )
    .expect("producer");

    // Call with single JSON body (count=3)
    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"count": 3}))));
    let out = producer.call(exchange).await.expect("call");

    // Response should be JSON array of items
    // Note: prost-reflect omits default values (0 for int32) in JSON serialization
    match out.input.body {
        Body::Json(v) => {
            let arr = v.as_array().expect("expected array");
            assert_eq!(arr.len(), 3);
            for (i, item) in arr.iter().enumerate() {
                let idx = item.get("index").and_then(|v| v.as_i64()).unwrap_or(0);
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                assert_eq!(idx, i as i64, "item {} index mismatch", i);
                assert_eq!(name, format!("item-{i}"), "item {} name mismatch", i);
            }
        }
        other => panic!("expected json body, got {other:?}"),
    }
}

#[tokio::test]
async fn grpc_producer_client_streaming() {
    let port = start_streaming_server().await;
    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/streaming.proto");

    let mut producer = GrpcProducer::new(
        format!("http://127.0.0.1:{port}"),
        proto_path,
        "streaming.StreamService".to_string(),
        "ClientSum".to_string(),
        GrpcMode::ClientStreaming,
    )
    .expect("producer");

    // Call with JSON array body (values 1, 2, 3, 4, 5)
    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!([
        {"value": 1},
        {"value": 2},
        {"value": 3},
        {"value": 4},
        {"value": 5},
    ]))));
    let out = producer.call(exchange).await.expect("call");

    // Response should be single JSON object with total=15
    match out.input.body {
        Body::Json(v) => {
            assert_eq!(v["total"], serde_json::json!(15));
        }
        other => panic!("expected json body, got {other:?}"),
    }
}

#[tokio::test]
async fn grpc_producer_bidi_streaming() {
    let port = start_streaming_server().await;
    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/streaming.proto");

    let mut producer = GrpcProducer::new(
        format!("http://127.0.0.1:{port}"),
        proto_path,
        "streaming.StreamService".to_string(),
        "BidiEcho".to_string(),
        GrpcMode::Bidi,
    )
    .expect("producer");

    // Call with JSON array body (3 echo messages)
    let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!([
        {"message": "hello"},
        {"message": "world"},
        {"message": "test"},
    ]))));
    let out = producer.call(exchange).await.expect("call");

    // Response should be JSON array of echoed messages with sequence numbers
    match out.input.body {
        Body::Json(v) => {
            let arr = v.as_array().expect("expected array");
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0]["message"], "hello");
            assert_eq!(arr[0]["sequence"], 1);
            assert_eq!(arr[1]["message"], "world");
            assert_eq!(arr[1]["sequence"], 2);
            assert_eq!(arr[2]["message"], "test");
            assert_eq!(arr[2]["sequence"], 3);
        }
        other => panic!("expected json body, got {other:?}"),
    }
}
