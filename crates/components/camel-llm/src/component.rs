use std::collections::HashMap;

use camel_ai::uri::{AiModelUri, resolve_chat_model};
use camel_component_api::{CamelError, Component, ComponentContext, Endpoint};

use crate::endpoint::LlmEndpoint;

#[derive(Default)]
pub struct LlmComponent;

impl Component for LlmComponent {
    fn scheme(&self) -> &str {
        "llm"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let (_, query) = uri.split_once('?').unwrap_or((uri, ""));
        let params = parse_query(query);

        let parsed = AiModelUri::parse(uri)?;
        let model = resolve_chat_model(uri)?;
        let temperature = params
            .get("temperature")
            .and_then(|v| v.parse::<f32>().ok());
        let system_prompt = params.get("system_prompt").cloned();
        let think = params.get("think").and_then(|v| match v.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        });

        Ok(Box::new(LlmEndpoint {
            uri: uri.to_string(),
            model,
            provider_name: format!("{:?}", parsed.provider).to_lowercase(),
            model_name: parsed.model,
            temperature,
            system_prompt,
            think,
        }))
    }
}

pub fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let k = parts.next()?.to_string();
            let v = parts.next().unwrap_or("").to_string();
            if k.is_empty() { None } else { Some((k, v)) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_component_scheme() {
        let c = LlmComponent;
        assert_eq!(c.scheme(), "llm");
    }

    #[test]
    fn parse_query_extracts_params() {
        let params = parse_query("base_url=http://localhost:11434&model=qwen3.5:4b");
        assert_eq!(params["base_url"], "http://localhost:11434");
        assert_eq!(params["model"], "qwen3.5:4b");
    }
}
