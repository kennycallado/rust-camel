mod support;

use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::rustls;

use camel_api::{Exchange, Message};
use camel_component_api::{
    BoxProcessor, Component, NoOpComponentContext, NoopRuntimeObservability, ProducerContext,
};
use camel_component_mail::{MailComponent, MailComponentConfig, MailScheme};
use support::docker::{Gate, gate, nanos};
use support::mailpit::{MailpitFixture, wait_for_subject};
use tokio::time::timeout;
use tower::Service;

#[test]
fn gate_off_prints_notice() {
    match gate(None, || true) {
        Gate::Skip(notice) => assert!(notice.contains("MAIL_ITEST=1")),
        Gate::Run => panic!("expected skip"),
    }
}

#[test]
#[should_panic(expected = "infra-unavailable")]
fn gate_on_docker_absent_fails_infra_unavailable() {
    let _ = gate(Some("1"), || false);
}

#[test]
fn gate_on_docker_present_runs() {
    assert!(matches!(gate(Some("1"), || true), Gate::Run));
}

#[tokio::test]
async fn mailpit_starttls_round_trip() {
    if !support::docker::gate_active() {
        return;
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = timeout(Duration::from_secs(120), MailpitFixture::start())
        .await
        .expect("infra-unavailable: mailpit start timeout");
    let subject = format!("itest-{}", nanos());

    let cfg: MailComponentConfig = toml::from_str(&format!(
        "[accounts.main]\nusername = \"u\"\npassword = \"p\"\ncaFile = {:?}\n",
        fixture.ca_path.display().to_string()
    ))
    .expect("account config");
    let uri = format!(
        "smtp://127.0.0.1:{}?from=bot@corp.test&to=ops@corp.test&subject={subject}",
        fixture.smtp_port
    );
    let component = MailComponent::new(MailScheme::Smtp, Arc::new(cfg));
    let endpoint = component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("endpoint");
    let mut p: BoxProcessor = endpoint
        .create_producer(
            Arc::new(NoopRuntimeObservability),
            &ProducerContext::default(),
        )
        .expect("producer");
    let out = timeout(
        Duration::from_secs(30),
        p.call(Exchange::new(Message::new("hello"))),
    )
    .await
    .expect("send timeout")
    .expect("send ok");
    let id = out
        .input
        .header("CamelMailMessageId")
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    assert!(!id.is_empty(), "message id header set");
    assert!(wait_for_subject(&fixture, &subject, Duration::from_secs(30)).await);
}
