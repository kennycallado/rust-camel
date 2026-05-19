use crate::client::{BridgeState, StylesheetId, XsltBridgeClient};
use crate::component::XsltBridgeRuntime;
use camel_component_api::{Body, CamelError, Exchange};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::OnceCell;
use tower::Service;
use tracing::{debug, error, warn};

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

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &*self.runtime.state_rx().borrow() {
            BridgeState::Ready { .. } => Poll::Ready(Ok(())),
            BridgeState::Starting | BridgeState::Restarting { .. } => {
                // Spawn a task that waits for the next state change and then
                // wakes this future. Avoids busy-poll while honouring Tower contract.
                // Guard with try_current: in contexts without a Tokio runtime (e.g.
                // unit tests) fall back to an immediate wake so the caller can retry.
                let waker = cx.waker().clone();
                let state_rx_arc = Arc::clone(self.runtime.state_rx());
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let mut rx = (*state_rx_arc).clone();
                        let _ = rx.changed().await;
                        waker.wake();
                    });
                } else {
                    waker.wake_by_ref();
                }
                Poll::Pending
            }
            BridgeState::Degraded(reason) => {
                warn!(reason = %reason, "xslt bridge degraded");
                Poll::Ready(Err(CamelError::ProcessorError(format!(
                    "xslt bridge degraded: {reason}"
                ))))
            }
            BridgeState::Stopped => {
                warn!("xslt bridge stopped");
                Poll::Ready(Err(CamelError::ProcessorError(
                    "xslt bridge stopped".to_string(),
                )))
            }
        }
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
            let stylesheet_id =
                compiled
                    .get_or_try_init(|| async {
                        debug!("xslt bridge starting");
                        runtime
                            .ensure_bridge_started(client.as_ref())
                            .await
                            .map_err(|e: crate::error::XsltError| {
                                CamelError::ProcessorError(e.to_string())
                            })?;
                        debug!("stylesheet compiled");
                        client.compile(stylesheet_bytes).await.map_err(
                            |e: crate::error::XsltError| CamelError::ProcessorError(e.to_string()),
                        )
                    })
                    .await
                    .map_err(|err| {
                        error!(error = %err, "stylesheet compilation failed");
                        err
                    })?;

            let input_body = std::mem::take(&mut exchange.input.body);
            let document = input_body
                .materialize()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("XSLT input body error: {e}")))?
                .to_vec();

            let transformed = runtime
                .transform_with_retry(&client, stylesheet_id, document, params, output_method)
                .await
                .map_err(|e| {
                    let err = CamelError::ProcessorError(e.to_string());
                    error!(error = %err, "xslt transform failed");
                    err
                })?;

            exchange.input.body = Body::from(transformed);
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::XsltBridgeRuntime;
    use crate::config::XsltComponentConfig;
    use std::sync::Arc;
    use std::task::{RawWaker, RawWakerVTable, Waker};
    use tokio::sync::{Mutex, watch};

    fn noop_waker() -> Waker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    fn make_producer_with_state(initial_state: BridgeState) -> XsltProducer {
        let (state_tx, state_rx) = watch::channel(initial_state);
        let state_rx_arc = Arc::new(state_rx);
        let client = Arc::new(XsltBridgeClient::new(Arc::clone(&state_rx_arc)));
        let runtime = Arc::new(XsltBridgeRuntime::new(
            XsltComponentConfig::default(),
            Arc::new(Mutex::new(None)),
            state_tx,
            state_rx_arc,
        ));
        XsltProducer::new(
            vec![],
            Arc::new(OnceCell::new()),
            vec![],
            None,
            client,
            runtime,
        )
    }

    #[test]
    fn test_poll_ready_bridge_starting() {
        let mut producer = make_producer_with_state(BridgeState::Starting);
        let result = producer.poll_ready(&mut Context::from_waker(&noop_waker()));
        assert!(matches!(result, Poll::Pending));
    }

    #[tokio::test]
    async fn test_poll_ready_bridge_ready() {
        let channel = tonic::transport::Channel::from_static("http://[::]:0").connect_lazy();
        let mut producer = make_producer_with_state(BridgeState::Ready { channel });
        let result = producer.poll_ready(&mut Context::from_waker(&noop_waker()));
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_poll_ready_bridge_degraded() {
        let mut producer =
            make_producer_with_state(BridgeState::Degraded("connection lost".to_string()));
        let result = producer.poll_ready(&mut Context::from_waker(&noop_waker()));
        assert!(matches!(result, Poll::Ready(Err(_))));
    }

    #[test]
    fn test_poll_ready_bridge_stopped() {
        let mut producer = make_producer_with_state(BridgeState::Stopped);
        let result = producer.poll_ready(&mut Context::from_waker(&noop_waker()));
        assert!(matches!(result, Poll::Ready(Err(_))));
    }
}
