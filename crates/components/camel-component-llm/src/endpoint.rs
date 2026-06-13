use std::sync::Arc;

use camel_component_api::{
    BoxProcessor, CamelError, Consumer, Endpoint, ProducerContext, RuntimeObservability,
};

use crate::LlmGlobalConfig;
use crate::config::LlmEndpointConfig;
use crate::producer::LlmProducer;
use crate::provider::LlmProvider;
use crate::provider_factory::ProviderMap;

/// Resolve the provider name from an endpoint config, falling back to the
/// global default. Returns the provider name or an error if neither is set.
///
/// Shared between `create_endpoint` (fail-fast validation in lib.rs) and
/// `resolve_provider` (actual provider lookup at producer creation time).
pub(crate) fn resolve_provider_name<'a>(
    config: &'a LlmEndpointConfig,
    global: &'a LlmGlobalConfig,
) -> Result<&'a str, CamelError> {
    config
        .provider
        .as_deref()
        .or(global.default_provider.as_deref())
        .ok_or_else(|| {
            CamelError::InvalidUri(
                "no provider specified and no default_provider configured".into(),
            )
        })
}

pub struct LlmEndpoint {
    uri: String,
    pub config: LlmEndpointConfig,
    providers: Arc<ProviderMap>,
    global_config: Arc<LlmGlobalConfig>,
}

impl LlmEndpoint {
    pub fn new(
        uri: String,
        config: LlmEndpointConfig,
        providers: Arc<ProviderMap>,
        global_config: Arc<LlmGlobalConfig>,
    ) -> Self {
        Self {
            uri,
            config,
            providers,
            global_config,
        }
    }

    fn resolve_provider(&self) -> Result<Arc<dyn LlmProvider>, CamelError> {
        let name = resolve_provider_name(&self.config, &self.global_config)?;
        self.providers.get(name).cloned().ok_or_else(|| {
            CamelError::InvalidUri(format!("provider '{}' not found in config", name))
        })
    }
}

impl Endpoint for LlmEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let provider = self.resolve_provider()?;

        let route_id = ctx.route_id().unwrap_or("unknown").to_string();
        let producer = LlmProducer::new(
            self.config.clone(),
            provider,
            self.global_config.max_prompt_bytes,
            route_id,
        );
        Ok(BoxProcessor::new(producer))
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::InvalidUri(
            "llm component does not support consumers in MVP".into(),
        ))
    }
}
