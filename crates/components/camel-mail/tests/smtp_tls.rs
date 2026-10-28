mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::rustls;

use camel_api::{CamelError, Exchange, Message};
use camel_component_api::{
    BoxProcessor, Component, NoOpComponentContext, NoopRuntimeObservability, ProducerContext,
};
use camel_component_mail::{MailComponent, MailComponentConfig, MailScheme};
use support::fake_smtp::{FakeSmtpOptions, FakeSmtpServer, SmtpBehavior, SmtpTls};
use support::tls_fixture::tls_fixture;
use tokio::time::timeout;
use tower::Service;

const BOUND: Duration = Duration::from_secs(10);

fn account(ca: Option<&Path>, login: bool) -> MailComponentConfig {
    let mut s = String::from("[accounts.main]\n");
    if login {
        s.push_str("username = \"u\"\npassword = \"hunter2\"\n");
    }
    if let Some(p) = ca {
        s.push_str(&format!("caFile = {:?}\n", p.display().to_string()));
    }
    toml::from_str(&s).expect("account config")
}

async fn start(tls: SmtpTls) -> FakeSmtpServer {
    timeout(
        BOUND,
        FakeSmtpServer::start(FakeSmtpOptions {
            behavior: SmtpBehavior::Accept,
            tls,
        }),
    )
    .await
    .expect("server start timeout")
}

async fn send(
    scheme: &str,
    port: u16,
    config: MailComponentConfig,
) -> Result<Exchange, CamelError> {
    // lettre uses the process-default rustls provider; pin ring in tests
    let _ = rustls::crypto::ring::default_provider().install_default();
    let uri = format!("{scheme}://127.0.0.1:{port}?from=bot@corp.test&to=a@corp.test");
    let mail_scheme = match scheme {
        "smtp" => MailScheme::Smtp,
        "smtps" => MailScheme::Smtps,
        other => panic!("not an SMTP scheme: {other}"),
    };
    let component = MailComponent::new(mail_scheme, Arc::new(config));
    let endpoint = component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("endpoint");
    let mut p: BoxProcessor = endpoint
        .create_producer(
            Arc::new(NoopRuntimeObservability),
            &ProducerContext::default(),
        )
        .expect("producer");
    timeout(BOUND, p.call(Exchange::new(Message::new("hello"))))
        .await
        .expect("send timeout")
}

#[tokio::test]
async fn smtp_without_starttls_refused_before_auth() {
    let server = start(SmtpTls::None).await;
    let r = send("smtp", server.port, account(None, true)).await;
    assert!(r.is_err());
    let rec = server.recording();
    assert!(!rec.commands.iter().any(|c| c.starts_with("AUTH")));
    assert!(!rec.commands.iter().any(|c| c.starts_with("MAIL FROM")));
    // base64("hunter2")
    assert!(
        !rec.commands
            .iter()
            .any(|c| c.contains("hunter2") || c.contains("aHVudGVyMg=="))
    );
}

#[tokio::test]
async fn starttls_with_private_ca_succeeds() {
    let fx = tls_fixture();
    let server = start(SmtpTls::StartTls(fx.server_config.clone())).await;
    send("smtp", server.port, account(Some(&fx.ca_path), true))
        .await
        .expect("sent");
    let rec = server.recording();
    let st = rec
        .commands
        .iter()
        .position(|c| c.eq_ignore_ascii_case("STARTTLS"))
        .expect("STARTTLS seen");
    assert!(rec.commands[st..].iter().any(|c| c.starts_with("AUTH")));
    assert_eq!(rec.messages.len(), 1);
}

#[tokio::test]
async fn starttls_without_ca_fails_verification() {
    let fx = tls_fixture();
    let server = start(SmtpTls::StartTls(fx.server_config.clone())).await;
    let e = send("smtp", server.port, account(None, true))
        .await
        .expect_err("must fail");
    assert!(e.to_string().to_lowercase().contains("certificate"), "{e}");
    assert!(server.recording().messages.is_empty());
}

#[tokio::test]
async fn implicit_tls_succeeds_without_starttls() {
    let fx = tls_fixture();
    let server = start(SmtpTls::Implicit(fx.server_config.clone())).await;
    send("smtps", server.port, account(Some(&fx.ca_path), true))
        .await
        .expect("sent");
    let rec = server.recording();
    assert!(
        !rec.commands
            .iter()
            .any(|c| c.eq_ignore_ascii_case("STARTTLS"))
    );
    assert_eq!(rec.messages.len(), 1);
}
