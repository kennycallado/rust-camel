//! Broker-gated AMQPS e2e tests.
//!
//! Gate semantics mirror `tests/common/mod.rs::gate`: an unset
//! `RABBITMQ_TLS_ITEST` prints a skip notice and returns (exit 0); an
//! ACTIVATED gate with missing required environment panics
//! `infra-unavailable` (never a silent skip). No Docker fixture and no CI
//! registration: a TLS broker is operator-provided.
//!
//! Required when the gate is active:
//! - `RABBITMQ_TLS_URL` — `amqps://` broker URL trusted by `RABBITMQ_TLS_CA`
//! - `RABBITMQ_TLS_CA` — PEM path of the CA that signed the broker certificate
//!
//! Optional: `RABBITMQ_TLS_MISMATCH_URL` — the same broker reached through a
//! host that is NOT in its certificate. Its absence skips only the hostname
//! test (printed notice), never the other two.

use std::sync::Arc;
use std::time::Duration;

use camel_component_api::NetworkRetryPolicy;
use camel_component_rabbitmq::config::RabbitBrokerConfig;
use camel_component_rabbitmq::connection::RabbitConnectionManager;
use camel_component_rabbitmq::tls::RabbitTlsConfig;
use tokio::time;

/// Bound on one broker connection attempt (includes TLS + AMQP handshake).
const BOUND: Duration = Duration::from_secs(15);

const SKIP_NOTICE: &str = "RabbitMQ TLS e2e tests skipped: RABBITMQ_TLS_ITEST is not set; \
     run with RABBITMQ_TLS_ITEST=1 RABBITMQ_TLS_URL=... RABBITMQ_TLS_CA=...";

/// Resolved gate decision. Pure: no I/O, nothing printed.
#[derive(Debug, PartialEq, Eq)]
enum Gate {
    /// `RABBITMQ_TLS_ITEST` unset: no test runs.
    Skip,
    /// Gate active with every mandatory variable present.
    Run {
        url: String,
        ca: String,
        mismatch_url: Option<String>,
    },
}

/// Pure gate decision: an unset gate skips; an ACTIVATED gate requires
/// `RABBITMQ_TLS_URL` and `RABBITMQ_TLS_CA` and panics `infra-unavailable`
/// naming every missing one. `RABBITMQ_TLS_MISMATCH_URL` stays optional: its
/// absence is carried as `None` so only the hostname test skips.
fn resolve_env(
    itest: Option<&str>,
    url: Option<&str>,
    ca: Option<&str>,
    mismatch_url: Option<&str>,
) -> Gate {
    if itest.is_none() {
        return Gate::Skip;
    }
    let (Some(url), Some(ca)) = (url, ca) else {
        let mut missing = Vec::new();
        if url.is_none() {
            missing.push("RABBITMQ_TLS_URL");
        }
        if ca.is_none() {
            missing.push("RABBITMQ_TLS_CA");
        }
        panic!(
            "infra-unavailable: RABBITMQ_TLS_ITEST is active but required environment is \
             not set: {}",
            missing.join(", ")
        );
    };
    Gate::Run {
        url: url.to_string(),
        ca: ca.to_string(),
        mismatch_url: mismatch_url.map(str::to_string),
    }
}

/// Read the gate variables, apply [`resolve_env`], and print the skip notice
/// when the gate is unset.
fn gate() -> Gate {
    let itest = std::env::var("RABBITMQ_TLS_ITEST").ok();
    let url = std::env::var("RABBITMQ_TLS_URL").ok();
    let ca = std::env::var("RABBITMQ_TLS_CA").ok();
    let mismatch_url = std::env::var("RABBITMQ_TLS_MISMATCH_URL").ok();
    let gate = resolve_env(
        itest.as_deref(),
        url.as_deref(),
        ca.as_deref(),
        mismatch_url.as_deref(),
    );
    if gate == Gate::Skip {
        eprintln!("{SKIP_NOTICE}");
    }
    gate
}

/// One bounded attempt: no retry loop, so a rejection surfaces immediately.
fn single_attempt_retry() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        enabled: true,
        max_attempts: 1,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_millis(1),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    }
}

fn broker(url: String, ca_cert: String) -> RabbitBrokerConfig {
    RabbitBrokerConfig {
        url,
        username: None,
        password: None,
        vhost: None,
        tls: Some(RabbitTlsConfig {
            ca_cert: Some(ca_cert),
            server_name: None,
        }),
    }
}

