use std::time::Duration;

use camel_component_api::NoOpComponentContext;
use camel_component_api::component::Component;
use camel_component_api::endpoint::Endpoint;
use camel_component_file::FileComponent;
use tokio::fs;

async fn write_file(dir: &std::path::Path, name: &str, content: &str) {
    fs::write(dir.join(name), content).await.unwrap();
}

fn file_endpoint(uri: &str) -> Box<dyn Endpoint> {
    let comp = FileComponent::default();
    comp.create_endpoint(uri, &NoOpComponentContext)
        .expect("endpoint creation should succeed")
}

#[tokio::test]
async fn polling_consumer_reads_file_into_body() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_str().unwrap();
    let uri = format!("file:{dir}?fileName=input.txt&noop=true");
    write_file(tmp.path(), "input.txt", "{\"k\":\"v\"}").await;

    let endpoint = file_endpoint(&uri);
    let mut poller = endpoint
        .polling_consumer()
        .expect("FileEndpoint should expose a PollingConsumer");
    let exchange = poller
        .receive(Duration::from_millis(500))
        .await
        .unwrap()
        .expect("file exists");

    let body_bytes = exchange.input.body.materialize().await.unwrap();
    assert_eq!(body_bytes.as_ref(), b"{\"k\":\"v\"}");
}

#[tokio::test]
async fn polling_consumer_returns_none_when_no_file_with_zero_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_str().unwrap();
    let uri = format!("file:{dir}?noop=true");
    let endpoint = file_endpoint(&uri);

    let mut poller = endpoint
        .polling_consumer()
        .expect("FileEndpoint should expose a PollingConsumer");
    let result = poller.receive(Duration::ZERO).await.unwrap();
    assert!(result.is_none(), "no files in dir, expected None");
}

#[tokio::test]
async fn polling_consumer_deletes_file_after_receive_when_delete_true() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_str().unwrap();
    let uri = format!("file:{dir}?fileName=in.txt&delete=true");
    write_file(tmp.path(), "in.txt", "x").await;

    let endpoint = file_endpoint(&uri);
    let mut poller = endpoint
        .polling_consumer()
        .expect("FileEndpoint should expose a PollingConsumer");
    let _ = poller.receive(Duration::from_millis(500)).await.unwrap();

    assert!(
        !tmp.path().join("in.txt").exists(),
        "file should be deleted after receive"
    );
}

#[tokio::test]
async fn polling_consumer_idempotent_second_call_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_str().unwrap();
    let uri = format!("file:{dir}?fileName=in.txt&idempotentKey=FileName");
    write_file(tmp.path(), "in.txt", "y").await;

    let endpoint = file_endpoint(&uri);
    let mut poller = endpoint
        .polling_consumer()
        .expect("FileEndpoint should expose a PollingConsumer");
    let _first = poller.receive(Duration::from_millis(500)).await.unwrap();
    let second = poller.receive(Duration::from_millis(500)).await.unwrap();
    assert!(
        second.is_none(),
        "second poll should return None (idempotency or file gone)"
    );
}
