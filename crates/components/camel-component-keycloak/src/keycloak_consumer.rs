use std::sync::Arc;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_component_api::{
    Body, Consumer, ConsumerContext, Exchange, Message, RuntimeObservability,
};
use indexmap::IndexSet;
use tracing::{debug, error, info, warn};

use crate::event_types::{AdminEventRepresentation, EventRepresentation};
use crate::events_endpoint_config::EventType;
use crate::events_endpoint_config::EventsEndpointConfig;

enum PollError {
    Auth(String),
    Transient(String),
}

enum BatchOutcome {
    Updated(u64),
    ChannelClosed,
}

trait KeycloakEvent {
    fn event_id(&self) -> Option<&String>;
    fn event_time(&self) -> Option<u64>;
}

impl KeycloakEvent for EventRepresentation {
    fn event_id(&self) -> Option<&String> {
        self.id.as_ref()
    }

    fn event_time(&self) -> Option<u64> {
        self.time
    }
}

impl KeycloakEvent for AdminEventRepresentation {
    fn event_id(&self) -> Option<&String> {
        self.id.as_ref()
    }

    fn event_time(&self) -> Option<u64> {
        self.time
    }
}

const HEADER_EVENT_ID: &str = "CamelKeycloakEventId";
const HEADER_EVENT_TIME: &str = "CamelKeycloakEventTime";
const HEADER_REALM_ID: &str = "CamelKeycloakRealmId";
const HEADER_EVENT_TYPE: &str = "CamelKeycloakEventType";
const HEADER_CLIENT_ID: &str = "CamelKeycloakClientId";
const HEADER_USER_ID: &str = "CamelKeycloakUserId";
const HEADER_SESSION_ID: &str = "CamelKeycloakSessionId";
const HEADER_IP_ADDRESS: &str = "CamelKeycloakIpAddress";
const HEADER_RESOURCE_TYPE: &str = "CamelKeycloakResourceType";
const HEADER_RESOURCE_PATH: &str = "CamelKeycloakResourcePath";
const HEADER_AUTH_USER_ID: &str = "CamelKeycloakAuthUserId";
const HEADER_AUTH_CLIENT_ID: &str = "CamelKeycloakAuthClientId";

fn set_opt(msg: &mut Message, key: &str, val: Option<&String>) {
    if let Some(v) = val {
        msg.set_header(key, v.clone());
    }
}

pub struct KeycloakEventConsumer {
    config: EventsEndpointConfig,
    /// Phase B will use this for `rt.metrics().increment_errors(...)` and
    /// `rt.health().force_unhealthy_for_route(...)` calls per ADR-0012.
    runtime: Arc<dyn RuntimeObservability>,
}

impl KeycloakEventConsumer {
    pub fn new(config: EventsEndpointConfig, runtime: Arc<dyn RuntimeObservability>) -> Self {
        Self { config, runtime }
    }

    async fn fetch_user_events(
        &self,
        token: &str,
        date_from: u64,
    ) -> Result<Vec<EventRepresentation>, PollError> {
        let url = self.config.build_url(date_from);
        let response = self
            .config
            .http
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| PollError::Transient(format!("events HTTP error: {e}")))?;

        let status = response.status();
        if status.is_success() {
            response
                .json::<Vec<EventRepresentation>>()
                .await
                .map_err(|e| PollError::Transient(format!("events parse error: {e}")))
        } else if status.as_u16() == 401 || status.as_u16() == 403 {
            Err(PollError::Auth(format!("events auth error: {}", status)))
        } else {
            let body = response.text().await.unwrap_or_default();
            let message = format!("events HTTP error: {} {}", status, body); // allow-secret
            Err(PollError::Transient(message))
        }
    }

    async fn fetch_admin_events(
        &self,
        token: &str,
        date_from: u64,
    ) -> Result<Vec<AdminEventRepresentation>, PollError> {
        let url = self.config.build_url(date_from);
        let response = self
            .config
            .http
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| PollError::Transient(format!("admin-events HTTP error: {e}")))?;

        let status = response.status();
        if status.is_success() {
            response
                .json::<Vec<AdminEventRepresentation>>()
                .await
                .map_err(|e| PollError::Transient(format!("admin-events parse error: {e}")))
        } else if status.as_u16() == 401 || status.as_u16() == 403 {
            Err(PollError::Auth(format!(
                "admin-events auth error: {}",
                status
            )))
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(PollError::Transient(format!(
                "admin-events HTTP error: {} {}",
                status, body
            )))
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_event_batch<E: KeycloakEvent>(
        events: &[E],
        label: &str,
        build_exchange: impl Fn(&E) -> Exchange,
        seen_ids: &mut IndexSet<String>,
        capacity: usize,
        runtime: &dyn RuntimeObservability,
        context: &ConsumerContext,
        last_event_time: u64,
    ) -> BatchOutcome {
        let mut max_time = last_event_time;
        for event in events {
            let id = match event.event_id() {
                Some(id) => id,
                None => {
                    warn!("skipping {} event with no id", label);
                    continue;
                }
            };
            if !seen_ids.insert(id.clone()) {
                continue;
            }
            while seen_ids.len() > capacity {
                seen_ids.shift_remove_index(0);
            }
            let exchange = build_exchange(event);
            if context.send(exchange).await.is_err() {
                runtime
                    .metrics()
                    .increment_errors(context.route_id(), "b-prime:keycloak:response-body");
                // log-policy: outside-contract
                error!(error = "failed to send exchange, channel closed");
                return BatchOutcome::ChannelClosed;
            }
            if let Some(t) = event.event_time() {
                max_time = max_time.max(t);
            }
        }
        BatchOutcome::Updated(max_time)
    }

    fn build_user_event_exchange(event: &EventRepresentation) -> Exchange {
        let mut msg = Message::new(Body::Json(serde_json::to_value(event).unwrap_or_default()));

        set_opt(&mut msg, HEADER_EVENT_ID, event.id.as_ref());
        if let Some(time) = event.time {
            msg.set_header(HEADER_EVENT_TIME, time.to_string());
        }
        set_opt(&mut msg, HEADER_REALM_ID, event.realmId.as_ref());
        set_opt(&mut msg, HEADER_EVENT_TYPE, event.event_type.as_ref());
        set_opt(&mut msg, HEADER_CLIENT_ID, event.clientId.as_ref());
        set_opt(&mut msg, HEADER_USER_ID, event.userId.as_ref());
        set_opt(&mut msg, HEADER_SESSION_ID, event.sessionId.as_ref());
        set_opt(&mut msg, HEADER_IP_ADDRESS, event.ipAddress.as_ref());

        Exchange::new(msg)
    }

    fn build_admin_event_exchange(event: &AdminEventRepresentation) -> Exchange {
        let mut msg = Message::new(Body::Json(serde_json::to_value(event).unwrap_or_default()));

        set_opt(&mut msg, HEADER_EVENT_ID, event.id.as_ref());
        if let Some(time) = event.time {
            msg.set_header(HEADER_EVENT_TIME, time.to_string());
        }
        set_opt(&mut msg, HEADER_REALM_ID, event.realmId.as_ref());
        set_opt(&mut msg, HEADER_EVENT_TYPE, event.operationType.as_ref());
        set_opt(&mut msg, HEADER_RESOURCE_TYPE, event.resourceType.as_ref());
        set_opt(&mut msg, HEADER_RESOURCE_PATH, event.resourcePath.as_ref());
        if let Some(ref auth) = event.authDetails {
            set_opt(&mut msg, HEADER_AUTH_USER_ID, auth.userId.as_ref());
            set_opt(&mut msg, HEADER_AUTH_CLIENT_ID, auth.clientId.as_ref());
        }

        Exchange::new(msg)
    }
}

