mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::rustls;

use camel_component_api::{
    Component, ConsumerContext, ExchangeEnvelope, NoOpComponentContext, NoopRuntimeObservability,
};
use camel_component_mail::MailAccountConfig;
use camel_component_mail::MailError;
use camel_component_mail::testing::{ProbeResult, imap_connect_probe, imap_mutation_probe};
use camel_component_mail::{MailComponent, MailComponentConfig, MailScheme};
use support::fake_imap::{FakeImapServer, ImapTls, Login, PROBE_EML};
use support::tls_fixture::tls_fixture;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const BOUND: Duration = Duration::from_secs(10);

fn account(ca: Option<&Path>) -> MailAccountConfig {
    let mut s = String::from("username = \"u\"\npassword = \"hunter2\"\n");
    if let Some(p) = ca {
        s.push_str(&format!("caFile = {:?}\n", p.display().to_string()));
    }
    toml::from_str(&s).expect("account config")
}

async fn start(tls: ImapTls, caps: &str) -> FakeImapServer {
    timeout(BOUND, FakeImapServer::start(tls, caps, PROBE_EML.to_vec()))
        .await
        .expect("server start timeout")
}

async fn probe(uri: &str, account: MailAccountConfig) -> Result<ProbeResult, MailError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    timeout(BOUND, imap_connect_probe(uri, account))
        .await
        .expect("probe timeout")
}

#[tokio::test]
async fn imap_without_starttls_refused_before_login() {
    let server = start(ImapTls::None, "AUTH=PLAIN").await;
    let r = probe(&format!("imap://127.0.0.1:{}", server.port), account(None)).await;
    assert!(r.is_err());
    let lines = server.lines();
    assert!(!lines.iter().any(|l| l.contains(" LOGIN ")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("hunter2")), "{lines:?}");
}

#[tokio::test]
async fn imaps_implicit_tls_round_trip() {
    let fx = tls_fixture();
    let server = start(ImapTls::Implicit(fx.server_config.clone()), "AUTH=PLAIN").await;
    let r = probe(
        &format!("imaps://127.0.0.1:{}", server.port),
        account(Some(&fx.ca_path)),
    )
    .await
    .expect("probe");
    assert_eq!(r.uids, vec![42]);
    assert_eq!(r.sizes, vec![(42, PROBE_EML.len() as u64)]);
    assert_eq!(r.first_body_len, PROBE_EML.len());
    let lines = server.lines();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("UID STORE 42 +FLAGS (\\Seen)")),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|l| l.contains("STARTTLS")), "{lines:?}");
}

#[tokio::test]
async fn imap_starttls_with_private_ca() {
    let fx = tls_fixture();
    let server = start(
        ImapTls::StartTls(fx.server_config.clone()),
        "STARTTLS AUTH=PLAIN",
    )
    .await;
    let r = probe(
        &format!("imap://127.0.0.1:{}", server.port),
        account(Some(&fx.ca_path)),
    )
    .await
    .expect("probe");
    assert_eq!(r.uids, vec![42]);
    let lines = server.lines();
    let st = lines.iter().position(|l| l.ends_with(" STARTTLS"));
    let login = lines.iter().position(|l| l.contains(" LOGIN "));
    assert!(st.is_some() && login.is_some() && st < login, "{lines:?}");
}

#[tokio::test]
async fn imap_cleartext_opt_in() {
    let server = start(ImapTls::None, "AUTH=PLAIN").await;
    let r = probe(
        &format!("imap://127.0.0.1:{}?allowCleartext=true", server.port),
        account(None),
    )
    .await
    .expect("probe");
    assert_eq!(r.uids, vec![42]);
}

#[tokio::test]
async fn imap_fetch_uses_size_then_peek() {
    let server = start(ImapTls::None, "AUTH=PLAIN").await;
    let r = probe(
        &format!("imap://127.0.0.1:{}?allowCleartext=true", server.port),
        account(None),
    )
    .await
    .expect("probe");
    assert_eq!(r.first_body_len, PROBE_EML.len());
    let lines = server.lines();
    let size = lines
        .iter()
        .position(|l| l.contains("UID FETCH 42") && l.contains("RFC822.SIZE"));
    let peek = lines
        .iter()
        .position(|l| l.contains("UID FETCH 42") && l.contains("BODY.PEEK[]"));
    assert!(size.is_some() && peek.is_some() && size < peek, "{lines:?}");
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("BODY[]") && !l.contains("PEEK")),
        "{lines:?}"
    );
}

