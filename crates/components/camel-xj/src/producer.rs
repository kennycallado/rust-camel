use crate::component::XjBridgeRuntime;
use crate::config::Direction;
use camel_component_api::{Body, CamelError, Exchange};
use camel_xslt::{StylesheetId, XsltBridgeClient};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::Service;

#[derive(Clone)]
pub struct XjProducer {
    stylesheet_id: StylesheetId,
    params: Vec<(String, String)>,
    client: Arc<XsltBridgeClient>,
    runtime: Arc<XjBridgeRuntime>,
    direction: Direction,
}

impl XjProducer {
    pub fn new(
        stylesheet_id: StylesheetId,
        params: Vec<(String, String)>,
        client: Arc<XsltBridgeClient>,
        runtime: Arc<XjBridgeRuntime>,
        direction: Direction,
    ) -> Self {
        Self {
            stylesheet_id,
            params,
            client,
            runtime,
            direction,
        }
    }
}

impl Service<Exchange> for XjProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let stylesheet_id = self.stylesheet_id.clone();
        let mut params = self.params.clone();
        let client = Arc::clone(&self.client);
        let runtime = Arc::clone(&self.runtime);
        let direction = self.direction;

        Box::pin(async move {
            let input_body = std::mem::take(&mut exchange.input.body);
            let raw = input_body
                .materialize()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("XJ input body error: {e}")))?
                .to_vec();

            // For json2xml the input is JSON, not XML. Saxon cannot parse JSON as a SAXSource.
            // Pass the JSON as the XSLT parameter `jsonInput` and send a minimal XML document.
            let document = if direction == Direction::JsonToXml {
                let json_str = String::from_utf8(raw).map_err(|e| {
                    CamelError::ProcessorError(format!("XJ: input is not valid UTF-8: {e}"))
                })?;
                params.push(("jsonInput".to_string(), json_str));
                b"<root/>".to_vec()
            } else {
                raw
            };

            let transformed = runtime
                .transform_with_retry(&client, &stylesheet_id, document, params)
                .await
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;

            exchange.input.body = Body::from(transformed);
            Ok(exchange)
        })
    }
}
