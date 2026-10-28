//! lettre-backed SMTP helpers. Third-party types stay in this module.

use async_trait::async_trait;
use lettre::address::Envelope;
use lettre::message::header::{HeaderName, HeaderValue};
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::PoolConfig;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Certificate, CertificateStore, Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use tracing::debug;

use crate::address::MailAddress;
use crate::config::{ContentKind, MailAccountConfig, SmtpEndpointConfig, TlsMode};
use crate::error::MailError;
use crate::message::OutboundMessage;
use crate::tls;
use crate::transport::MailTransport;

fn to_address(m: lettre::message::Mailbox) -> MailAddress {
    MailAddress {
        name: m.name,
        email: m.email.to_string(),
    }
}

pub(crate) fn parse_mailbox(raw: &str) -> Result<MailAddress, MailError> {
    raw.trim()
        .parse::<lettre::message::Mailbox>()
        .map(to_address)
        .map_err(|e| MailError::Address(e.to_string()))
}

pub(crate) fn parse_mailbox_list(raw: &str) -> Result<Vec<MailAddress>, MailError> {
    let list = raw
        .parse::<lettre::message::Mailboxes>()
        .map_err(|e| MailError::Address(e.to_string()))?;
    let out: Vec<MailAddress> = list.into_iter().map(to_address).collect();
    if out.is_empty() {
        return Err(MailError::Address("empty address list".to_string()));
    }
    Ok(out)
}

fn to_lettre_address(a: &MailAddress) -> Result<lettre::Address, MailError> {
    a.email
        .parse::<lettre::Address>()
        .map_err(|e| MailError::Address(e.to_string()))
}

fn mailbox(a: &MailAddress) -> Result<Mailbox, MailError> {
    Ok(Mailbox::new(a.name.clone(), to_lettre_address(a)?))
}

fn build_envelope(msg: &OutboundMessage) -> Result<Envelope, MailError> {
    let from = to_lettre_address(&msg.from)?;
    let to = msg
        .recipients
        .envelope()
        .into_iter()
        .map(to_lettre_address)
        .collect::<Result<Vec<_>, _>>()?;
    Envelope::new(Some(from), to).map_err(|e| MailError::Transport(e.to_string()))
}

fn transport_err(e: impl std::fmt::Display) -> MailError {
    MailError::Transport(e.to_string())
}

/// Render the message to RFC 5322 bytes. `Bcc` is never written; the SMTP
/// envelope is built explicitly so Bcc-only messages still render.
pub(crate) fn render_mime(msg: &OutboundMessage) -> Result<Vec<u8>, MailError> {
    let mut builder = lettre::Message::builder()
        .from(mailbox(&msg.from)?)
        .message_id(Some(msg.message_id.clone()))
        .envelope(build_envelope(msg)?);
    for a in &msg.recipients.to {
        builder = builder.to(mailbox(a)?);
    }
    for a in &msg.recipients.cc {
        builder = builder.cc(mailbox(a)?);
    }
    if let Some(subject) = &msg.subject {
        builder = builder.subject(subject.clone());
    }
    for (name, value) in &msg.extra_headers {
        let name = HeaderName::new_from_ascii(name.clone()).map_err(transport_err)?;
        builder = builder.raw_header(HeaderValue::new(name, value.clone()));
    }
    let message = match (&msg.content, &msg.html_alternative) {
        (ContentKind::Html, _) => builder.singlepart(SinglePart::html(msg.body.clone())),
        (ContentKind::Plain, Some(html)) => builder.multipart(MultiPart::alternative_plain_html(
            msg.body.clone(),
            html.clone(),
        )),
        (ContentKind::Plain, None) => builder.singlepart(SinglePart::plain(msg.body.clone())),
    }
    .map_err(transport_err)?;
    Ok(message.formatted())
}

/// Pooled lettre SMTP transport (rustls with the ring provider).
pub(crate) struct LettreSmtpTransport {
    inner: AsyncSmtpTransport<Tokio1Executor>,
}

