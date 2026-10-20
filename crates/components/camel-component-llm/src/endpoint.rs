use std::sync::Arc;
use std::time::Duration;

use camel_component_api::{
    BoxProcessor, CamelError, Consumer, Endpoint, NetworkRetryPolicy, ProducerContext,
    RuntimeObservability,
};
use tokio::sync::Semaphore;

use crate::LlmGlobalConfig;
use crate::config::LlmEndpointConfig;
use crate::cost::PricingTable;
use crate::producer::LlmProducer;
use crate::producer_cache::ProducerCache;
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

    #[allow(clippy::type_complexity)]
    fn resolve_provider(
        &self,
    ) -> Result<
        (
            Arc<dyn LlmProvider>,
            Option<usize>,
            Option<u64>,
            Option<NetworkRetryPolicy>,
            Option<PricingTable>,
            Option<(u64, Option<usize>)>, // (cache_ttl_secs, cache_max_entries)
        ),
        CamelError,
    > {
        let name = resolve_provider_name(&self.config, &self.global_config)?;
        let provider = self.providers.get(name).cloned().ok_or_else(|| {
            CamelError::InvalidUri(format!("provider '{}' not found in config", name))
        })?;
        let max_concurrency = self
            .global_config
            .providers
            .get(name)
            .and_then(|pc| pc.max_concurrency());
        let provider_timeout = self
            .global_config
            .providers
            .get(name)
            .and_then(|pc| pc.timeout_secs());
        let network_retry = self
            .global_config
            .providers
            .get(name)
            .and_then(|pc| pc.network_retry());
        let pricing = self
            .global_config
            .providers
            .get(name)
            .and_then(|pc| pc.pricing());
        let cache_ttl = self
            .global_config
            .providers
            .get(name)
            .and_then(|pc| pc.cache_ttl_secs());
        let cache_max = self
            .global_config
            .providers
            .get(name)
            .and_then(|pc| pc.cache_max_entries());
        let cache_info = cache_ttl.map(|ttl| (ttl, cache_max));
        Ok((
            provider,
            max_concurrency,
            provider_timeout,
            network_retry,
            pricing,
            cache_info,
        ))
    }
}

impl Endpoint for LlmEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let (provider, max_concurrency, provider_timeout, network_retry, pricing, cache_info) =
            self.resolve_provider()?;

        let route_id = ctx.route_id().unwrap_or("unknown").to_string();

        let semaphore = max_concurrency.map(|n| Arc::new(Semaphore::new(n)));

        // Effective timeout: provider overrides global, None = no timeout.
        let timeout_secs = provider_timeout.or(self.global_config.timeout_secs);
        let timeout = timeout_secs.map(Duration::from_secs);

        let pricing = pricing.map(Arc::new);

        // Build cache from config, if configured.
        let cache = cache_info.map(|(ttl_secs, max_entries)| {
            if max_entries.is_none() {
                tracing::warn!(
                    route_id = %route_id,
                    "cache_max_entries is not set — cache is unbounded (high-cardinality prompts may cause memory growth)"
                );
            }
            Arc::new(ProducerCache::new(Duration::from_secs(ttl_secs), max_entries))
        });

        let producer = LlmProducer::new(
            self.config.clone(),
            provider,
            self.global_config.max_prompt_bytes,
            route_id,
        )
        .with_semaphore(semaphore)
        .with_timeout(timeout)
        .with_retry(network_retry)
        .with_pricing(pricing)
        .with_cache(cache)
        .with_observability(Some(rt))
        .with_max_header_json_bytes(self.global_config.max_header_json_bytes)
        .build();
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

#[cfg(all(test, any(feature = "generic-providers", feature = "all-providers")))]
mod tests {
    use std::collections::HashMap;

    use camel_component_api::{Component, NoOpComponentContext};

    use super::*;
    use crate::LlmComponent;
    use crate::config::{GenericProviderConfig, LlmProviderConfig, LlmServiceKind};
    use crate::provider_factory::build_provider_map;

    fn generic_config(service: LlmServiceKind) -> GenericProviderConfig {
        GenericProviderConfig {
            service,
            api_key: "explicit-key".into(),
            default_model: "explicit-model".into(),
            base_url: None,
            timeout_secs: Some(7),
            max_concurrency: Some(2),
            network_retry: None,
            pricing: Some(PricingTable {
                input_per_1k_tokens: 0.01,
                output_per_1k_tokens: 0.02,
            }),
            cache_ttl_secs: Some(9),
            cache_max_entries: Some(4),
        }
    }

