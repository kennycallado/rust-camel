// MQTT Consumer: subscribes to topics and feeds Exchanges into the route pipeline.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use camel_api::CamelError;
use camel_component_api::{
    ConcurrencyModel, Consumer, ConsumerContext, NetworkRetryPolicy, RuntimeObservability,
};
use rumqttc::{AsyncClient, Event, MqttOptions, NetworkOptions, Packet, QoS};

use crate::client_id::build_client_id_with_override;
use crate::config::{AckMode, MqttBrokerConfig, MqttEndpointConfig, check_payload_len};
use crate::headers::build_exchange;

/// Decide whether to (re)subscribe after a ConnAck.
///
/// With `clean_session=false` and a persisted session, the broker reports
/// `session_present=true` and subscriptions are already active — resubscribing
/// would duplicate them. With `session_present=false`, subscribe.
fn should_resubscribe(session_present: bool) -> bool {
    !session_present
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AckDecision {
    /// Do not send a manual ack (QoS 0, or auto-ack mode).
    NoAck,
    /// Ack only after the downstream pipeline succeeds.
    AckAfterSuccess,
}

/// Decide ack handling from the *received* packet QoS (not the configured QoS).
///
/// A QoS 0 message is never acked regardless of mode; under manual mode a QoS 1/2
/// message is acked only after the downstream pipeline succeeds. Using the received
/// QoS matters because a subscription at QoS 1 can still deliver QoS 0 messages.
fn ack_decision(ack_mode: &AckMode, received_qos: QoS) -> AckDecision {
    match (ack_mode, received_qos) {
        (AckMode::Manual, QoS::AtLeastOnce) | (AckMode::Manual, QoS::ExactlyOnce) => {
            AckDecision::AckAfterSuccess
        }
        _ => AckDecision::NoAck,
    }
}

pub struct MqttConsumer {
    config: MqttEndpointConfig,
    broker: MqttBrokerConfig,
    client_id_prefix: String,
    fallback_reconnect: NetworkRetryPolicy,
    runtime: Arc<dyn RuntimeObservability>,
    cancel_token: Option<CancellationToken>,
    task_handle: Option<JoinHandle<Result<(), CamelError>>>,
    route_id: Option<String>,
}

impl MqttConsumer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: MqttEndpointConfig,
        runtime: Arc<dyn RuntimeObservability>,
        broker: MqttBrokerConfig,
        client_id_prefix: String,
        fallback_reconnect: NetworkRetryPolicy,
    ) -> Self {
        Self {
            config,
            broker,
            client_id_prefix,
            fallback_reconnect,
            runtime,
            cancel_token: None,
            task_handle: None,
            route_id: None,
        }
    }
}

pub fn validate_for_consumer(config: &MqttEndpointConfig) -> Result<(), CamelError> {
    if config.subscriptions.is_empty() {
        return Err(CamelError::Config(
            "mqtt consumer: no subscriptions defined (set path topic or ?topics=)".into(),
        ));
    }
    config.validate()
}

