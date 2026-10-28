mod support;

use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange, Message};
use camel_component_api::{
    BoxProcessor, Component, NoOpComponentContext, NoopRuntimeObservability, ProducerContext,
};
use camel_component_mail::{MailComponent, MailComponentConfig, MailScheme};
use support::fake_smtp::{FakeSmtpOptions, FakeSmtpServer, SmtpBehavior, SmtpTls};
use tokio::time::timeout;
use tower::Service;

const BOUND: Duration = Duration::from_secs(10);

async fn start(behavior: SmtpBehavior) -> FakeSmtpServer {
    timeout(
        BOUND,
        FakeSmtpServer::start(FakeSmtpOptions {
            behavior,
            tls: SmtpTls::None,
        }),
    )
    .await
    .expect("server start timeout")
}

async fn producer_for(uri: &str, config: MailComponentConfig) -> BoxProcessor {
    let component = MailComponent::new(MailScheme::Smtp, Arc::new(config));
    let endpoint = component
        .create_endpoint(uri, &NoOpComponentContext)
        .expect("endpoint");
    endpoint
        .create_producer(
            Arc::new(NoopRuntimeObservability),
            &ProducerContext::default(),
        )
        .expect("producer")
}

async fn send(p: &mut BoxProcessor, ex: Exchange) -> Result<Exchange, CamelError> {
    timeout(BOUND, p.call(ex)).await.expect("send timeout")
}

fn uri(port: u16, extra: &str) -> String {
    format!("smtp://127.0.0.1:{port}?from=bot@corp.test&to=a@corp.test&allowCleartext=true{extra}")
}

fn ex(body: &str) -> Exchange {
    Exchange::new(Message::new(body))
}

async fn run(extra: &str, ex: Exchange) -> (FakeSmtpServer, Result<Exchange, CamelError>) {
    let server = start(SmtpBehavior::Accept).await;
    let mut p = producer_for(&uri(server.port, extra), MailComponentConfig::default()).await;
    let r = send(&mut p, ex).await;
    (server, r)
}

#[tokio::test]
async fn text_body_received() {
    let (server, r) = run("", ex("hello")).await;
    r.expect("sent");
    let rec = server.recording();
    assert_eq!(rec.messages.len(), 1);
    let m = &rec.messages[0];
    let ct = m.header("Content-Type").expect("ct").to_ascii_lowercase();
    assert!(ct.starts_with("text/plain"), "{ct}");
    assert!(ct.contains("charset=utf-8"), "{ct}");
    assert!(m.data.contains("hello"));
}

#[tokio::test]
async fn explicit_cleartext_opt_in_succeeds() {
    let (server, r) = run("", ex("hello")).await;
    r.expect("sent");
    let rec = server.recording();
    assert!(
        !rec.commands
            .iter()
            .any(|c| c.to_ascii_uppercase().contains("STARTTLS"))
    );
}

#[tokio::test]
async fn html_body_received() {
    let (server, r) = run("&contentType=text/html", ex("<b>x</b>")).await;
    r.expect("sent");
    let rec = server.recording();
    let ct = rec.messages[0].header("Content-Type").expect("ct");
    assert!(ct.starts_with("text/html"), "{ct}");
}

#[tokio::test]
async fn alternative_body_received() {
    let mut e = ex("plain");
    e.input.set_header("CamelMailHtmlBody", "<b>x</b>");
    let (server, r) = run("", e).await;
    r.expect("sent");
    let rec = server.recording();
    let m = &rec.messages[0];
    let ct = m.header("Content-Type").expect("ct");
    assert!(ct.starts_with("multipart/alternative"), "{ct}");
    assert!(m.data.contains("text/plain"));
    assert!(m.data.contains("text/html"));
}

#[tokio::test]
async fn message_id_matches_wire() {
    let (server, r) = run("", ex("hello")).await;
    let out = r.expect("sent");
    let id = out
        .input
        .header("CamelMailMessageId")
        .and_then(|v| v.as_str().map(str::to_string))
        .expect("id header");
    let rec = server.recording();
    assert_eq!(rec.messages[0].header("Message-ID"), Some(id));
}

#[tokio::test]
async fn subject_header_overrides_option_on_wire() {
    let mut e = ex("hello");
    e.input.set_header("CamelMailSubject", "b");
    let (server, r) = run("&subject=a", e).await;
    r.expect("sent");
    assert_eq!(
        server.recording().messages[0].header("Subject"),
        Some("b".to_string())
    );
}

#[tokio::test]
async fn spoofed_from_header_ignored_on_wire() {
    let mut e = ex("hello");
    e.input.set_header("From", "ceo@corp.test");
    let (server, r) = run("", e).await;
    r.expect("sent");
    let rec = server.recording();
    let m = &rec.messages[0];
    let from = m.header("From").expect("from");
    assert!(from.contains("bot@corp.test"), "{from}");
    assert!(!from.contains("ceo@corp.test"), "{from}");
    assert_eq!(m.mail_from, "bot@corp.test");
}

#[tokio::test]
async fn x_header_emitted_on_wire() {
    let mut e = ex("hello");
    e.input.set_header("X-Trace", "abc");
    let (server, r) = run("", e).await;
    r.expect("sent");
    assert_eq!(
        server.recording().messages[0].header("X-Trace"),
        Some("abc".to_string())
    );
}

