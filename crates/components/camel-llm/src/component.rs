use std::collections::HashMap;
use std::sync::Arc;

use camel_ai::{ChatModel, OllamaAdapter, OllamaConfig, OpenAiAdapter, OpenAiConfig};
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

        // Detect provider variant from URI: "llm:ollama?..." vs "llm:openai?..."
        let variant = uri
            .split(':')
            .nth(1)
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("")
            .trim_start_matches('/');
        let is_ollama = variant == "ollama";

        let base_url = params
            .get("base_url")
            .cloned()
            .unwrap_or_else(|| "http://localhost:11434".into());
        let model = params
            .get("model")
            .cloned()
            .unwrap_or_else(|| "qwen3.5:4b".into());
        let api_key = params.get("api_key").cloned();
        let temperature = params
            .get("temperature")
            .and_then(|v| v.parse::<f32>().ok());
        let system_prompt = params.get("system_prompt").cloned();
        let think = params.get("think").and_then(|v| match v.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        });

        let adapter: Arc<dyn ChatModel> = if is_ollama {
            Arc::new(OllamaAdapter::new(OllamaConfig { base_url, model }))
        } else {
            Arc::new(OpenAiAdapter::new(OpenAiConfig {
                api_key: api_key.or_else(|| std::env::var("OPENAI_API_KEY").ok()),
                base_url: Some(base_url),
                model,
            }))
        };

        Ok(Box::new(LlmEndpoint {
            uri: uri.to_string(),
            model: adapter,
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
