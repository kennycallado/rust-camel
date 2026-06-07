use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{Body, BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message};
use camel_component_api::endpoint::PollingConsumer;
use camel_processor::{EnrichService, PollEnrichService, UseEnrichedBody};
use tower::{Service, ServiceExt};

/// A mock PollingConsumer that returns a fixed exchange with "from-poller" body.
struct StaticPoller;

#[async_trait]
impl PollingConsumer for StaticPoller {
    async fn receive(&mut self, _timeout: Duration) -> Result<Option<Exchange>, CamelError> {
        Ok(Some(Exchange::new(Message::new(Body::Text(
            "from-poller".to_string(),
        )))))
    }
}

#[tokio::test]
async fn poll_enrich_service_replaces_body_with_polled_content() {
    let mut svc = PollEnrichService::new(
        Box::new(StaticPoller),
        Duration::from_millis(100),
        Arc::new(UseEnrichedBody),
    );
    svc.ready().await.unwrap();
    let result = svc
        .call(Exchange::new(Message::new(Body::Text(
            "original".to_string(),
        ))))
        .await
        .unwrap();
    assert_eq!(
        match &result.input.body {
            Body::Text(s) => s.clone(),
            _ => panic!("unexpected body variant"),
        },
        "from-poller"
    );
}

/// A mock PollingConsumer that always returns None (no message available).
struct EmptyPoller;

#[async_trait]
impl PollingConsumer for EmptyPoller {
    async fn receive(&mut self, _timeout: Duration) -> Result<Option<Exchange>, CamelError> {
        Ok(None)
    }
}

#[tokio::test]
async fn poll_enrich_service_passes_through_when_no_message() {
    let mut svc = PollEnrichService::new(
        Box::new(EmptyPoller),
        Duration::from_millis(10),
        Arc::new(UseEnrichedBody),
    );
    svc.ready().await.unwrap();
    let result = svc
        .call(Exchange::new(Message::new(Body::Text(
            "original".to_string(),
        ))))
        .await
        .unwrap();
    // When no message is polled, the original exchange passes through unchanged.
    assert_eq!(
        match &result.input.body {
            Body::Text(s) => s.clone(),
            _ => panic!("unexpected body variant"),
        },
        "original"
    );
}

#[tokio::test]
async fn enrich_service_replaces_body_with_producer_output() {
    // A mock producer that always returns an exchange with body "from-producer",
    // regardless of the input exchange.
    let producer = BoxProcessor::from_fn(|_ex| {
        Box::pin(async {
            Ok::<Exchange, CamelError>(Exchange::new(Message::new(Body::Text(
                "from-producer".to_string(),
            ))))
        })
    });

    let mut svc = EnrichService::new(producer, Arc::new(UseEnrichedBody));
    svc.ready().await.unwrap();
    let result = svc
        .call(Exchange::new(Message::new(Body::Text(
            "original".to_string(),
        ))))
        .await
        .unwrap();

    // The strategy replaces the original body with the producer's output.
    assert_eq!(
        match &result.input.body {
            Body::Text(s) => s.clone(),
            _ => panic!("unexpected body variant"),
        },
        "from-producer"
    );
}