    #[tokio::test]
    async fn generic_endpoint_embed_is_rejected() {
        for service in [LlmServiceKind::Anthropic, LlmServiceKind::Gemini] {
            let server = wiremock::MockServer::start().await;
            let mut providers = HashMap::new();
            providers.insert(
                "native".into(),
                LlmProviderConfig::Generic(GenericProviderConfig {
                    service,
                    api_key: "explicit-key".into(),
                    default_model: "explicit-model".into(),
                    base_url: Some(server.uri()),
                    timeout_secs: None,
                    max_concurrency: None,
                    network_retry: None,
                    pricing: None,
                    cache_ttl_secs: None,
                    cache_max_entries: None,
                }),
            );
            let global = LlmGlobalConfig {
                providers,
                allow_internal: true,
                ..Default::default()
            };
            let component = LlmComponent::new(global).expect("generic provider map builds lazily");

            let err = match component
                .create_endpoint("llm:embed?provider=native", &NoOpComponentContext)
            {
                Ok(_) => {
                    panic!("generic {service:?} embed must be rejected at endpoint construction")
                }
                Err(err) => err,
            };

            let message = err.to_string();
            assert!(
                message.contains("does not support embed"),
                "expected unsupported-embedding diagnostic for {service:?}, got: {message}"
            );
            assert!(
                message.contains("native"),
                "diagnostic must name the provider for {service:?}, got: {message}"
            );

            let requests = server
                .received_requests()
                .await
                .expect("wiremock request journal available");
            assert!(
                requests.is_empty(),
                "embed rejection must not perform I/O for {service:?}, got {} request(s)",
                requests.len()
            );
        }
    }

    #[test]
    fn generic_endpoint_policy_tuple_preserves_settings() {
        let retry = NetworkRetryPolicy {
            enabled: true,
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            multiplier: 2.0,
            max_delay: Duration::from_millis(5),
            jitter_factor: 0.0,
            max_attempts_absolute: None,
        };

        for service in [LlmServiceKind::Anthropic, LlmServiceKind::Gemini] {
            let mut config = generic_config(service);
            config.network_retry = Some(retry.clone());

            let mut providers = HashMap::new();
            providers.insert("native".into(), LlmProviderConfig::Generic(config));
            let global = LlmGlobalConfig {
                timeout_secs: Some(99),
                providers,
                allow_internal: true,
                ..Default::default()
            };

            let map = build_provider_map(&global).expect("generic provider map builds lazily");
            let endpoint_config =
                LlmEndpointConfig::from_uri("llm:chat?provider=native").expect("endpoint uri");
            let endpoint = LlmEndpoint::new(
                "llm:chat?provider=native".into(),
                endpoint_config,
                Arc::new(map),
                Arc::new(global.clone()),
            );

            let (_, concurrency, provider_timeout, network_retry, pricing, cache_info) =
                endpoint.resolve_provider().expect("policy tuple resolves");

            assert_eq!(concurrency, Some(2), "concurrency for {service:?}");
            assert_eq!(provider_timeout, Some(7), "timeout for {service:?}");
            assert_eq!(
                network_retry.as_ref(),
                Some(&retry),
                "retry policy must be preserved exactly for {service:?}"
            );
            let pricing = pricing.expect("pricing present");
            assert_eq!(pricing.input_per_1k_tokens, 0.01);
            assert_eq!(pricing.output_per_1k_tokens, 0.02);
            assert_eq!(
                cache_info,
                Some((9, Some(4))),
                "cache settings for {service:?}"
            );

            // With no provider timeout, resolve reports None and the existing
            // create_producer fallback selects the global 99s timeout.
            let mut global_without_timeout = global.clone();
            let provider_config = global_without_timeout
                .providers
                .get_mut("native")
                .expect("provider config present");
            if let LlmProviderConfig::Generic(config) = provider_config {
                config.timeout_secs = None;
            }

            let map = build_provider_map(&global_without_timeout)
                .expect("generic provider map builds lazily");
            let endpoint_config =
                LlmEndpointConfig::from_uri("llm:chat?provider=native").expect("endpoint uri");
            let endpoint = LlmEndpoint::new(
                "llm:chat?provider=native".into(),
                endpoint_config,
                Arc::new(map),
                Arc::new(global_without_timeout.clone()),
            );

            let (_, _, provider_timeout, _, _, _) =
                endpoint.resolve_provider().expect("policy tuple resolves");
            assert_eq!(
                provider_timeout, None,
                "absent provider timeout for {service:?}"
            );
            assert_eq!(global_without_timeout.timeout_secs, Some(99));
            assert_eq!(
                provider_timeout.or(endpoint.global_config.timeout_secs),
                Some(99),
                "create_producer fallback preserves the global timeout for {service:?}"
            );
        }
    }
}