#[tokio::test]
async fn header_recipients_replace_endpoint_recipients_on_wire() {
    let mut e = ex("hello");
    e.input.set_header("CamelMailTo", "b@corp.test");
    let (server, r) = run("&allowedRecipients=@corp.test", e).await;
    r.expect("sent");
    assert_eq!(
        server.recording().messages[0].rcpt_to,
        vec!["b@corp.test".to_string()]
    );
}

#[tokio::test]
async fn bcc_envelope_only_on_wire() {
    let mut e = ex("hello");
    e.input.set_header("CamelMailBcc", "zbcc@corp.test");
    let (server, r) = run("&allowedRecipients=@corp.test", e).await;
    r.expect("sent");
    let rec = server.recording();
    let m = &rec.messages[0];
    assert!(m.rcpt_to.contains(&"zbcc@corp.test".to_string()));
    assert!(!m.data.contains("zbcc@corp.test"));
    assert!(!m.data.contains("Bcc:"));
}

#[tokio::test]
async fn title_case_to_has_no_effect_on_wire() {
    let server = start(SmtpBehavior::Accept).await;
    let u = format!(
        "smtp://127.0.0.1:{}?from=bot@corp.test&to=own@corp.test&allowCleartext=true",
        server.port
    );
    let mut p = producer_for(&u, MailComponentConfig::default()).await;
    let mut e = ex("hello");
    e.input.set_header("To", "a@x.test");
    send(&mut p, e).await.expect("sent");
    assert_eq!(
        server.recording().messages[0].rcpt_to,
        vec!["own@corp.test".to_string()]
    );
}

#[tokio::test]
async fn crlf_injection_opens_no_connection() {
    let mut e = ex("hello");
    e.input
        .set_header("CamelMailSubject", "hi\r\nBcc: evil@x.test");
    let (server, r) = run("", e).await;
    let err = r.expect_err("must fail");
    assert!(err.to_string().contains("CamelMailSubject"), "{err}");
    assert_eq!(server.recording().connections, 0);
}

#[tokio::test]
async fn json_body_opens_no_connection() {
    let e = Exchange::new(Message::new(Body::Json(serde_json::json!({"a": 1}))));
    let (server, r) = run("", e).await;
    assert!(r.is_err());
    assert_eq!(server.recording().connections, 0);
}

#[tokio::test]
async fn failure_after_data_not_resent() {
    let server = start(SmtpBehavior::DropAfterData).await;
    let mut p = producer_for(&uri(server.port, ""), MailComponentConfig::default()).await;
    assert!(send(&mut p, ex("hello")).await.is_err());
    let rec = server.recording();
    assert_eq!(rec.data_commands, 1);
    assert_eq!(rec.connections, 1);
}

#[tokio::test]
async fn failure_before_data_is_plain_error() {
    let server = start(SmtpBehavior::RejectRcpt550).await;
    let mut p = producer_for(&uri(server.port, ""), MailComponentConfig::default()).await;
    let err = send(&mut p, ex("hello")).await.expect_err("must fail");
    assert!(err.to_string().contains("550"), "{err}");
    assert_eq!(server.recording().data_commands, 0);
}

/// Send `e` through an endpoint with `extra` options and assert the
/// exchange fails before the fake server sees a connection or a DATA.
async fn assert_nothing_sent(extra: &str, e: Exchange) {
    let (server, r) = run(extra, e).await;
    assert!(r.is_err(), "{extra}: the exchange must fail");
    let rec = server.recording();
    assert_eq!(rec.connections, 0, "{extra}: no connection");
    assert_eq!(rec.data_commands, 0, "{extra}: no DATA");
}

#[tokio::test]
async fn nul_in_x_header_opens_no_connection() {
    let mut e = ex("hello");
    e.input.set_header("X-Trace", "a\0b");
    assert_nothing_sent("", e).await;
}

#[tokio::test]
async fn over_long_subject_opens_no_connection() {
    let mut e = ex("hello");
    e.input.set_header("CamelMailSubject", "s".repeat(999));
    assert_nothing_sent("", e).await;
}

#[tokio::test]
async fn recipient_cap_opens_no_connection() {
    let mut e = ex("hello");
    e.input
        .set_header("CamelMailTo", "a@corp.test,b@corp.test,c@corp.test");
    assert_nothing_sent("&allowedRecipients=@corp.test&maxRecipients=2", e).await;
}

#[tokio::test]
async fn routing_header_on_unarmed_endpoint_opens_no_connection() {
    let mut e = ex("hello");
    e.input.set_header("CamelMailTo", "a@x.test");
    assert_nothing_sent("", e).await;
}

#[tokio::test]
async fn address_outside_allowlist_opens_no_connection() {
    let mut e = ex("hello");
    e.input.set_header("CamelMailTo", "a@evil.test");
    assert_nothing_sent("&allowedRecipients=@corp.test", e).await;
}

#[tokio::test]
async fn allowed_domain_matches_case_insensitively_on_wire() {
    let mut e = ex("hello");
    e.input.set_header("CamelMailTo", "a@corp.test");
    let (server, r) = run("&allowedRecipients=@Corp.Test", e).await;
    r.expect("sent");
    let rec = server.recording();
    assert_eq!(rec.data_commands, 1);
    assert_eq!(rec.messages[0].rcpt_to, vec!["a@corp.test".to_string()]);
}
