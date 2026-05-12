use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use camel_ai::{ChatMessage, ChatModel, ChatRequest, ChatRole, set_ai_headers};
use camel_api::{CamelError, Exchange};
use tower::Service;

#[derive(Clone)]
pub struct AiClassifyService {
    pub model: Arc<dyn ChatModel>,
    pub provider_name: String,
    pub model_name: String,
    pub labels: Vec<String>,
    pub output_header: String,
    pub on_unknown: Option<String>,
    pub fallback_label: Option<String>,
}

impl Service<Exchange> for AiClassifyService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let model = Arc::clone(&self.model);
        let provider_name = self.provider_name.clone();
        let model_name = self.model_name.clone();
        let labels = self.labels.clone();
        let output_header = self.output_header.clone();
        let on_unknown = self.on_unknown.clone();
        let fallback_label = self.fallback_label.clone();

        Box::pin(async move {
            let body_str = exchange
                .input
                .body
                .as_text()
                .ok_or_else(|| CamelError::TypeConversionFailed("body is not text".into()))?
                .to_string();

            let labels_str = labels.join(", ");
            let prompt = format!(
                "Classify following text into exactly one of these categories: {labels_str}.\n\
                 Respond with ONLY category name, nothing else.\n\nText: {body_str}"
            );

            let start = std::time::Instant::now();
            let req = ChatRequest {
                messages: vec![ChatMessage {
                    role: ChatRole::User,
                    content: prompt,
                }],
                temperature: Some(0.0),
                max_tokens: Some(32),
                think: Some(false),
            };

            let resp = model.complete(req).await?;
            let latency_ms = start.elapsed().as_millis() as u64;
            let category = normalize_classify_response(&resp.content);

            let matched = labels
                .iter()
                .find(|l| l.to_lowercase() == category)
                .cloned()
                .unwrap_or_else(|| {
                    tracing::warn!(category = %category, "ai_classify: LLM returned unknown category");
                    category
                });

            let final_label = if !labels
                .iter()
                .any(|l| l.to_lowercase() == matched.to_lowercase())
            {
                match on_unknown.as_deref().unwrap_or("raw") {
                    "error" => {
                        return Err(CamelError::RouteError(format!(
                            "ai_classify: unknown category '{matched}', labels: {:?}",
                            labels
                        )));
                    }
                    "fallback" => fallback_label.clone().unwrap_or_else(|| matched.clone()),
                    other => {
                        tracing::warn!(on_unknown = %other, "ai_classify: unknown on_unknown policy, treating as 'raw'");
                        matched.clone()
                    }
                }
            } else {
                matched
            };

            set_ai_headers(
                &mut exchange.input.headers,
                &provider_name,
                &model_name,
                "ai_classify",
                latency_ms,
                resp.usage.as_ref(),
            );

            exchange
                .input
                .headers
                .insert(output_header, serde_json::Value::String(final_label));
            Ok(exchange)
        })
    }
}

fn normalize_classify_response(raw: &str) -> String {
    let s = raw.trim().to_lowercase();
    let s = s.trim_end_matches('.');
    let s = s.trim_end_matches('!');
    let s = s.trim_end_matches('?');
    let s = s.trim_start_matches('"');
    let s = s.trim_end_matches('"');
    s.trim().to_string()
}

#[derive(Clone)]
pub struct AiExtractService {
    pub model: Arc<dyn ChatModel>,
    pub provider_name: String,
    pub model_name: String,
    pub schema: String,
    pub output_header: String,
    pub prompt: Option<String>,
}

impl Service<Exchange> for AiExtractService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let model = Arc::clone(&self.model);
        let provider_name = self.provider_name.clone();
        let model_name = self.model_name.clone();
        let schema = self.schema.clone();
        let output_header = self.output_header.clone();
        let custom_prompt = self.prompt.clone();

        Box::pin(async move {
            let body_str = exchange
                .input
                .body
                .as_text()
                .ok_or_else(|| CamelError::TypeConversionFailed("body is not text".into()))?
                .to_string();

            let prompt = match custom_prompt {
                Some(p) => format!("{p}\n\n{body_str}"),
                None => format!(
                    "Extract structured data from following text according to this JSON schema:\n\
                     {schema}\n\
                     Respond with ONLY valid JSON matching schema, nothing else.\n\nText: {body_str}"
                ),
            };

            let start = Instant::now();
            let req = ChatRequest {
                messages: vec![ChatMessage {
                    role: ChatRole::User,
                    content: prompt,
                }],
                temperature: Some(0.0),
                max_tokens: Some(512),
                think: Some(false),
            };

            let resp = model.complete(req).await?;
            let latency_ms = start.elapsed().as_millis() as u64;
            let raw = resp.content.trim();

            let json_str = strip_markdown_fences(raw);

            tracing::debug!(response = %json_str, "ai_extract LLM response");

            let json_val: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
                tracing::warn!(raw = %raw, "ai_extract: failed to parse LLM response as JSON");
                CamelError::RouteError(format!("AI extract: invalid JSON response: {e}"))
            })?;

            validate_json_schema(&schema, &json_val)?;

            set_ai_headers(
                &mut exchange.input.headers,
                &provider_name,
                &model_name,
                "ai_extract",
                latency_ms,
                resp.usage.as_ref(),
            );

            exchange.input.headers.insert(output_header, json_val);
            Ok(exchange)
        })
    }
}

