use camel_component_api::{Body, Exchange, Message};
use camel_component_opensearch::{OpenSearchEndpointConfig, OpenSearchProducer};
use futures_util::future::poll_fn;
use tower::Service;

async fn call_ready(producer: &mut OpenSearchProducer, exchange: Exchange) {
    poll_fn(|cx| producer.poll_ready(cx))
        .await
        .expect("producer ready");
    producer
        .call(exchange)
        .await
        .expect("operation should succeed");
}

#[tokio::test]
#[ignore = "requires live OpenSearch at localhost:9200"]
async fn live_index_search_delete_roundtrip() {
    let index = "camel_live_test_idx";

    let mut producer = OpenSearchProducer::new(
        OpenSearchEndpointConfig::from_uri(&format!(
            "opensearch://localhost:9200/{index}?operation=INDEX"
        ))
        .expect("valid endpoint config"),
    );

    let mut msg = Message::new(Body::Json(serde_json::json!({"msg": "hello"})));
    msg.set_header("CamelOpenSearch.Id", serde_json::json!("doc-1"));
    call_ready(&mut producer, Exchange::new(msg)).await;

    let mut search = OpenSearchProducer::new(
        OpenSearchEndpointConfig::from_uri(&format!(
            "opensearch://localhost:9200/{index}?operation=SEARCH&size=1&from=0"
        ))
        .expect("valid search endpoint config"),
    );
    call_ready(
        &mut search,
        Exchange::new(Message::new(Body::Json(
            serde_json::json!({"query": {"match_all": {}}}),
        ))),
    )
    .await;

    let mut delete = OpenSearchProducer::new(
        OpenSearchEndpointConfig::from_uri(&format!(
            "opensearch://localhost:9200/{index}?operation=DELETE"
        ))
        .expect("valid delete endpoint config"),
    );
    let mut delete_msg = Message::default();
    delete_msg.set_header("CamelOpenSearch.Id", serde_json::json!("doc-1"));
    call_ready(&mut delete, Exchange::new(delete_msg)).await;
}
