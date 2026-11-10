use std::sync::Arc;

use camel_component_api::{
    BoxProcessor, CamelError, Consumer, Endpoint, ProducerContext, RuntimeObservability,
};
use tokio::sync::Semaphore;

use crate::LlmGlobalConfig;
use crate::config::LlmEndpointConfig;
use crate::decision::DecisionProvider;
use crate::decision::config;
use crate::decision::config::QuestionSetConfig;
use crate::decision::producer::DecisionProducer;

/// Endpoint for `llm:decide`. The provider and question set are resolved and
/// validated when the endpoint is created, before any network I/O.
pub struct DecisionEndpoint {
    uri: String,
    config: LlmEndpointConfig,
    provider_name: String,
    provider: Arc<dyn DecisionProvider>,
    set: Arc<QuestionSetConfig>,
    global: Arc<LlmGlobalConfig>,
}

impl DecisionEndpoint {
    /// Creates an endpoint from an already-resolved provider and question set.
    pub(crate) fn new(
        uri: String,
        config: LlmEndpointConfig,
        provider_name: String,
        provider: Arc<dyn DecisionProvider>,
        set: Arc<QuestionSetConfig>,
        global: Arc<LlmGlobalConfig>,
    ) -> Self {
        Self {
            uri,
            config,
            provider_name,
            provider,
            set,
            global,
        }
    }
}

impl Endpoint for DecisionEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let provider_cfg = self.global.decision_providers.get(&self.provider_name);
        let max_concurrency = provider_cfg.and_then(|pc| pc.max_concurrency());
        let provider_timeout = provider_cfg.and_then(|pc| pc.timeout_secs());
        let network_retry = provider_cfg.and_then(|pc| pc.network_retry());

        // Total deadline for every decide call: provider or global
        // `timeout_secs`, otherwise the default. Never `None`.
        let timeout = Some(config::decide_total_timeout_secs(
            provider_timeout,
            self.global.timeout_secs,
        ));

        let route_id = ctx.route_id().unwrap_or("unknown").to_string();

        let producer = DecisionProducer::new(
            Arc::clone(&self.provider),
            self.config.model.clone(),
            Arc::clone(&self.set),
            self.global.max_prompt_bytes,
            route_id,
        )
        .with_semaphore(max_concurrency.map(|n| Arc::new(Semaphore::new(n))))
        .with_timeout(timeout)
        .with_retry(network_retry);
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

