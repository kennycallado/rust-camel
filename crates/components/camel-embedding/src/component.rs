use std::collections::HashMap;
use std::sync::Arc;

use camel_ai::{OpenAiCompatible, OpenAiCompatibleConfig};
use camel_component_api::{CamelError, Component, ComponentContext, Endpoint};

use crate::endpoint::EmbeddingEndpoint;

pub fn parse_query(query: &str) -> HashMap<String, String> {
    query.split('&').filter_map(|pair| {
        let mut parts = pair.splitn(2, '=');
        let k = parts.next()?.to_string();
        let v = parts.next().unwrap_or("").to_string();
        if k.is_empty() { None } else { Some((k, v)) }
    }).collect()
}

#[derive(Default)]
pub struct EmbeddingComponent;

impl Component for EmbeddingComponent {
    fn scheme(&self) -> &str {
        "embedding"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let (_, query) = uri.split_once('?').unwrap_or((uri, ""));
        let params = parse_query(query);

        let adapter = Arc::new(OpenAiCompatible::new(OpenAiCompatibleConfig {
            base_url: params
                .get("base_url")
                .cloned()
                .unwrap_or_else(|| "http://localhost:11434".into()),
            model: params
                .get("model")
                .cloned()
                .unwrap_or_else(|| "embeddinggemma".into()),
            api_key: params.get("api_key").cloned(),
            use_ollama_api: false,
        }));

        Ok(Box::new(EmbeddingEndpoint {
            uri: uri.to_string(),
            model: adapter,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_component_scheme() {
        assert_eq!(EmbeddingComponent::default().scheme(), "embedding");
    }

    #[test]
    fn parse_query_works() {
        let p = parse_query("model=embeddinggemma&base_url=http://localhost");
        assert_eq!(p["model"], "embeddinggemma");
        assert_eq!(p["base_url"], "http://localhost");
    }

    #[test]
    fn parse_query_empty_value() {
        let p = parse_query("key=");
        assert_eq!(p["key"], "");
    }

    #[test]
    fn parse_query_no_value() {
        let p = parse_query("key");
        assert_eq!(p["key"], "", "key without '=' gets empty value");
    }

    #[test]
    fn parse_query_empty_key_skipped() {
        let p = parse_query("=value");
        assert!(p.is_empty(), "empty key should be skipped");
    }
}
