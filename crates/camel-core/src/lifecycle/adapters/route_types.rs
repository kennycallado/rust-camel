// lifecycle/adapters/route_types.rs
// Route is the compiled artifact produced by route_compiler from a RouteDefinition.
// It lives in adapters/ because it contains BoxProcessor (Tower) — a compiled pipeline.

use camel_api::BoxProcessor;
use camel_component::ConcurrencyModel;

/// A Route defines a message flow: from a source endpoint, through a composed
/// Tower Service pipeline.
pub struct Route {
    /// The source endpoint URI.
    pub(crate) from_uri: String,
    /// The composed processor pipeline as a type-erased Tower Service.
    pub(crate) pipeline: BoxProcessor,
    /// Optional per-route concurrency model override.
    /// When `None`, the consumer's default concurrency model is used.
    pub(crate) concurrency: Option<ConcurrencyModel>,
}

impl Route {
    /// Create a new route from the given source URI and processor pipeline.
    pub fn new(from_uri: impl Into<String>, pipeline: BoxProcessor) -> Self {
        Self {
            from_uri: from_uri.into(),
            pipeline,
            concurrency: None,
        }
    }

    /// The source endpoint URI.
    pub fn from_uri(&self) -> &str {
        &self.from_uri
    }

    /// Consume the route and return its pipeline.
    pub fn into_pipeline(self) -> BoxProcessor {
        self.pipeline
    }

    /// Set a concurrency model override for this route.
    pub fn with_concurrency(mut self, model: ConcurrencyModel) -> Self {
        self.concurrency = Some(model);
        self
    }

    /// Get the concurrency model override, if any.
    pub fn concurrency_override(&self) -> Option<&ConcurrencyModel> {
        self.concurrency.as_ref()
    }

    /// Consume the route, returning the pipeline and optional concurrency override.
    pub fn into_parts(self) -> (BoxProcessor, Option<ConcurrencyModel>) {
        (self.pipeline, self.concurrency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::IdentityProcessor;

    #[test]
    fn route_accessors_and_parts_work() {
        let route = Route::new("direct:a", BoxProcessor::new(IdentityProcessor));
        assert_eq!(route.from_uri(), "direct:a");
        assert!(route.concurrency_override().is_none());
    }

    #[test]
    fn route_with_concurrency_sets_override() {
        let route = Route::new("direct:b", BoxProcessor::new(IdentityProcessor))
            .with_concurrency(ConcurrencyModel::Concurrent { max: Some(3) });
        assert!(matches!(
            route.concurrency_override(),
            Some(ConcurrencyModel::Concurrent { max: Some(3) })
        ));

        let (_pipeline, concurrency) = route.into_parts();
        assert!(matches!(
            concurrency,
            Some(ConcurrencyModel::Concurrent { max: Some(3) })
        ));
    }

    #[test]
    fn into_pipeline_consumes_route() {
        let route = Route::new("direct:c", BoxProcessor::new(IdentityProcessor));
        let _pipeline = route.into_pipeline();
    }
}
