#![allow(dead_code)]

use std::{collections::HashMap, sync::Arc};

use camel_component_jms::{BrokerConfig, BrokerType, JmsBridgePool, JmsComponent, JmsPoolConfig};
use tokio::sync::OnceCell;

use super::activemq::shared_activemq;
use super::artemis::{shared_artemis, shared_artemis_auth};
use super::bridge_bg_rt;

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
            let pool_config = JmsPoolConfig {
                max_bridges: 1,
                brokers: HashMap::from([(
                    "default".to_string(),
                    BrokerConfig {
                        broker_url: broker_url.to_string(),
                        broker_type: BrokerType::ActiveMq,
                        username: Some("admin".to_string()),
                        password: Some("admin".to_string()),
                    },
                )]),
                bridge_start_timeout_ms: 90_000,
                ..JmsPoolConfig::default()
            };

            let pool = Arc::new(JmsBridgePool::from_config(pool_config).unwrap());
            // Eagerly start the bridge slot on the permanent background runtime so
            // the tonic Channel dispatch task and health monitor outlive all
            // individual test runtimes.
            let pool_init = pool.clone();
            let _ = bridge_bg_rt()
                .spawn(async move { pool_init.get_or_create_slot("default").await })
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
            let pool_config = JmsPoolConfig {
                max_bridges: 1,
                brokers: HashMap::from([(
                    "default".to_string(),
                    BrokerConfig {
                        broker_url: broker_url.to_string(),
                        broker_type: BrokerType::Artemis,
                        username: None,
                        password: None,
                    },
                )]),
                bridge_start_timeout_ms: 90_000,
                ..JmsPoolConfig::default()
            };

            let pool = Arc::new(JmsBridgePool::from_config(pool_config).unwrap());
            // Eagerly start the bridge slot on the permanent background runtime.
            let pool_init = pool.clone();
            let _ = bridge_bg_rt()
                .spawn(async move { pool_init.get_or_create_slot("default").await })
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
            let pool_config = JmsPoolConfig {
                max_bridges: 1,
                brokers: HashMap::from([(
                    "default".to_string(),
                    BrokerConfig {
                        broker_url: broker_url.to_string(),
                        broker_type: BrokerType::Artemis,
                        username: Some("artemis".to_string()),
                        password: Some("artemis".to_string()),
                    },
                )]),
                bridge_start_timeout_ms: 90_000,
                ..JmsPoolConfig::default()
            };

            let pool = Arc::new(JmsBridgePool::from_config(pool_config).unwrap());
            // Eagerly start the bridge slot on the permanent background runtime.
            let pool_init = pool.clone();
            let _ = bridge_bg_rt()
                .spawn(async move { pool_init.get_or_create_slot("default").await })
                .await;
            JmsComponent::with_scheme("jms", pool)
        })
        .await
        .clone()
}
