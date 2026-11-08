#![allow(dead_code)]

use std::{collections::HashMap, sync::Arc};

use camel_component_jms::{BrokerConfig, BrokerType, JmsBridgePool, JmsComponent, JmsPoolConfig};
use tokio::sync::OnceCell;

use super::activemq::{ACTIVEMQ_PASSWORD, ACTIVEMQ_USER, ActiveMqBroker, shared_activemq};
use super::artemis::{shared_artemis, shared_artemis_auth};
use super::bridge_bg_rt;

/// Builds the single-broker pool config every JMS fixture in this module
/// uses: one broker keyed `"default"`, one bridge, no TLS, a 90 s bridge
/// start budget, everything else at the [`JmsPoolConfig`] default. The
/// fixtures used to carry this literal inline; the builder keeps the
/// values byte-identical while giving each fixture its own URL, broker
/// type, and credentials. (`JmsPoolConfig::single_broker` does not fit:
/// it hardcodes `username/password: None` and the 30 s default start
/// timeout.)
fn pool_config(
    broker_url: String,
    broker_type: BrokerType,
    username: Option<&str>,
    password: Option<&str>,
    bridge_start_timeout_ms: u64,
) -> JmsPoolConfig {
    JmsPoolConfig {
        max_bridges: 1,
        brokers: HashMap::from([(
            "default".to_string(),
            BrokerConfig {
                broker_url,
                broker_type,
                username: username.map(str::to_string),
                password: password.map(str::to_string),
                tls: None,
            },
        )]),
        bridge_start_timeout_ms,
        ..JmsPoolConfig::default()
    }
}

/// Shared JmsComponent backed by ActiveMQ Classic.
/// Cloning this component shares the same underlying bridge process, so
/// parallel tests do not each spawn their own native bridge binary.
static JMS_ACTIVEMQ: OnceCell<JmsComponent> = OnceCell::const_new();

/// Shared JmsComponent backed by Artemis (ANONYMOUS_LOGIN=true).
static JMS_ARTEMIS: OnceCell<JmsComponent> = OnceCell::const_new();

/// Shared JmsComponent backed by Artemis with mandatory auth.
static JMS_ARTEMIS_AUTH: OnceCell<JmsComponent> = OnceCell::const_new();

/// Returns a clone of the shared ActiveMQ-backed JmsComponent.
///
/// All callers share the same bridge process. The bridge slot is created
/// eagerly on the permanent background runtime so that the tonic Channel
/// dispatch task and health monitor are never killed when a test runtime
/// is dropped between tests (prevents DispatchGone errors).
pub async fn shared_jms_activemq() -> JmsComponent {
    JMS_ACTIVEMQ
        .get_or_init(|| async {
            let (_, broker_url) = shared_activemq().await;
            let pool_config = pool_config(
                broker_url.to_string(),
                BrokerType::ActiveMq,
                Some("admin"),
                Some("admin"),
                90_000,
            );

            let pool = Arc::new(JmsBridgePool::from_config(pool_config).unwrap());
            // Eagerly start the bridge slot on the permanent background runtime so
            // the tonic Channel dispatch task and health monitor outlive all
            // individual test runtimes.
            let pool_init = pool.clone();
            let _ = bridge_bg_rt()
                .spawn(async move { pool_init.get_or_create_slot("default", false, false).await })
                .await;
            JmsComponent::with_scheme("jms", pool)
        })
        .await
        .clone()
}

/// Returns a clone of the shared Artemis-backed JmsComponent (anonymous login).
pub async fn shared_jms_artemis() -> JmsComponent {
    JMS_ARTEMIS
        .get_or_init(|| async {
            let (_, broker_url) = shared_artemis().await;
            let pool_config = pool_config(
                broker_url.to_string(),
                BrokerType::Artemis,
                None,
                None,
                90_000,
            );

            let pool = Arc::new(JmsBridgePool::from_config(pool_config).unwrap());
            // Eagerly start the bridge slot on the permanent background runtime.
            let pool_init = pool.clone();
            let _ = bridge_bg_rt()
                .spawn(async move { pool_init.get_or_create_slot("default", false, false).await })
                .await;
            JmsComponent::with_scheme("jms", pool)
        })
        .await
        .clone()
}

/// Returns a clone of the shared Artemis-backed JmsComponent (mandatory auth).
pub async fn shared_jms_artemis_auth() -> JmsComponent {
    JMS_ARTEMIS_AUTH
        .get_or_init(|| async {
            let (_, broker_url) = shared_artemis_auth().await;
            let pool_config = pool_config(
                broker_url.to_string(),
                BrokerType::Artemis,
                Some("artemis"),
                Some("artemis"),
                90_000,
            );

            let pool = Arc::new(JmsBridgePool::from_config(pool_config).unwrap());
            // Eagerly start the bridge slot on the permanent background runtime.
            let pool_init = pool.clone();
            let _ = bridge_bg_rt()
                .spawn(async move { pool_init.get_or_create_slot("default", false, false).await })
                .await;
            JmsComponent::with_scheme("jms", pool)
        })
        .await
        .clone()
}

/// A fresh JmsComponent bound to a dedicated ActiveMQ Classic broker (one
/// bridge process, no process-wide sharing).
///
/// Tests that assert broker-side state through the broker's Jolokia console —
/// exact `ConsumerCount`/`EnqueueCount` readings — use a dedicated broker (see
/// the isolation note on [`ActiveMqBroker`]) and this constructor instead of
/// [`shared_jms_activemq`], whose shared broker carries every other test's
/// connections and exposes no console. The slot is eagerly started on the
/// permanent background runtime, mirroring [`shared_jms_activemq`], so the
/// tonic Channel dispatch task and health monitor outlive the test runtime.
pub async fn jms_component_for_broker(broker: &ActiveMqBroker) -> JmsComponent {
    let pool_config = pool_config(
        broker.broker_url.clone(),
        BrokerType::ActiveMq,
        Some(ACTIVEMQ_USER),
        Some(ACTIVEMQ_PASSWORD),
        90_000,
    );

    let pool = Arc::new(JmsBridgePool::from_config(pool_config).unwrap());
    let pool_init = pool.clone();
    let _ = bridge_bg_rt()
        .spawn(async move { pool_init.get_or_create_slot("default", false, false).await })
        .await; // allow-test-wait: bounded internally by bridge_start_timeout_ms (start_bridge_process total_timeout), mirroring shared_jms_*
    JmsComponent::with_scheme("jms", pool)
}
