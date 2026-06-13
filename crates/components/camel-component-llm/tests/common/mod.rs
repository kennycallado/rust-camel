use std::collections::HashMap;

use camel_component_llm::{
    LlmComponent, LlmGlobalConfig, LlmProviderConfig, config::MockProviderConfig,
};

#[allow(dead_code)]
pub fn make_test_component_with_response(response: &str) -> LlmComponent {
    let mut providers = HashMap::new();
    providers.insert(
        "test".into(),
        LlmProviderConfig::Mock(MockProviderConfig {
            response: response.into(),
            default_model: "mock-model".into(),
            error_message: None,
        }),
    );
    LlmComponent::new(LlmGlobalConfig {
        providers,
        ..Default::default()
    })
    .expect("valid component")
}

#[allow(dead_code)]
pub fn make_test_component() -> LlmComponent {
    make_test_component_with_response("fixed:Hello from LLM")
}

#[allow(dead_code)]
pub fn make_test_component_with_error(error_msg: &str) -> LlmComponent {
    let mut providers = HashMap::new();
    providers.insert(
        "err".into(),
        LlmProviderConfig::Mock(MockProviderConfig {
            response: "echo".into(),
            default_model: "mock-model".into(),
            error_message: Some(error_msg.into()),
        }),
    );
    LlmComponent::new(LlmGlobalConfig {
        providers,
        ..Default::default()
    })
    .expect("component")
}

pub fn make_producer_ctx() -> camel_component_api::ProducerContext {
    camel_component_api::ProducerContext::new().with_route_id("route-1")
}
