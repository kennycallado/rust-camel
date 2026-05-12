use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use camel_api::body::Body;
use camel_api::{CamelError, Exchange};
use tower::Service;

#[derive(Clone)]
pub struct PromptTemplateService {
    pub template: String,
}

impl PromptTemplateService {
    fn render(&self, exchange: &Exchange) -> String {
        let mut result = self.template.clone();
        let body = exchange.input.body.as_text().unwrap_or("");
        result = result.replace("${body}", body);

        for (key, value) in &exchange.input.headers {
            let pattern = format!("${{header.{key}}}");
            let replacement = value.as_str().unwrap_or("");
            result = result.replace(&pattern, replacement);
        }

        result
    }
}

impl Service<Exchange> for PromptTemplateService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let rendered = self.render(&exchange);
        exchange.input.body = Body::Text(rendered);
        Box::pin(async move { Ok(exchange) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;

    fn make_exchange(body: &str) -> Exchange {
        Exchange::new(Message::new(body))
    }

    fn make_exchange_with_headers(body: &str, headers: &[(&str, &str)]) -> Exchange {
        let mut msg = Message::new(body);
        for (k, v) in headers {
            msg.set_header(k.to_string(), camel_api::Value::String(v.to_string()));
        }
        Exchange::new(msg)
    }

    #[tokio::test]
    async fn template_replaces_body_placeholder() {
        let mut svc = PromptTemplateService {
            template: "Question: ${body}".into(),
        };
        let ex = svc.call(make_exchange("hello")).await.unwrap();
        assert_eq!(ex.input.body.as_text(), Some("Question: hello"));
    }

    #[tokio::test]
    async fn template_replaces_header_placeholder() {
        let mut svc = PromptTemplateService {
            template: "Q: ${header.question}\nCtx: ${body}".into(),
        };
        let ex = svc
            .call(make_exchange_with_headers(
                "some context",
                &[("question", "what is this?")],
            ))
            .await
            .unwrap();
        assert_eq!(
            ex.input.body.as_text(),
            Some("Q: what is this?\nCtx: some context")
        );
    }

    #[tokio::test]
    async fn template_missing_header_leaves_placeholder() {
        let mut svc = PromptTemplateService {
            template: "Q: ${header.missing}".into(),
        };
        let ex = svc.call(make_exchange("body")).await.unwrap();
        assert_eq!(ex.input.body.as_text(), Some("Q: ${header.missing}"));
    }

    #[tokio::test]
    async fn template_no_placeholders() {
        let mut svc = PromptTemplateService {
            template: "static text".into(),
        };
        let ex = svc.call(make_exchange("ignored")).await.unwrap();
        assert_eq!(ex.input.body.as_text(), Some("static text"));
    }
}
