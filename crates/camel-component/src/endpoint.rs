use camel_api::{BodyType, BoxProcessor, CamelError};

use crate::ProducerContext;
use crate::consumer::Consumer;

/// An Endpoint represents a source or destination in a route URI.
pub trait Endpoint: Send + Sync {
    /// The URI that identifies this endpoint.
    fn uri(&self) -> &str;

    /// Create a consumer that reads from this endpoint.
    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError>;

    /// Create a producer that writes to this endpoint.
    fn create_producer(&self, ctx: &ProducerContext) -> Result<BoxProcessor, CamelError>;

    /// Optional body type contract for the producer.
    ///
    /// When `Some(t)`, the pipeline will coerce the body to `t` before calling
    /// the producer. Default: `None` (accept any body variant, zero overhead).
    fn body_contract(&self) -> Option<BodyType> {
        None
    }
}