#[async_trait]
impl Consumer for KeycloakEventConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        let poll_delay = std::time::Duration::from_millis(self.config.poll_delay_ms);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut last_event_time = now.saturating_sub(self.config.lookback_window_ms);
        let mut seen_ids: IndexSet<String> = IndexSet::new();
        let capacity = self.config.dedup_capacity;
        let mut consecutive_auth_errors: u32 = 0;
        let max_auth_errors = self.config.max_auth_errors;

        info!(
            realm = %self.config.realm,
            event_type = ?self.config.event_type,
            "keycloak event consumer starting"
        );

        loop {
            tokio::select! {
                _ = context.cancelled() => {
                    info!("keycloak event consumer cancelled, stopping");
                    break;
                }
                _ = tokio::time::sleep(poll_delay) => {
                    let token = match self.config.token_provider.get_token().await {
                        Ok(t) => t,
                        Err(e) => {
                            self.runtime.metrics().increment_errors(
                                context.route_id(),
                                "e:keycloak:auth-material",
                            );
                            // log-policy: outside-contract
                            error!(error = %e, "failed to acquire event auth material, skipping poll");
                            continue;
                        }
                    };

                    let result = match self.config.event_type {
                        EventType::Events => {
                            match self.fetch_user_events(&token, last_event_time.saturating_add(1)).await {
                                Ok(events) => {
                                    match Self::process_event_batch(
                                        &events,
                                        "user",
                                        Self::build_user_event_exchange,
                                        &mut seen_ids,
                                        capacity,
                                        &*self.runtime,
                                        &context,
                                        last_event_time,
                                    )
                                    .await
                                    {
                                        BatchOutcome::Updated(t) => {
                                            last_event_time = t;
                                            consecutive_auth_errors = 0;
                                            Ok(())
                                        }
                                        BatchOutcome::ChannelClosed => return Ok(()),
                                    }
                                }
                                Err(e) => Err(e),
                            }
                        }
                        EventType::AdminEvents => {
                            match self.fetch_admin_events(&token, last_event_time.saturating_add(1)).await {
                                Ok(events) => {
                                    match Self::process_event_batch(
                                        &events,
                                        "admin",
                                        Self::build_admin_event_exchange,
                                        &mut seen_ids,
                                        capacity,
                                        &*self.runtime,
                                        &context,
                                        last_event_time,
                                    )
                                    .await
                                    {
                                        BatchOutcome::Updated(t) => {
                                            last_event_time = t;
                                            consecutive_auth_errors = 0;
                                            Ok(())
                                        }
                                        BatchOutcome::ChannelClosed => return Ok(()),
                                    }
                                }
                                Err(e) => Err(e),
                            }
                        }
                    };

                    if let Err(e) = result {
                        match e {
                            PollError::Auth(msg) => {
                                consecutive_auth_errors += 1;
                                if consecutive_auth_errors >= max_auth_errors {
                                    // log-policy: system-broken
                                    error!(
                                        errors = consecutive_auth_errors,
                                        "max consecutive auth errors reached, stopping consumer"
                                    );
                                    return Err(CamelError::ProcessorError(format!(
                                        "keycloak event consumer: {} consecutive auth failures",
                                        consecutive_auth_errors
                                    )));
                                }
                                warn!(error = %msg, errors = consecutive_auth_errors, "auth error, will retry");
                            }
                            PollError::Transient(msg) => {
                                warn!(error = %msg, "transient error, will retry next poll");
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        debug!("keycloak event consumer stopped");
        Ok(())
    }
}
