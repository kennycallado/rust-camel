use camel_api::Exchange;

/// Merges the original Exchange with the enriched/polled Exchange in the EIP-7
/// `enrich` and `pollEnrich` verbs.
///
/// Distinct from the EIP-22 `AggregationStrategy` family (which combines many
/// Exchanges into one); `EnrichmentStrategy` is always 2→1 (original + enriched).
pub trait EnrichmentStrategy: Send + Sync {
    fn aggregate(&self, original: Exchange, enriched: Exchange) -> Exchange;
}

/// Default strategy: discard the original body, replace with the enriched body.
/// Headers and properties from the original are preserved; enriched headers
/// are discarded. Implement a custom `EnrichmentStrategy` if you need to merge
/// headers from the enriched exchange.
pub struct UseEnrichedBody;

impl EnrichmentStrategy for UseEnrichedBody {
    fn aggregate(&self, mut original: Exchange, enriched: Exchange) -> Exchange {
        // Replace the body from the enriched exchange, but keep original headers/properties.
        original.input.body = enriched.input.body;
        original
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
