use crate::client::{StylesheetId, XsltBridgeClient};
use crate::component::XsltBridgeRuntime;
use camel_component_api::{Body, CamelError, Exchange};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::OnceCell;
use tower::Service;

#[derive(Clone)]
pub struct XsltProducer {
    stylesheet_bytes: Vec<u8>,
    compiled: Arc<OnceCell<StylesheetId>>,
    params: Vec<(String, String)>,
    output_method: Option<String>,
    client: Arc<XsltBridgeClient>,
    runtime: Arc<XsltBridgeRuntime>,
}

impl XsltProducer {
    pub fn new(
        stylesheet_bytes: Vec<u8>,
        compiled: Arc<OnceCell<StylesheetId>>,
        params: Vec<(String, String)>,
        output_method: Option<String>,
        client: Arc<XsltBridgeClient>,
        runtime: Arc<XsltBridgeRuntime>,
    ) -> Self {
        Self {
            stylesheet_bytes,
            compiled,
            params,
            output_method,
            client,
            runtime,
        }
    }
}

impl Service<Exchange> for XsltProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let stylesheet_bytes = self.stylesheet_bytes.clone();
        let compiled = Arc::clone(&self.compiled);
        let params = self.params.clone();
        let output_method = self.output_method.clone();
        let client = Arc::clone(&self.client);
        let runtime = Arc::clone(&self.runtime);

        Box::pin(async move {
            // Lazy: start bridge and compile stylesheet on first call
            let stylesheet_id = compiled
                .get_or_try_init(|| async {
                    runtime
                        .ensure_bridge_started(client.as_ref())
                        .await
                        .map_err(|e: crate::error::XsltError| {
                            CamelError::ProcessorError(e.to_string())
                        })?;
                    client
                        .compile(stylesheet_bytes)
                        .await
                        .map_err(|e: crate::error::XsltError| {
                            CamelError::ProcessorError(e.to_string())
                        })
                })
                .await?;

            let input_body = std::mem::take(&mut exchange.input.body);
            let document = input_body
                .materialize()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("XSLT input body error: {e}")))?
                .to_vec();

            let transformed = runtime
                .transform_with_retry(&client, stylesheet_id, document, params, output_method)
                .await
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;

            exchange.input.body = Body::from(transformed);
            Ok(exchange)
        })
    }
}