#[cfg(test)]
mod decide_tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use camel_api::{Body, Exchange, Message};
    use camel_component_api::{
        Component, Endpoint, NoOpComponentContext, NoopRuntimeObservability, ProducerContext,
        RuntimeObservability,
    };
    use tower::Service;

    use super::DecisionEndpoint;
    use crate::LlmComponent;
    use crate::LlmEndpointConfig;
    use crate::LlmGlobalConfig;
    use crate::decision::test_support::TRIAGE_TOML;
    use crate::decision::test_support::triage_set;
    use crate::decision::{
        DecisionCapabilities, DecisionProvider, DecisionRequest, DecisionResponse,
    };
    use crate::error::LlmError;

    const DEFAULT_FAKE: &str = "default_decision_provider = \"fake\"\n";

    /// Provider whose `decide` future never resolves, so only the producer's
    /// total deadline can end the call.
    struct PendingDecisionProvider;

    #[async_trait]
    impl DecisionProvider for PendingDecisionProvider {
        fn id(&self) -> &str {
            "pending"
        }

        fn default_model(&self) -> &str {
            "pending-model"
        }

        fn capabilities(&self) -> DecisionCapabilities {
            DecisionCapabilities::new(400)
        }

        async fn decide(&self, _req: DecisionRequest) -> Result<DecisionResponse, LlmError> {
            std::future::pending::<Result<DecisionResponse, LlmError>>().await
        }
    }

    /// Builds a component with one mock decision provider named `fake` and the
    /// triage question set. `prefix` is prepended before any table header.
    fn component_from(prefix: &str) -> LlmComponent {
        let toml_str = format!("{prefix}[decision_providers.fake]\ntype = \"mock\"\n{TRIAGE_TOML}");
        let config: LlmGlobalConfig = toml::from_str(&toml_str).expect("config parses");
        LlmComponent::new(config).expect("component builds")
    }

    /// Returns the error text of a failing `create_endpoint` call.
    fn create_err(component: &LlmComponent, uri: &str) -> String {
        match component.create_endpoint(uri, &NoOpComponentContext) {
            Ok(_) => panic!("expected create_endpoint({uri}) to fail"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn decide_endpoint_unknown_set() {
        let component = component_from("");
        let text = create_err(&component, "llm:decide?provider=fake&questions=nope");
        assert!(text.contains("nope"), "error was: {text}");
    }

    #[test]
    fn decide_endpoint_unknown_set_via_default_provider() {
        let component = component_from(DEFAULT_FAKE);
        let text = create_err(&component, "llm:decide?questions=nope");
        assert!(text.contains("nope"), "error was: {text}");
    }

    #[test]
    fn decide_endpoint_unknown_provider() {
        let component = component_from("");
        let text = create_err(&component, "llm:decide?provider=ghost&questions=triage");
        assert!(text.contains("ghost"), "error was: {text}");
    }

    #[test]
    fn decide_endpoint_no_provider() {
        let component = component_from("");
        let text = create_err(&component, "llm:decide?questions=triage");
        assert!(
            text.contains("default_decision_provider"),
            "error was: {text}"
        );
    }

    #[test]
    fn decide_with_zero_chat_providers_succeeds() {
        let component = component_from(DEFAULT_FAKE);
        component
            .create_endpoint("llm:decide?questions=triage", &NoOpComponentContext)
            .expect("decide endpoint without chat providers");
    }

    #[test]
    fn decide_endpoint_creates_producer() {
        let component = component_from("");
        let endpoint = component
            .create_endpoint(
                "llm:decide?provider=fake&questions=triage",
                &NoOpComponentContext,
            )
            .expect("decide endpoint");
        let ctx = ProducerContext::new().with_route_id("r");
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        endpoint.create_producer(rt, &ctx).expect("decide producer");
    }

    #[test]
    fn from_parts_has_empty_decision_map() {
        let component = LlmComponent::from_parts(LlmGlobalConfig::default(), HashMap::new());
        assert!(component.decision_providers().is_empty());
    }

    #[test]
    fn from_parts_with_invalid_set_is_rejected_at_endpoint() {
        let toml_str = format!(
            "[decision_providers.fake]\ntype = \"mock\"\n{TRIAGE_TOML}[[decision_question_sets.triage.questions]]\nname = \"dept\"\ntype = \"predicate\"\ninstructions = \"Duplicate?\"\n"
        );
        let config: LlmGlobalConfig = toml::from_str(&toml_str).expect("config parses");
        let decisions = crate::decision::factory::build_decision_provider_map(&config)
            .expect("decision map builds");
        let component =
            LlmComponent::from_parts(config, HashMap::new()).with_decision_providers(decisions);
        let text = create_err(&component, "llm:decide?provider=fake&questions=triage");
        assert!(text.contains("duplicate"), "error was: {text}");
    }

    #[test]
    fn from_parts_without_registration_names_the_fix() {
        let toml_str = format!("[decision_providers.fake]\ntype = \"mock\"\n{TRIAGE_TOML}");
        let config: LlmGlobalConfig = toml::from_str(&toml_str).expect("config parses");
        let component = LlmComponent::from_parts(config, HashMap::new());
        let text = create_err(&component, "llm:decide?provider=fake&questions=triage");
        assert!(text.contains("not registered"), "error was: {text}");
        assert!(
            text.contains("with_decision_providers"),
            "error was: {text}"
        );
    }

    /// The endpoint injects the default total deadline when no `timeout_secs`
    /// is configured, so a hanging provider cannot park the exchange forever.
    ///
    /// The 121 s virtual guard only fires when the wiring passes `None` through.
    #[tokio::test(start_paused = true)]
    async fn decide_endpoint_applies_default_total_when_unconfigured() {
        let endpoint = DecisionEndpoint::new(
            "llm:decide?questions=triage".into(),
            LlmEndpointConfig::default(),
            "pending".into(),
            Arc::new(PendingDecisionProvider),
            Arc::new(triage_set()),
            Arc::new(LlmGlobalConfig::default()),
        );
        let ctx = ProducerContext::new().with_route_id("r");
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let mut producer = endpoint.create_producer(rt, &ctx).expect("decide producer");

        let guarded = tokio::time::timeout(
            Duration::from_secs(121),
            Service::call(
                &mut producer,
                Exchange::new(Message::new(Body::Text("refund please".into()))),
            ),
        )
        .await;

        let result = match guarded {
            Ok(result) => result,
            Err(_) => panic!(
                "decide call outlived the 121s guard: the endpoint did not apply a total deadline"
            ),
        };
        let err = result.expect_err("a pending provider must hit the total deadline");
        let msg = err.to_string();
        assert!(msg.contains("timeout"), "error was {msg}");
        assert!(msg.contains("120s"), "error was {msg}");
    }
}
