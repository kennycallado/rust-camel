use std::sync::Arc;

use async_trait::async_trait;
use camel_api::{CamelError, Exchange};

/// Merges the original Exchange with the enriched/polled Exchange in the EIP-7
/// `enrich` and `pollEnrich` verbs.
///
/// Distinct from the EIP-22 `AggregationStrategy` family (which combines many
/// Exchanges into one); `EnrichmentStrategy` is always 2→1 (original + enriched).
#[async_trait]
pub trait EnrichmentStrategy: Send + Sync {
    fn aggregate(&self, original: Exchange, enriched: Exchange) -> Exchange;

    /// Called when pollEnrich receives no message (`None` from
    /// `PollingConsumer::receive`). The default implementation passes the
    /// original exchange through unchanged, matching Camel 4.x passthrough
    /// semantics. Override to throw an error or perform other actions.
    async fn on_no_poll(&self, original: Exchange) -> Result<Exchange, CamelError> {
        Ok(original)
    }
}

/// Default strategy: discard the original body, replace with the enriched body.
/// Headers and properties from the original are preserved; enriched headers
/// are discarded. Implement a custom `EnrichmentStrategy` if you need to merge
/// headers from the enriched exchange.
pub struct UseEnrichedBody;

#[async_trait]
impl EnrichmentStrategy for UseEnrichedBody {
    fn aggregate(&self, mut original: Exchange, enriched: Exchange) -> Exchange {
        // Replace the body from the enriched exchange, but keep original headers/properties.
        original.input.body = enriched.input.body;
        original
    }
    // on_no_poll uses the default passthrough
}

/// Strategy that throws a [`CamelError::ProcessorError`] when pollEnrich
/// receives no message. Wraps a base [`EnrichmentStrategy`] for the
/// `aggregate` behaviour.
pub struct ThrowOnNoPoll {
    inner: Arc<dyn EnrichmentStrategy>,
}

impl ThrowOnNoPoll {
    pub fn new(inner: Arc<dyn EnrichmentStrategy>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl EnrichmentStrategy for ThrowOnNoPoll {
    fn aggregate(&self, original: Exchange, enriched: Exchange) -> Exchange {
        self.inner.aggregate(original, enriched)
    }

    async fn on_no_poll(&self, _original: Exchange) -> Result<Exchange, CamelError> {
        Err(CamelError::ProcessorError(
            "poll-enrich: no message available".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Body, Message};

    fn exchange(body: &str) -> Exchange {
        Exchange::new(Message::new(body))
    }

    #[test]
    fn use_enriched_body_replaces_body() {
        let strategy = UseEnrichedBody;
        let result = strategy.aggregate(exchange("original"), exchange("enriched"));
        match &result.input.body {
            Body::Text(s) => assert_eq!(s, "enriched"),
            other => panic!("unexpected body {other:?}"),
        }
    }
}
