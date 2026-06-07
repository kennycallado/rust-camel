use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_component_api::Consumer;
use camel_component_api::ProducerContext;
use camel_component_api::{
    CamelError, Endpoint, Exchange, Message, PollingConsumer, RuntimeObservability,
};

/// An endpoint that exposes a polling consumer returning one exchange.
struct FileReadyEndpoint {
    uri: String,
}

impl Endpoint for FileReadyEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed("not implemented".into()))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<camel_component_api::BoxProcessor, CamelError> {
        Err(CamelError::ProcessorError("not implemented".into()))
    }

    fn polling_consumer(&self) -> Option<Box<dyn PollingConsumer>> {
        Some(Box::new(AlwaysOneExchange))
    }
}

struct AlwaysOneExchange;

#[async_trait]
impl PollingConsumer for AlwaysOneExchange {
    async fn receive(&mut self, _timeout: Duration) -> Result<Option<Exchange>, CamelError> {
        Ok(Some(Exchange::new(Message::new("hello"))))
    }
}

#[tokio::test]
async fn polling_consumer_returns_exchange_via_receive_with_timeout() {
    let ep = FileReadyEndpoint {
        uri: "test://x".to_string(),
    };
    let mut poller = ep
        .polling_consumer()
        .expect("endpoint should expose a polling consumer");
    let exchange = poller.receive(Duration::from_millis(100)).await.unwrap();
    assert!(exchange.is_some());
    assert_eq!(exchange.unwrap().input.body.as_text(), Some("hello"));
}

#[tokio::test]
async fn polling_consumer_receive_returns_none_for_default_endpoint() {
    struct BareEndpoint {
        uri: String,
    }

    impl Endpoint for BareEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }

        fn create_consumer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
        ) -> Result<Box<dyn Consumer>, CamelError> {
            unimplemented!()
        }

        fn create_producer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<camel_component_api::BoxProcessor, CamelError> {
            unimplemented!()
        }
    }

    let ep = BareEndpoint {
        uri: "bare://x".to_string(),
    };
    assert!(ep.polling_consumer().is_none());
}
