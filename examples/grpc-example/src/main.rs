use camel_api::CamelError;
use camel_api::body::Body;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_grpc::GrpcComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(GrpcComponent::new());

    let proto_path = format!("{}/protos/helloworld.proto", env!("CARGO_MANIFEST_DIR"));

    // Consumer route (listens for gRPC requests on port 50051)
    let consumer_route = RouteBuilder::from(&format!(
        "grpc://0.0.0.0:50051/helloworld.Greeter/SayHello?protoFile={}",
        proto_path
    ))
    .set_body(Body::Json(
        serde_json::json!({"message": "Hello from consumer!"}),
    ))
    .to("log:grpc-consumer?showBody=true")
    .build()?;

    // Producer route (calls the consumer)
    let producer_route = RouteBuilder::from("timer:grpc-tick?period=3000&repeatCount=3")
        .set_body(Body::Json(serde_json::json!({"name": "World"})))
        .to(&format!(
            "grpc://127.0.0.1:50051/helloworld.Greeter/SayHello?protoFile={}",
            proto_path
        ))
        .to("log:grpc-response?showBody=true")
        .build()?;

    ctx.add_route_definition(consumer_route).await?;
    ctx.add_route_definition(producer_route).await?;
    ctx.start().await?;

    println!(
        "gRPC example running. Consumer on port 50051, producer sends 3 requests every 3 seconds."
    );
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    println!("Done.");

    Ok(())
}