fn strip_markdown_fences(raw: &str) -> &str {
    if let Some(inner) = raw
        .strip_prefix("```json")
        .or_else(|| raw.strip_prefix("```"))
    {
        inner.trim_start().trim_end_matches("```").trim()
    } else {
        raw
    }
}

fn validate_json_schema(schema_str: &str, value: &serde_json::Value) -> Result<(), CamelError> {
    let schema: serde_json::Value = serde_json::from_str(schema_str)
        .map_err(|e| CamelError::RouteError(format!("AI extract: invalid schema JSON: {e}")))?;

    let validator = jsonschema::validator_for(&schema)
        .map_err(|e| CamelError::RouteError(format!("AI extract: invalid JSON Schema: {e}")))?;

    let msgs: Vec<String> = validator
        .iter_errors(value)
        .map(|e| format!("{e} at {}", e.instance_path()))
        .collect();

    if !msgs.is_empty() {
        return Err(CamelError::RouteError(format!(
            "AI extract: response does not match schema: {}",
            msgs.join("; ")
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use camel_ai::{ChatRequest, ChatResponse};

    struct MockChatModel {
        response: String,
        usage: Option<camel_ai::TokenUsage>,
    }

    #[async_trait]
    impl ChatModel for MockChatModel {
        async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, CamelError> {
            assert_eq!(
                req.think,
                Some(false),
                "think must be false for AI processors"
            );
            Ok(ChatResponse {
                content: self.response.clone(),
                usage: self.usage.clone(),
            })
        }
    }

    fn mock_exchange(body: &str) -> Exchange {
        Exchange::new(camel_api::Message::new(body))
    }

    #[test]
    fn strip_fences_json() {
        assert_eq!(
            strip_markdown_fences("```json\n{\"a\":1}\n```"),
            "{\"a\":1}"
        );
    }

    #[test]
    fn strip_fences_plain() {
        assert_eq!(strip_markdown_fences("```\n{\"a\":1}\n```"), "{\"a\":1}");
    }

    #[test]
    fn strip_fences_none() {
        assert_eq!(strip_markdown_fences("{\"a\":1}"), "{\"a\":1}");
    }

    #[tokio::test]
    async fn ai_extract_valid_json() {
        let model = Arc::new(MockChatModel {
            response: r#"{"name":"John","age":30}"#.into(),
            usage: None,
        });
        let mut svc = AiExtractService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            schema: r#"{"type":"object","properties":{"name":{"type":"string"},"age":{"type":"number"}}}"#.into(),
            output_header: "result".into(),
            prompt: None,
        };
        let ex = svc.call(mock_exchange("some text")).await.unwrap();
        assert!(ex.input.headers.contains_key("result"));
    }

    #[tokio::test]
    async fn ai_extract_valid_json_with_fences() {
        let model = Arc::new(MockChatModel {
            response: "```json\n{\"name\":\"John\"}\n```".into(),
            usage: None,
        });
        let mut svc = AiExtractService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            schema: r#"{"type":"object","properties":{"name":{"type":"string"}}}"#.into(),
            output_header: "result".into(),
            prompt: None,
        };
        let ex = svc.call(mock_exchange("some text")).await.unwrap();
        assert!(ex.input.headers.contains_key("result"));
    }

    #[tokio::test]
    async fn ai_extract_invalid_json_errors() {
        let model = Arc::new(MockChatModel {
            response: "not json at all".into(),
            usage: None,
        });
        let mut svc = AiExtractService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            schema: r#"{"type":"object"}"#.into(),
            output_header: "result".into(),
            prompt: None,
        };
        let err = svc.call(mock_exchange("text")).await.unwrap_err();
        assert!(format!("{err}").contains("invalid JSON response"));
    }

    #[tokio::test]
    async fn ai_extract_schema_mismatch_errors() {
        let model = Arc::new(MockChatModel {
            response: r#"{"name":"John","age":"not-a-number"}"#.into(),
            usage: None,
        });
        let mut svc = AiExtractService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            schema:
                r#"{"type":"object","properties":{"age":{"type":"number"}},"required":["age"]}"#
                    .into(),
            output_header: "result".into(),
            prompt: None,
        };
        let err = svc.call(mock_exchange("text")).await.unwrap_err();
        assert!(format!("{err}").contains("does not match schema"));
    }

    #[tokio::test]
    async fn ai_extract_invalid_schema_errors() {
        let model = Arc::new(MockChatModel {
            response: r#"{"a":1}"#.into(),
            usage: None,
        });
        let mut svc = AiExtractService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            schema: "not valid json".into(),
            output_header: "result".into(),
            prompt: None,
        };
        let err = svc.call(mock_exchange("text")).await.unwrap_err();
        assert!(format!("{err}").contains("invalid schema JSON"));
    }

    #[tokio::test]
    async fn ai_extract_sets_observability_headers() {
        let model = Arc::new(MockChatModel {
            response: r#"{"answer":"yes"}"#.into(),
            usage: Some(camel_ai::TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        });
        let mut svc = AiExtractService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            schema: r#"{"type":"object","properties":{"answer":{"type":"string"}}}"#.into(),
            output_header: "result".into(),
            prompt: None,
        };
        let ex = svc.call(mock_exchange("text")).await.unwrap();
        assert!(ex.input.headers.contains_key("CamelAiOperation"));
        assert!(ex.input.headers.contains_key("CamelAiLatencyMs"));
        assert!(ex.input.headers.contains_key("CamelAiTotalTokens"));
    }

    #[tokio::test]
    async fn ai_extract_think_is_false() {
        let model = Arc::new(MockChatModel {
            response: r#"{"ok":true}"#.into(),
            usage: None,
        });
        let mut svc = AiExtractService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            schema: r#"{"type":"object","properties":{"ok":{"type":"boolean"}}}"#.into(),
            output_header: "result".into(),
            prompt: None,
        };
        let _ = svc.call(mock_exchange("text")).await.unwrap();
    }

    #[tokio::test]
    async fn ai_classify_normalizes_response() {
        let model = Arc::new(MockChatModel {
            response: "\"Billing\".".into(),
            usage: None,
        });
        let mut svc = AiClassifyService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            labels: vec!["billing".into(), "technical".into()],
            output_header: "cat".into(),
            on_unknown: None,
            fallback_label: None,
        };
        let ex = svc.call(mock_exchange("text")).await.unwrap();
        assert_eq!(
            ex.input.headers.get("cat").unwrap().as_str(),
            Some("billing")
        );
    }

    #[tokio::test]
    async fn ai_classify_on_unknown_error() {
        let model = Arc::new(MockChatModel {
            response: "unknown_category".into(),
            usage: None,
        });
        let mut svc = AiClassifyService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            labels: vec!["billing".into()],
            output_header: "cat".into(),
            on_unknown: Some("error".into()),
            fallback_label: None,
        };
        let err = svc.call(mock_exchange("text")).await.unwrap_err();
        assert!(format!("{err}").contains("unknown category"));
    }

    #[tokio::test]
    async fn ai_classify_on_unknown_fallback() {
        let model = Arc::new(MockChatModel {
            response: "unknown_category".into(),
            usage: None,
        });
        let mut svc = AiClassifyService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            labels: vec!["billing".into()],
            output_header: "cat".into(),
            on_unknown: Some("fallback".into()),
            fallback_label: Some("billing".into()),
        };
        let ex = svc.call(mock_exchange("text")).await.unwrap();
        assert_eq!(
            ex.input.headers.get("cat").unwrap().as_str(),
            Some("billing")
        );
    }

    #[tokio::test]
    async fn ai_classify_on_unknown_raw() {
        let model = Arc::new(MockChatModel {
            response: "something_else".into(),
            usage: None,
        });
        let mut svc = AiClassifyService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            labels: vec!["billing".into()],
            output_header: "cat".into(),
            on_unknown: Some("raw".into()),
            fallback_label: None,
        };
        let ex = svc.call(mock_exchange("text")).await.unwrap();
        assert_eq!(
            ex.input.headers.get("cat").unwrap().as_str(),
            Some("something_else")
        );
    }

    #[tokio::test]
    async fn ai_classify_sets_observability_headers() {
        let model = Arc::new(MockChatModel {
            response: "billing".into(),
            usage: Some(camel_ai::TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        });
        let mut svc = AiClassifyService {
            model,
            provider_name: "ollama".into(),
            model_name: "qwen3.5:4b".into(),
            labels: vec!["billing".into()],
            output_header: "cat".into(),
            on_unknown: None,
            fallback_label: None,
        };
        let ex = svc.call(mock_exchange("text")).await.unwrap();
        assert!(ex.input.headers.contains_key("CamelAiOperation"));
        assert!(ex.input.headers.contains_key("CamelAiLatencyMs"));
    }
}