impl LettreSmtpTransport {
    pub(crate) fn new(
        cfg: &SmtpEndpointConfig,
        account: Option<&MailAccountConfig>,
    ) -> Result<Self, MailError> {
        let mut builder =
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(cfg.server.host.clone())
                .port(cfg.server.port);

        let tls = match cfg.tls {
            TlsMode::Cleartext => Tls::None,
            TlsMode::StartTls | TlsMode::Implicit => {
                let mut params = TlsParameters::builder(cfg.server.host.clone())
                    .certificate_store(CertificateStore::WebpkiRoots);
                if let Some(path) = account.and_then(|a| a.ca_file.as_deref()) {
                    for der in tls::load_ca_file(path)? {
                        let cert = Certificate::from_der(der.to_vec())
                            .map_err(|e| MailError::Tls(e.to_string()))?;
                        params = params.add_root_certificate(cert);
                    }
                }
                let params = params
                    // lettre 0.11 takes no provider argument: it uses the process default rustls
                    // provider when one is installed, else ring (the only backend lettre is built with).
                    .build_rustls()
                    .map_err(|e| MailError::Tls(e.to_string()))?;
                if cfg.tls == TlsMode::StartTls {
                    Tls::Required(params)
                } else {
                    Tls::Wrapper(params)
                }
            }
        };
        builder = builder.tls(tls);

        if let Some(a) = account
            && let (Some(user), Some(secret)) = (&a.username, &a.password)
        {
            builder = builder
                .credentials(Credentials::new(user.clone(), secret.expose().to_string()))
                .authentication(vec![Mechanism::Plain, Mechanism::Login]);
        }

        let inner = builder.pool_config(PoolConfig::new()).build();
        Ok(Self { inner })
    }
}