fn manager_for(broker: &RabbitBrokerConfig) -> Arc<RabbitConnectionManager> {
    Arc::new(
        RabbitConnectionManager::try_from_broker_config("tls-e2e", broker, single_attempt_retry())
            .expect("the e2e broker config is valid"),
    )
}

fn wrong_ca_path() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tls/wrong-ca.pem")
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn amqps_connects_with_custom_ca() {
    let Gate::Run { url, ca, .. } = gate() else {
        return;
    };
    let manager = manager_for(&broker(url, ca));

    let connected = time::timeout(BOUND, manager.connection_within(BOUND))
        .await
        .expect("the AMQPS connect settles within the bound");
    let (connection, _generation) =
        connected.expect("configured test CA must trust the broker certificate");

    let channel = time::timeout(BOUND, connection.create_channel())
        .await
        .expect("channel.open settles within the bound")
        .expect("channel.open succeeds on a live AMQP connection");
    drop(channel);
    drop(connection);
}

#[tokio::test]
async fn amqps_rejects_untrusted_ca() {
    let Gate::Run { url, .. } = gate() else {
        return;
    };
    let manager = manager_for(&broker(url, wrong_ca_path()));

    let outcome = time::timeout(BOUND, manager.connection_within(BOUND))
        .await
        .expect("the rejected connect settles within the bound");
    assert!(
        outcome.is_err(),
        "a broker certificate signed by an unrelated CA must be rejected"
    );
}

#[tokio::test]
async fn amqps_rejects_hostname_mismatch() {
    let Gate::Run {
        ca, mismatch_url, ..
    } = gate()
    else {
        return;
    };
    let Some(url) = mismatch_url else {
        eprintln!(
            "RabbitMQ TLS hostname-mismatch test skipped: RABBITMQ_TLS_MISMATCH_URL is not set"
        );
        return;
    };
    let manager = manager_for(&broker(url, ca));

    let outcome = time::timeout(BOUND, manager.connection_within(BOUND))
        .await
        .expect("the hostname-mismatch connect settles within the bound");
    assert!(
        outcome.is_err(),
        "a broker reached through a host absent from its certificate must be rejected"
    );
}

#[cfg(test)]
mod gate_tests {
    use super::{Gate, resolve_env};

    const URL: &str = "amqps://broker.test:5671/";
    const CA: &str = "/tmp/test-ca.pem";

    #[test]
    fn unset_gate_skips_regardless_of_the_other_variables() {
        assert_eq!(resolve_env(None, None, None, None), Gate::Skip);
        assert_eq!(
            resolve_env(None, Some(URL), Some(CA), Some("amqps://other.test/")),
            Gate::Skip
        );
    }

    #[test]
    fn activated_gate_with_both_mandatory_variables_runs() {
        assert_eq!(
            resolve_env(Some("1"), Some(URL), Some(CA), None),
            Gate::Run {
                url: URL.to_string(),
                ca: CA.to_string(),
                mismatch_url: None,
            }
        );
    }

    #[test]
    fn activated_gate_carries_the_optional_mismatch_url() {
        assert_eq!(
            resolve_env(Some("1"), Some(URL), Some(CA), Some("amqps://other.test/")),
            Gate::Run {
                url: URL.to_string(),
                ca: CA.to_string(),
                mismatch_url: Some("amqps://other.test/".to_string()),
            }
        );
    }

    #[test]
    #[should_panic(
        expected = "infra-unavailable: RABBITMQ_TLS_ITEST is active but required environment is not set: RABBITMQ_TLS_URL"
    )]
    fn activated_gate_without_url_panics() {
        let _ = resolve_env(Some("1"), None, Some(CA), None);
    }

    #[test]
    #[should_panic(
        expected = "infra-unavailable: RABBITMQ_TLS_ITEST is active but required environment is not set: RABBITMQ_TLS_CA"
    )]
    fn activated_gate_without_ca_panics() {
        let _ = resolve_env(Some("1"), Some(URL), None, None);
    }

    #[test]
    #[should_panic(
        expected = "infra-unavailable: RABBITMQ_TLS_ITEST is active but required environment is not set: RABBITMQ_TLS_URL, RABBITMQ_TLS_CA"
    )]
    fn activated_gate_without_both_names_both() {
        let _ = resolve_env(Some("1"), None, None, None);
    }
}