#[async_trait]
impl Consumer for MqttConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        if self.cancel_token.is_some() {
            return Err(CamelError::EndpointCreationFailed(
                "MQTT consumer already started".into(),
            ));
        }

        let route_id = ctx.route_id().to_string();
        self.route_id = Some(route_id.clone());

        let client_id = build_client_id_with_override(
            &self.client_id_prefix,
            &route_id,
            &format!(
                "{}-{}",
                self.config.broker_name,
                self.config.subscriptions.join(",")
            ),
            self.config.client_id_override.as_deref(),
        );

        let cancel_token = ctx.cancel_token();
        self.cancel_token = Some(cancel_token.clone());

        let reconnect = self.config.effective_reconnect(&self.fallback_reconnect);

        let config = self.config.clone();
        let broker = self.broker.clone();
        let runtime = self.runtime.clone();

        let handle = tokio::spawn(run_consumer_loop(
            config,
            broker,
            client_id,
            reconnect,
            ctx,
            cancel_token,
            runtime,
            route_id,
        ));
        self.task_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(token) = &self.cancel_token {
            token.cancel();
        }
        let route_id = self.route_id.as_deref().unwrap_or("?");
        let mut result = Ok(());
        if let Some(mut handle) = self.task_handle.take() {
            match tokio::time::timeout(Duration::from_secs(10), &mut handle).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => {
                    self.runtime
                        .metrics()
                        .increment_errors(route_id, "b-prime:mqtt:stop-task-error");
                    // log-policy: outside-contract
                    error!(route_id = %route_id, "MQTT consumer task error on stop: {e}");
                    result = Err(e);
                }
                Ok(Err(join_err)) => {
                    // log-policy: system-broken
                    error!(route_id = %route_id, "MQTT consumer task panicked: {join_err}");
                    result = Err(CamelError::ProcessorError(format!(
                        "task panicked: {join_err}"
                    )));
                }
                Err(_) => {
                    handle.abort();
                    let _ = handle.await;
                    warn!("MQTT consumer did not stop in 10s; aborted");
                }
            }
        }
        self.cancel_token = None;
        result
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.task_handle.take()
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_consumer_loop(
    config: MqttEndpointConfig,
    broker: MqttBrokerConfig,
    client_id: String,
    reconnect: NetworkRetryPolicy,
    ctx: ConsumerContext,
    cancel_token: CancellationToken,
    runtime: Arc<dyn RuntimeObservability>,
    route_id: String,
) -> Result<(), CamelError> {
    let (host, port) = broker.host_port()?;
    let mut mqtt_opts = MqttOptions::new(&client_id, (host, port));
    mqtt_opts.set_keep_alive(config.keep_alive_secs as u16);
    mqtt_opts.set_clean_session(config.clean_session);
    // set_manual_acks MUST be set before the client builder consumes MqttOptions.
    mqtt_opts.set_manual_acks(config.ack_mode == AckMode::Manual);

    if let (Some(user), Some(pass)) = (&broker.username, &broker.password) {
        mqtt_opts.set_credentials(user, pass.as_bytes().to_vec());
    }

    // TLS setup (feature-gated)
    #[cfg(feature = "tls")]
    if broker.is_tls() {
        let tls_config = crate::tls::build_tls_config(broker.tls_ca_cert.as_deref())?;
        mqtt_opts.set_transport(rumqttc::Transport::tls_with_config(tls_config));
    }
    #[cfg(not(feature = "tls"))]
    if broker.is_tls() {
        return Err(CamelError::Config(
            "mqtt: mqtts:// requires the 'tls' feature on camel-component-mqtt".into(),
        ));
    }

    let (client, mut eventloop) = AsyncClient::builder(mqtt_opts).capacity(10).build();

    let mut net_opts = NetworkOptions::new();
    net_opts.set_connection_timeout(10);
    eventloop.set_network_options(net_opts);

    let qos = config.qos.to_rumqttc();
    let mut attempt: u32 = 0;

    loop {
        tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                info!(route_id = %route_id, "MQTT consumer shutting down");
                break;
            }
            event = eventloop.poll() => {
                match event {
                    Ok(Event::Incoming(Packet::ConnAck(ack))) => {
                        attempt = 0;
                        if should_resubscribe(ack.session_present) {
                            for filter in &config.subscriptions {
                                if let Err(e) = client.subscribe(filter, qos).await {
                                    runtime.metrics().increment_errors(&route_id, "b-prime:mqtt:subscribe-failed");
                                    // log-policy: outside-contract
                                    error!(route_id = %route_id, filter = %filter, error = %e, "Failed to subscribe");
                                    return Err(CamelError::ProcessorError(e.to_string()));
                                }
                            }
                        }
                    }
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        check_payload_len(publish.payload.len(), config.max_payload_bytes)?;

                        let exchange = build_exchange(&publish, &client_id);
                        match ack_decision(&config.ack_mode, publish.qos) {
                            AckDecision::NoAck => {
                                ctx.send(exchange).await?;
                            }
                            AckDecision::AckAfterSuccess => {
                                match ctx.send_and_wait(exchange).await {
                                    Ok(_) => {
                                        if let Err(e) = client.ack(&publish).await {
                                            runtime.metrics().increment_errors(&route_id, "b-prime:mqtt:ack-failed");
                                            // log-policy: outside-contract
                                            error!(route_id = %route_id, error = %e, "Failed to send MQTT ack");
                                            return Err(CamelError::ProcessorError(e.to_string()));
                                        }
                                    }
                                    Err(e) => {
                                        runtime.metrics().increment_errors(&route_id, "b-prime:mqtt:pipeline-failed");
                                        // log-policy: outside-contract
                                        error!(route_id = %route_id, error = %e, "Pipeline failed; closing MQTT connection for redelivery");
                                        return Err(e);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(route_id = %route_id, error = %e, "MQTT connection error; will retry");
                        let delay = reconnect.delay_for(attempt);
                        attempt = attempt.saturating_add(1);
                        tokio::select! {
                            biased;
                            _ = cancel_token.cancelled() => break,
                            _ = tokio::time::sleep(delay) => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AckMode, MqttEndpointConfig};
    use rumqttc::QoS;

    #[test]
    fn consumer_validate_rejects_empty_subscriptions() {
        let config = MqttEndpointConfig::default();
        let result = validate_for_consumer(&config);
        assert!(result.is_err());
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn consumer_validate_accepts_valid_config() {
        let mut config = MqttEndpointConfig::default();
        config.broker_name = "local".to_string();
        config.subscriptions = vec!["sensors/#".to_string()];
        let result = validate_for_consumer(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn concurrency_model_is_sequential() {
        let config = MqttEndpointConfig::default();
        let rt = std::sync::Arc::new(camel_component_api::NoopRuntimeObservability);
        let consumer = MqttConsumer::new(
            config,
            rt,
            crate::config::MqttBrokerConfig {
                url: "mqtt://localhost:1883".to_string(),
                username: None,
                password: None,
                tls_ca_cert: None,
            },
            "camel".to_string(),
            NetworkRetryPolicy::default(),
        );
        assert!(matches!(
            consumer.concurrency_model(),
            camel_component_api::ConcurrencyModel::Sequential
        ));
    }

    #[test]
    fn resubscribe_decision_respects_session_present() {
        assert!(should_resubscribe(false));
        assert!(!should_resubscribe(true));
    }

    #[test]
    fn ack_decision_matches_qos_and_mode() {
        assert_eq!(
            ack_decision(&AckMode::Manual, QoS::AtMostOnce),
            AckDecision::NoAck
        );
        assert_eq!(
            ack_decision(&AckMode::Manual, QoS::AtLeastOnce),
            AckDecision::AckAfterSuccess
        );
        assert_eq!(
            ack_decision(&AckMode::Manual, QoS::ExactlyOnce),
            AckDecision::AckAfterSuccess
        );
        assert_eq!(
            ack_decision(&AckMode::Auto, QoS::AtLeastOnce),
            AckDecision::NoAck
        );
    }

    #[test]
    fn payload_limit_rejects_oversized() {
        assert!(check_payload_len(100, 50).is_err());
        assert!(check_payload_len(50, 50).is_ok());
        assert!(check_payload_len(0, 50).is_ok());
    }
}
