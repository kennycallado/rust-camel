use async_trait::async_trait;
use camel_component_api::{Body, CamelError, Exchange, Message};
use camel_component_api::{ConcurrencyModel, Consumer, ConsumerContext};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::component::JmsComponent;
use crate::config::JmsEndpointConfig;
use crate::headers::apply_jms_headers;
use crate::proto::{JmsMessage, SubscribeRequest, bridge_service_client::BridgeServiceClient};

pub struct JmsConsumer {
    component: Arc<JmsComponent>,
    endpoint_config: JmsEndpointConfig,
    reconnect_interval_ms: u64,
    cancel_token: Option<CancellationToken>,
    task_handle: Option<JoinHandle<Result<(), CamelError>>>,
}

impl JmsConsumer {
    pub fn new(
        component: Arc<JmsComponent>,
        endpoint_config: JmsEndpointConfig,
        reconnect_interval_ms: u64,
    ) -> Self {
        Self {
            component,
            endpoint_config,
            reconnect_interval_ms,
            cancel_token: None,
            task_handle: None,
        }
    }
}

fn build_exchange(msg: JmsMessage) -> Exchange {
    let body_bytes = msg.body.clone();
    let body = if msg.content_type.starts_with("text/") {
        match String::from_utf8(body_bytes.clone()) {
            Ok(s) => Body::Text(s),
            Err(_) => Body::Bytes(bytes::Bytes::from(body_bytes)),
        }
    } else if msg.content_type.contains("json") {
        match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
            Ok(v) => Body::Json(v),
            Err(_) => Body::Bytes(bytes::Bytes::from(body_bytes)),
        }
    } else if body_bytes.is_empty() {
        Body::Empty
    } else {
        Body::Bytes(bytes::Bytes::from(body_bytes))
    };

    let mut exchange = Exchange::new(Message::new(body));
    apply_jms_headers(&mut exchange, &msg);
    exchange
}

#[async_trait]
impl Consumer for JmsConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let component = self.component.clone();
        let destination = self.endpoint_config.destination();
        let reconnect_interval_ms = self.reconnect_interval_ms;
        let subscription_id = Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        self.cancel_token = Some(cancel.clone());

        let handle = tokio::spawn(async move {
            let request = SubscribeRequest {
                destination: destination.clone(),
                subscription_id,
            };

            // Obtain an initial channel; re-fetch on each reconnection attempt
            // so that if the bridge restarts on a different port, we pick up
            // the new address instead of keeping a stale channel.
            let mut channel = match component.ensure_bridge().await {
                Ok(ch) => ch,
                Err(e) => {
                    warn!("JMS consumer initial bridge unavailable for {destination}: {e}");
                    // Fall into the retry loop below
                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                info!("JMS consumer cancelled for {destination}");
                                return Ok(());
                            }
                            _ = tokio::time::sleep(Duration::from_millis(reconnect_interval_ms)) => {}
                        }
                        match component.ensure_bridge().await {
                            Ok(ch) => break ch,
                            Err(e) => {
                                warn!(
                                    "JMS consumer still cannot reach bridge for {destination}: {e}"
                                );
                            }
                        }
                    }
                }
            };

            loop {
                let mut client = BridgeServiceClient::new(channel.clone());
                let mut stream = match client.subscribe(request.clone()).await {
                    Ok(resp) => resp.into_inner(),
                    Err(e) => {
                        warn!(
                            "JMS subscribe failed for {destination}: {e}, refreshing bridge and retrying in {reconnect_interval_ms}ms"
                        );
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                info!("JMS consumer cancelled for {destination}");
                                return Ok(());
                            }
                            _ = tokio::time::sleep(Duration::from_millis(reconnect_interval_ms)) => {}
                        }
                        // Re-obtain channel from component; this will restart the
                        // bridge if needed and return a fresh channel pointing to
                        // the new port.
                        channel = match component.ensure_bridge().await {
                            Ok(ch) => ch,
                            Err(e2) => {
                                warn!("JMS consumer cannot refresh bridge for {destination}: {e2}");
                                channel // keep old channel, will retry next iteration
                            }
                        };
                        continue;
                    }
                };

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            info!("JMS consumer cancelled for {destination}");
                            return Ok(());
                        }
                        msg = stream.message() => {
                            match msg {
                                Ok(Some(jms_msg)) => {
                                    let exchange = build_exchange(jms_msg);
                                    if let Err(e) = ctx.send(exchange).await {
                                        error!("JMS consumer route error: {e}");
                                    }
                                }
                                Ok(None) => {
                                    info!("JMS stream ended for {destination}, reconnecting...");
                                    break;
                                }
                                Err(e) => {
                                    warn!(
                                        "JMS stream error for {destination}: {e}, reconnecting in {reconnect_interval_ms}ms"
                                    );
                                    break;
                                }
                            }
                        }
                    }
                }

                if cancel.is_cancelled() {
                    info!("JMS consumer cancelled for {destination}");
                    return Ok(());
                }

                tokio::time::sleep(Duration::from_millis(reconnect_interval_ms)).await;

                // After a stream error/close, refresh the channel in case the
                // bridge was restarted while this consumer was running.
                channel = match component.ensure_bridge().await {
                    Ok(ch) => ch,
                    Err(e) => {
                        warn!(
                            "JMS consumer cannot refresh bridge after stream close for {destination}: {e}"
                        );
                        channel
                    }
                };
            }
        });

        self.task_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.task_handle.take() {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    warn!("JMS consumer task returned error on stop: {e}");
                }
                Err(join_err) => {
                    warn!("JMS consumer task panicked on stop: {join_err}");
                }
            }
        }
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::JmsConfig;

    #[test]
    fn build_exchange_text_body() {
        let msg = JmsMessage {
            message_id: "ID:1".to_string(),
            body: b"hello world".to_vec(),
            content_type: "text/plain".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(msg);
        assert!(matches!(ex.input.body, Body::Text(_)));
    }

    #[test]
    fn build_exchange_binary_body() {
        let msg = JmsMessage {
            message_id: "ID:2".to_string(),
            body: vec![0x00, 0x01, 0x02],
            content_type: "application/octet-stream".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(msg);
        assert!(matches!(ex.input.body, Body::Bytes(_)));
    }

    #[test]
    fn build_exchange_json_body() {
        let msg = JmsMessage {
            message_id: "ID:json".to_string(),
            body: br#"{"ok":true}"#.to_vec(),
            content_type: "application/json".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(msg);
        assert!(matches!(ex.input.body, Body::Json(_)));
    }

    #[test]
    fn build_exchange_empty_body() {
        let msg = JmsMessage {
            message_id: "ID:3".to_string(),
            body: vec![],
            content_type: "".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(msg);
        assert!(matches!(ex.input.body, Body::Empty));
    }

    #[tokio::test]
    async fn stop_without_start_is_noop() {
        let component = Arc::new(JmsComponent::new(JmsConfig::default()));
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(component, endpoint_cfg, 50);
        assert!(consumer.stop().await.is_ok());
    }
}
