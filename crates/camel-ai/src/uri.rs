use std::collections::HashMap;
use std::sync::Arc;

use crate::adapters::{OllamaAdapter, OllamaConfig, OpenAiAdapter, OpenAiConfig};
use crate::traits::{ChatModel, EmbeddingModel};
use camel_api::CamelError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProvider {
    Ollama,
    OpenAi,
}

#[derive(Debug, Clone)]
pub struct AiModelUri {
    pub provider: AiProvider,
    pub capability: AiCapability,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub extra: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiCapability {
    Chat,
    Embedding,
}

impl AiModelUri {
    pub fn parse(uri: &str) -> Result<Self, CamelError> {
        let (scheme, query) = uri.split_once('?').unwrap_or((uri, ""));
        let params = parse_query_params(query);

        let (capability_str, provider_str) = scheme
            .split_once(':')
            .ok_or_else(|| CamelError::RouteError(format!("invalid AI URI (no ':'): {uri}")))?;

        let capability = match capability_str {
            "llm" => AiCapability::Chat,
            "embedding" => AiCapability::Embedding,
            other => {
                return Err(CamelError::RouteError(format!(
                    "unknown AI capability '{other}' in URI: {uri}"
                )))
            }
        };

        let provider = match provider_str {
            "ollama" => AiProvider::Ollama,
            "openai" => AiProvider::OpenAi,
            "create" => {
                tracing::warn!(
                    "deprecated provider 'create' in URI — use 'ollama' or 'openai' instead"
                );
                AiProvider::Ollama
            }
            other => {
                return Err(CamelError::RouteError(format!(
                    "unknown AI provider '{other}' in URI: {uri}"
                )))
            }
        };

        let base_url = params
            .get("base_url")
            .cloned()
            .unwrap_or_else(|| match provider {
                AiProvider::Ollama => "http://localhost:11434".into(),
                AiProvider::OpenAi => "https://api.openai.com/v1".into(),
            });

        let model = params
            .get("model")
            .cloned()
            .unwrap_or_else(|| match (provider, capability) {
                (AiProvider::Ollama, AiCapability::Chat) => "qwen3.5:4b".into(),
                (AiProvider::Ollama, AiCapability::Embedding) => "embeddinggemma".into(),
                (AiProvider::OpenAi, AiCapability::Chat) => "gpt-4o-mini".into(),
                (AiProvider::OpenAi, AiCapability::Embedding) => "text-embedding-3-small".into(),
            });

        let api_key = params.get("api_key").cloned().or_else(|| match provider {
            AiProvider::OpenAi => std::env::var("OPENAI_API_KEY").ok(),
            AiProvider::Ollama => None,
        });

        if params.contains_key("api_key") {
            tracing::warn!(
                "api_key in URI is for demos only — prefer OPENAI_API_KEY env var"
            );
        }

        Ok(Self {
            provider,
            capability,
            base_url,
            model,
            api_key,
            extra: params
                .into_iter()
                .filter(|(k, _)| !matches!(k.as_str(), "base_url" | "model" | "api_key"))
                .collect(),
        })
    }
}

pub fn resolve_chat_model(uri: &str) -> Result<Arc<dyn ChatModel>, CamelError> {
    let parsed = AiModelUri::parse(uri)?;
    if parsed.capability != AiCapability::Chat {
        return Err(CamelError::RouteError(format!(
            "expected chat capability, got {:?}",
            parsed.capability
        )));
    }
    match parsed.provider {
        AiProvider::Ollama => Ok(Arc::new(OllamaAdapter::new(OllamaConfig {
            base_url: parsed.base_url,
            model: parsed.model,
        }))),
        AiProvider::OpenAi => Ok(Arc::new(OpenAiAdapter::new(OpenAiConfig {
            api_key: parsed.api_key,
            base_url: Some(parsed.base_url),
            model: parsed.model,
        }))),
    }
}

pub fn resolve_embedding_model(uri: &str) -> Result<Arc<dyn EmbeddingModel>, CamelError> {
    let parsed = AiModelUri::parse(uri)?;
    if parsed.capability != AiCapability::Embedding {
        return Err(CamelError::RouteError(format!(
            "expected embedding capability, got {:?}",
            parsed.capability
        )));
    }
    match parsed.provider {
        AiProvider::Ollama => Ok(Arc::new(OllamaAdapter::new(OllamaConfig {
            base_url: parsed.base_url,
            model: parsed.model,
        }))),
        AiProvider::OpenAi => Ok(Arc::new(OpenAiAdapter::new(OpenAiConfig {
            api_key: parsed.api_key,
            base_url: Some(parsed.base_url),
            model: parsed.model,
        }))),
    }
}

fn parse_query_params(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let k = parts.next()?.to_string();
            let v = parts.next().unwrap_or("").to_string();
            if k.is_empty() {
                None
            } else {
                Some((k, v))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_llm_ollama() {
        let uri = AiModelUri::parse("llm:ollama?base_url=http://localhost:11434&model=qwen3.5:4b").unwrap();
        assert_eq!(uri.provider, AiProvider::Ollama);
        assert_eq!(uri.capability, AiCapability::Chat);
        assert_eq!(uri.base_url, "http://localhost:11434");
        assert_eq!(uri.model, "qwen3.5:4b");
    }

    #[test]
    fn parse_llm_openai() {
        let uri = AiModelUri::parse("llm:openai?model=gpt-4o-mini").unwrap();
        assert_eq!(uri.provider, AiProvider::OpenAi);
        assert_eq!(uri.capability, AiCapability::Chat);
        assert_eq!(uri.base_url, "https://api.openai.com/v1");
        assert_eq!(uri.model, "gpt-4o-mini");
    }

    #[test]
    fn parse_embedding_ollama() {
        let uri = AiModelUri::parse("embedding:ollama?base_url=http://localhost:11434&model=embeddinggemma").unwrap();
        assert_eq!(uri.provider, AiProvider::Ollama);
        assert_eq!(uri.capability, AiCapability::Embedding);
    }

    #[test]
    fn parse_embedding_openai() {
        let uri = AiModelUri::parse("embedding:openai?model=text-embedding-3-small").unwrap();
        assert_eq!(uri.provider, AiProvider::OpenAi);
        assert_eq!(uri.capability, AiCapability::Embedding);
        assert_eq!(uri.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn openai_default_url_not_localhost() {
        let uri = AiModelUri::parse("llm:openai?model=gpt-4o-mini").unwrap();
        assert!(!uri.base_url.contains("localhost"));
        assert!(!uri.base_url.contains("11434"));
    }

    #[test]
    fn ollama_default_url() {
        let uri = AiModelUri::parse("llm:ollama?model=test").unwrap();
        assert_eq!(uri.base_url, "http://localhost:11434");
    }

    #[test]
    fn parse_rejects_unknown_scheme() {
        let err = AiModelUri::parse("foo:bar").unwrap_err();
        assert!(format!("{err}").contains("unknown AI capability"));
    }

    #[test]
    fn parse_rejects_unknown_provider() {
        let err = AiModelUri::parse("llm:groq?model=x").unwrap_err();
        assert!(format!("{err}").contains("unknown AI provider"));
    }

    #[test]
    fn parse_no_colon_error() {
        let err = AiModelUri::parse("nocolon").unwrap_err();
        assert!(format!("{err}").contains("no ':'"));
    }

    #[test]
    fn api_key_from_uri_warns() {
        let uri = AiModelUri::parse("llm:openai?model=gpt-4o-mini&api_key=sk-test").unwrap();
        assert_eq!(uri.api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn extra_params_preserved() {
        let uri = AiModelUri::parse("llm:ollama?model=test&think=false&temperature=0.5").unwrap();
        assert_eq!(uri.extra.get("think").unwrap(), "false");
        assert_eq!(uri.extra.get("temperature").unwrap(), "0.5");
    }

    #[test]
    fn parse_create_alias_backward_compat() {
        let uri = AiModelUri::parse("embedding:create?base_url=http://localhost:11434&model=embeddinggemma").unwrap();
        assert_eq!(uri.provider, AiProvider::Ollama);
        assert_eq!(uri.capability, AiCapability::Embedding);
    }

    #[test]
    fn parse_llm_create_alias_backward_compat() {
        let uri = AiModelUri::parse("llm:create?base_url=http://localhost:11434&model=qwen3.5:4b").unwrap();
        assert_eq!(uri.provider, AiProvider::Ollama);
        assert_eq!(uri.capability, AiCapability::Chat);
    }

    #[test]
    fn parse_empty_query_params() {
        let uri = AiModelUri::parse("llm:ollama").unwrap();
        assert_eq!(uri.model, "qwen3.5:4b");
    }

    #[test]
    fn parse_rejects_empty_provider() {
        let err = AiModelUri::parse("llm:?model=x").unwrap_err();
        assert!(format!("{err}").contains("unknown AI provider"));
    }
}