async fn mutate(server: &FakeImapServer, op: &str, folder: &str) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let uri = format!("imap://127.0.0.1:{}?allowCleartext=true", server.port);
    timeout(BOUND, imap_mutation_probe(&uri, account(None), op, folder))
        .await
        .expect("mutation probe timeout")
        .expect("mutation probe");
}

#[tokio::test]
async fn adapter_delete_uses_uid_expunge() {
    let server = start(ImapTls::None, "AUTH=PLAIN UIDPLUS").await;
    mutate(&server, "delete", "").await;
    let lines = server.lines();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("UID STORE 42 +FLAGS.SILENT (\\Deleted)")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("UID EXPUNGE 42")),
        "{lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.split(' ').nth(1) == Some("EXPUNGE") && l.split(' ').count() == 2),
        "{lines:?}"
    );
}

#[tokio::test]
async fn adapter_move_uses_uid_move() {
    let server = start(ImapTls::None, "AUTH=PLAIN MOVE").await;
    mutate(&server, "move", "Done").await;
    let lines = server.lines();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("UID MOVE 42") && l.contains("Done")),
        "{lines:?}"
    );
}

#[tokio::test]
async fn adapter_copy_uses_uid_copy() {
    let server = start(ImapTls::None, "AUTH=PLAIN").await;
    mutate(&server, "copy", "Archive").await;
    let lines = server.lines();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("UID COPY 42") && l.contains("Archive")),
        "{lines:?}"
    );
}

fn login_lines(server: &FakeImapServer) -> usize {
    server
        .lines()
        .iter()
        .filter(|l| l.to_ascii_uppercase().contains(" LOGIN "))
        .count()
}

async fn refusing_server() -> FakeImapServer {
    timeout(
        BOUND,
        FakeImapServer::start_with(
            ImapTls::None,
            "AUTH=PLAIN",
            PROBE_EML.to_vec(),
            Login::Refuse,
        ),
    )
    .await
    .expect("server start timeout")
}

#[tokio::test]
async fn imap_refused_login_is_permanent() {
    let server = refusing_server().await;
    let r = probe(
        &format!("imap://127.0.0.1:{}?allowCleartext=true", server.port),
        account(None),
    )
    .await;
    let err = r.expect_err("login must be refused");
    assert!(matches!(err, MailError::LoginRefused), "{err:?}");
    assert!(!err.is_retryable(), "{err}");
    let text = err.to_string();
    assert!(
        !text.contains("hunter2") && !text.contains("invalid credentials"),
        "{text}"
    );
    assert_eq!(login_lines(&server), 1);
}

#[tokio::test]
async fn consumer_stops_on_refused_login() {
    let (_guard, logs) = support::capture_logs();
    let server = refusing_server().await;
    let cfg: MailComponentConfig =
        toml::from_str("[accounts.main]\nusername = \"u\"\npassword = \"hunter2\"\n")
            .expect("account config");
    let endpoint = MailComponent::new(MailScheme::Imap, Arc::new(cfg))
        .create_endpoint(
            &format!(
                "imap://127.0.0.1:{}?allowCleartext=true&initialDelay=0",
                server.port
            ),
            &NoOpComponentContext,
        )
        .expect("imap endpoint");
    let mut consumer = endpoint
        .create_consumer(Arc::new(NoopRuntimeObservability))
        .expect("consumer");
    let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(1);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "refused-login".to_string());
    let res = timeout(BOUND, consumer.start(ctx))
        .await
        .expect("consumer must stop, not loop");
    let err = res.expect_err("a refused login stops the consumer");
    assert!(err.to_string().contains("refused the login"), "{err}");
    assert_eq!(login_lines(&server), 1, "{:?}", server.lines());
    // No URL is ever logged, so nothing needs `redact_url`; keep it so.
    let text = logs.text();
    assert!(text.contains("mail consumer stopped"), "{text}");
    assert!(!text.contains("hunter2"), "{text}");
    assert!(
        !text.contains(&format!("127.0.0.1:{}", server.port)),
        "{text}"
    );
}
