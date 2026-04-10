use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_component_api::{
    Body, CamelError, ConcurrencyModel, Consumer, ConsumerContext, Exchange, Message,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::component::{BridgeState, JmsBridgePool, is_bridge_transport_error};
use crate::config::{DestinationType, JmsEndpointConfig};
use crate::headers::apply_jms_headers;
use crate::proto::{JmsMessage, SubscribeRequest, bridge_service_client::BridgeServiceClient};

pub struct JmsConsumer {
    pool: Arc<JmsBridgePool>,
    broker_name: String,
    endpoint_config: JmsEndpointConfig,
    reconnect_interval_ms: u64,
    cancel_token: Option<CancellationToken>,
    task_handle: Option<JoinHandle<()>>,
}

impl JmsConsumer {
    pub fn new(
        pool: Arc<JmsBridgePool>,
        broker_name: String,
        endpoint_config: JmsEndpointConfig,
        reconnect_interval_ms: u64,
    ) -> Self {
        Self {
            pool,
            broker_name,
            endpoint_config,
            reconnect_interval_ms,
            cancel_token: None,
            task_handle: None,
        }
    }
}

fn build_exchange(msg: &JmsMessage) -> Exchange {
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
    apply_jms_headers(&mut exchange, msg);
    exchange
}

fn destination(endpoint_config: &JmsEndpointConfig) -> String {
    format!(
        "{}:{}",
        match endpoint_config.destination_type {
            DestinationType::Queue => "queue",
            DestinationType::Topic => "topic",
        },
        endpoint_config.destination_name
    )
}

async fn await_ready_channel(
    pool: &JmsBridgePool,
    broker_name: &str,
) -> Result<Channel, CamelError> {
    let slot = pool.get_or_create_slot(broker_name).await?;
    let mut rx = slot.state_rx.clone();

    loop {
        match &*rx.borrow() {
            BridgeState::Ready { channel } => return Ok(channel.clone()),
            BridgeState::Stopped => {
                return Err(CamelError::ProcessorError(format!(
                    "JMS broker '{}' is stopped",
                    broker_name
                )));
            }
            _ => {}
        }

        if rx.changed().await.is_err() {
            return Err(CamelError::ProcessorError(format!(
                "JMS broker '{}' state channel closed",
                broker_name
            )));
        }
    }
}

#[async_trait]
impl Consumer for JmsConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let pool = Arc::clone(&self.pool);
        let broker_name = self.broker_name.clone();
        let endpoint_config = self.endpoint_config.clone();
        let reconnect_interval_ms = self.reconnect_interval_ms;
        let cancel = CancellationToken::new();
        self.cancel_token = Some(cancel.clone());

        let handle = tokio::spawn(async move {
            let destination = destination(&endpoint_config);
            let mut consecutive_transport_failures: u32 = 0;
            loop {
                let channel = tokio::select! {
                    _ = cancel.cancelled() => {
                        info!(broker = %broker_name, destination = %destination, "JMS consumer cancelled");
                        break;
                    }
                    _ = ctx.cancelled() => {
                        info!(broker = %broker_name, destination = %destination, "JMS consumer context cancelled");
                        break;
                    }
                    result = await_ready_channel(&pool, &broker_name) => {
                        match result {
                            Ok(channel) => channel,
                            Err(e) => {
                                warn!(
                                    broker = %broker_name,
                                    destination = %destination,
                                    error = %e,
                                    "JMS consumer waiting for ready bridge failed"
                                );
                                tokio::select! {
                                    _ = cancel.cancelled() => break,
                                    _ = ctx.cancelled() => break,
                                    _ = tokio::time::sleep(Duration::from_millis(reconnect_interval_ms)) => {}
                                }
                                continue;
                            }
                        }
                    }
                };

                let mut client = BridgeServiceClient::new(channel);
                let mut stream = match client
                    .subscribe(SubscribeRequest {
                        destination: destination.clone(),
                        subscription_id: Uuid::new_v4().to_string(),
                    })
                    .await
                    .map_err(|e| {
                        CamelError::ProcessorError(format!("JMS gRPC subscribe error: {e}"))
                    }) {
                    Ok(resp) => {
                        consecutive_transport_failures = 0;
                        info!(broker = %broker_name, destination = %destination, "JMS consumer subscribed successfully");
                        resp.into_inner()
                    }
                    Err(e) => {
                        if is_bridge_transport_error(&e) {
                            consecutive_transport_failures += 1;
                            if consecutive_transport_failures >= 2 {
                                warn!(
                                    broker = %broker_name,
                                    destination = %destination,
                                    failures = consecutive_transport_failures,
                                    "JMS subscribe transport failures exceeded threshold; refreshing channel"
                                );
                                if let Err(refresh_err) =
                                    pool.refresh_slot_channel(&broker_name).await
                                {
                                    warn!(
                                        broker = %broker_name,
                                        destination = %destination,
                                        error = %refresh_err,
                                        "JMS channel refresh failed; requesting bridge restart"
                                    );
                                    pool.restart_slot(&broker_name);
                                }
                                consecutive_transport_failures = 0;
                            }
                        } else {
                            consecutive_transport_failures = 0;
                        }
                        warn!(
                            broker = %broker_name,
                            destination = %destination,
                            error = %e,
                            "JMS subscribe failed; retrying"
                        );
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = ctx.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_millis(reconnect_interval_ms)) => {}
                        }
                        continue;
                    }
                };

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            info!(broker = %broker_name, destination = %destination, "JMS consumer cancelled");
                            return;
                        }
                        _ = ctx.cancelled() => {
                            info!(broker = %broker_name, destination = %destination, "JMS consumer context cancelled");
                            return;
                        }
                        msg = stream.message() => {
                            match msg {
                                Ok(Some(jms_msg)) => {
                                    let exchange = build_exchange(&jms_msg);
                                    if let Err(e) = ctx.send(exchange).await {
                                        error!("JMS consumer route error: {e}");
                                    }
                                }
                                Ok(None) => {
                                    info!(broker = %broker_name, destination = %destination, "JMS stream ended; reconnecting");
                                    break;
                                }
                                Err(e) => {
                                    let subscribe_err = CamelError::ProcessorError(format!(
                                        "JMS gRPC subscribe error: {e}"
                                    ));
                                    if is_bridge_transport_error(&subscribe_err) {
                                        consecutive_transport_failures += 1;
                                        if consecutive_transport_failures >= 2 {
                                            warn!(
                                                broker = %broker_name,
                                                destination = %destination,
                                                failures = consecutive_transport_failures,
                                                "JMS stream transport failures exceeded threshold; refreshing channel"
                                            );
                                            if let Err(refresh_err) =
                                                pool.refresh_slot_channel(&broker_name).await
                                            {
                                                warn!(
                                                    broker = %broker_name,
                                                    destination = %destination,
                                                    error = %refresh_err,
                                                    "JMS channel refresh failed; requesting bridge restart"
                                                );
                                                pool.restart_slot(&broker_name);
                                            }
                                            consecutive_transport_failures = 0;
                                        }
                                    } else {
                                        consecutive_transport_failures = 0;
                                    }
                                    warn!(
                                        broker = %broker_name,
                                        destination = %destination,
                                        error = %subscribe_err,
                                        "JMS stream error; reconnecting"
                                    );
                                    break;
                                }
                            }
                        }
                    }
                }

                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ctx.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(reconnect_interval_ms)) => {}
                }
            }
        });

        self.task_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.task_handle.take()
            && let Err(join_err) = handle.await
        {
            warn!("JMS consumer task panicked on stop: {join_err}");
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
    use crate::BrokerType;
    use crate::config::JmsPoolConfig;

    #[test]
    fn build_exchange_text_body() {
        let msg = JmsMessage {
            message_id: "ID:1".to_string(),
            body: b"hello world".to_vec(),
            content_type: "text/plain".to_string(),
            ..Default::default()
        };
        let ex = build_exchange(&msg);
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
        let ex = build_exchange(&msg);
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
        let ex = build_exchange(&msg);
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
        let ex = build_exchange(&msg);
        assert!(matches!(ex.input.body, Body::Empty));
    }

    #[tokio::test]
    async fn stop_without_start_is_noop() {
        let pool = Arc::new(
            JmsBridgePool::from_config(JmsPoolConfig::single_broker(
                "tcp://localhost:61616",
                BrokerType::Generic,
            ))
            .unwrap(),
        );
        let endpoint_cfg = crate::config::JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = JmsConsumer::new(pool, "default".to_string(), endpoint_cfg, 50);
        assert!(consumer.stop().await.is_ok());
    }
}
