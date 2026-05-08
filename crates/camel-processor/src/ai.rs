use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use camel_ai::{ChatMessage, ChatModel, ChatRequest, ChatRole};
use camel_api::{CamelError, Exchange};
use tower::Service;

#[derive(Clone)]
pub struct AiClassifyService {
    pub model: Arc<dyn ChatModel>,
    pub labels: Vec<String>,
    pub output_header: String,
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
        let labels = self.labels.clone();
        let output_header = self.output_header.clone();

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
            let category = resp.content.trim().to_lowercase();
            let matched = labels
                .iter()
                .find(|l| l.to_lowercase() == category)
                .cloned()
                .unwrap_or_else(|| {
                    tracing::warn!(category = %category, "ai_classify: LLM returned unknown category, using raw value");
                    category
                });

            exchange
                .input
                .headers
                .insert(output_header, serde_json::Value::String(matched));
            Ok(exchange)
        })
    }
}

#[derive(Clone)]
pub struct AiExtractService {
    pub model: Arc<dyn ChatModel>,
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
            let json_val: serde_json::Value =
                serde_json::from_str(resp.content.trim()).map_err(|e| {
                    CamelError::RouteError(format!("AI extract: invalid JSON response: {e}"))
                })?;

            exchange.input.headers.insert(output_header, json_val);
            Ok(exchange)
        })
    }
}
