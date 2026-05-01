use std::path::PathBuf;

use camel_component_api::{Body, Exchange, Message};
use camel_component_grpc::producer::GrpcProducer;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status};
use tower::Service;

mod helloworld {
    tonic::include_proto!("helloworld");
}

use helloworld::greeter_server::{Greeter, GreeterServer};
use helloworld::{HelloReply, HelloRequest};

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