#[async_trait]
impl MailTransport for LettreSmtpTransport {
    async fn send(&self, message: &OutboundMessage) -> Result<(), MailError> {
        let envelope = build_envelope(message)?;
        let raw = render_mime(message)?;
        let response = self
            .inner
            .send_raw(&envelope, &raw)
            .await
            .map_err(transport_err)?;
        debug!(
            reply = %response.message().collect::<Vec<_>>().join(" "),
            "mail accepted by server"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::build_outbound;
    use camel_api::{Body, Exchange, Message, Value};

    fn outbound(opts: &str, body: &str, headers: &[(&str, &str)]) -> OutboundMessage {
        let cfg = SmtpEndpointConfig::from_uri(&format!("smtp://h?from=bot@corp.test{opts}"))
            .expect("test uri");
        let mut m = Message::new(Body::Text(body.to_string()));
        for (k, v) in headers {
            m.set_header(*k, Value::String((*v).to_string()));
        }
        build_outbound(&cfg, &Exchange::new(m)).expect("outbound")
    }

    fn render(m: &OutboundMessage) -> String {
        String::from_utf8(render_mime(m).expect("render")).expect("utf8")
    }

    #[test]
    fn render_plain_utf8() {
        let out = render(&outbound("&to=a@corp.test", "hello", &[])).to_ascii_lowercase();
        assert!(
            out.contains("content-type: text/plain; charset=utf-8"),
            "{out}"
        );
        assert!(out.contains("hello"));
    }

    #[test]
    fn render_html() {
        let out = render(&outbound(
            "&to=a@corp.test&contentType=text/html",
            "<b>x</b>",
            &[],
        ));
        assert!(out.contains("Content-Type: text/html"), "{out}");
    }

    #[test]
    fn render_alternative() {
        let m = outbound(
            "&to=a@corp.test",
            "plain",
            &[("CamelMailHtmlBody", "<b>h</b>")],
        );
        let out = render(&m);
        assert!(out.contains("multipart/alternative"), "{out}");
        assert!(out.contains("text/plain"));
        assert!(out.contains("text/html"));
    }

    #[test]
    fn render_never_writes_bcc() {
        let out = render(&outbound("&to=a@corp.test&bcc=zbcc@corp.test", "x", &[]));
        assert!(!out.contains("Bcc:"), "{out}");
        assert!(!out.contains("zbcc@corp.test"), "{out}");
    }

    #[test]
    fn render_bcc_only_message() {
        let m = outbound("&bcc=zbcc@corp.test", "x", &[]);
        assert!(m.recipients.to.is_empty() && m.recipients.cc.is_empty());
        let out = render(&m);
        assert!(!out.contains("Bcc:"), "{out}");
        assert!(!out.contains("zbcc@corp.test"), "{out}");
    }

    #[test]
    fn render_emits_x_header() {
        let out = render(&outbound("&to=a@corp.test", "x", &[("X-Trace", "abc")]));
        assert!(out.contains("X-Trace: abc"), "{out}");
    }

    #[test]
    fn render_sets_message_id() {
        let m = outbound("&to=a@corp.test", "x", &[]);
        let out = render(&m);
        assert!(
            out.contains(&format!("Message-ID: {}", m.message_id)),
            "{out}"
        );
    }

    #[tokio::test]
    async fn send_to_dropping_server_is_transport_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let accept = tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        let uri = format!("smtp://127.0.0.1:{port}?from=bot@corp.test&allowCleartext=true");
        let cfg = SmtpEndpointConfig::from_uri(&uri).expect("uri");
        let t = LettreSmtpTransport::new(&cfg, None).expect("transport");
        let m = outbound("&to=a@corp.test", "x", &[]);
        let r = tokio::time::timeout(std::time::Duration::from_secs(10), t.send(&m))
            .await
            .expect("send finished");
        assert!(matches!(r, Err(MailError::Transport(_))), "{r:?}");
        tokio::time::timeout(std::time::Duration::from_secs(10), accept)
            .await
            .expect("accept finished")
            .expect("join");
    }

    #[tokio::test]
    async fn transport_builds_for_each_tls_mode() {
        for uri in [
            "smtp://h?from=a@x.test",
            "smtps://h?from=a@x.test",
            "smtp://h?from=a@x.test&allowCleartext=true",
        ] {
            let cfg = SmtpEndpointConfig::from_uri(uri).expect("uri");
            assert!(LettreSmtpTransport::new(&cfg, None).is_ok(), "{uri}");
        }
        let cfg = SmtpEndpointConfig::from_uri("smtp://h?from=a@x.test").expect("uri");
        let account = MailAccountConfig {
            username: None,
            password: None,
            ca_file: Some(std::path::PathBuf::from("/nonexistent/ca.pem")),
        };
        assert!(matches!(
            LettreSmtpTransport::new(&cfg, Some(&account)),
            Err(MailError::Tls(_))
        ));
    }

    #[test]
    fn parse_mailbox_with_display_name() {
        let a = parse_mailbox("Bot <bot@corp.test>").unwrap();
        assert_eq!(a.name.as_deref(), Some("Bot"));
        assert_eq!(a.email, "bot@corp.test");
    }

    #[test]
    fn parse_mailbox_list_empty_is_address_error() {
        assert!(matches!(parse_mailbox_list(""), Err(MailError::Address(_))));
    }

    #[test]
    fn recipient_display_name_cannot_inject_a_header() {
        let cfg = SmtpEndpointConfig::from_uri(
            "smtp://h?from=bot@corp.test&allowedRecipients=@corp.test",
        )
        .expect("uri");
        for raw in [
            "\"a\r\nBcc: evil@x.test\" <a@corp.test>",
            "a\r\nBcc: evil@x.test <a@corp.test>",
            "=?utf-8?q?a=0D=0ABcc:_evil@x.test?= <a@corp.test>",
        ] {
            let mut m = Message::new(Body::Text("x".to_string()));
            m.set_header("CamelMailTo", Value::String(raw.to_string()));
            let Ok(out) = build_outbound(&cfg, &Exchange::new(m)) else {
                continue;
            };
            let rendered = render(&out);
            let head = rendered.split("\r\n\r\n").next().unwrap_or_default();
            assert!(
                !head
                    .lines()
                    .any(|l| l.to_ascii_lowercase().starts_with("bcc")),
                "{raw:?} injected a header: {head}"
            );
            let envelope: Vec<&str> = out
                .recipients
                .envelope()
                .into_iter()
                .map(|a| a.email.as_str())
                .collect();
            assert_eq!(envelope, ["a@corp.test"], "{raw:?}");
        }
    }
}
