mod helloworld {
    tonic::include_proto!("helloworld");
}

use camel_api::CamelError;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_grpc::GrpcComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use helloworld::{
    HelloReply, HelloRequest,
    greeter_server::{Greeter, GreeterServer},
};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status, transport::Server};

#[derive(Default)]
struct MyGreeter;

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        let name = request.into_inner().name;
        Ok(Response::new(HelloReply {
            message: format!("Hello {}!", name),
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    println!("Starting gRPC server on random local port...");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|e| CamelError::Io(e.to_string()))?
        .port();

    tokio::spawn(async move {
        Server::builder()
            .add_service(GreeterServer::new(MyGreeter))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(GrpcComponent::new());

    let proto_path = format!("{}/protos/helloworld.proto", env!("CARGO_MANIFEST_DIR"));
    let grpc_uri = format!(
        "grpc://127.0.0.1:{}/helloworld.Greeter/SayHello?protoFile={}",
        port, proto_path
    );

    println!("Server listening on 127.0.0.1:{}", port);
    println!("Using proto file: {}", proto_path);
    println!("Sending timer-triggered requests to: {}", grpc_uri);

    let route = RouteBuilder::from("timer:grpc-tick?period=3000&repeatCount=3")
        .set_body(Body::Json(serde_json::json!({"name": "World"})))
        .to(&grpc_uri)
        .to("log:grpc-response?showBody=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("gRPC example running. It will send 3 requests every 3 seconds.");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    println!("Done.");

    Ok(())
}
