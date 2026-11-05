//! `MailComponent`: one instance per registered mail scheme.

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::IdempotentRepository;
use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, ComponentMetadata, Consumer, Endpoint,
    NetworkRetryPolicy, PollingConsumer, ProducerContext, RuntimeObservability,
};

use crate::adapter::imap::AsyncImapConnector;
use crate::adapter::smtp::LettreSmtpTransport;
use crate::config;
use crate::config::{
    ImapEndpointConfig, MailAccountConfig, MailComponentConfig, MailScheme, SmtpEndpointConfig,
};
use crate::consumer::MailConsumer;
use crate::health::{MailHealthCheck, MailHealthState};
use crate::metadata;
use crate::poll::Mailbox;
use crate::polling::MailPollingConsumer;
use crate::producer::SmtpProducer;

/// Component serving one mail URI scheme. `MailBundle` registers one per
/// scheme; build one directly only to register a single scheme.
pub struct MailComponent {
    scheme: MailScheme,
    config: Arc<MailComponentConfig>,
    repositories: Arc<HashMap<String, Arc<dyn IdempotentRepository>>>,
}

impl MailComponent {
    /// Component for `scheme` reading accounts and reconnect policy from
    /// `config`, with no idempotent repository.
    pub fn new(scheme: MailScheme, config: Arc<MailComponentConfig>) -> Self {
        Self {
            scheme,
            config,
            repositories: Arc::new(HashMap::new()),
        }
    }

    /// Provide the named idempotent repositories `idempotentRepository=` resolves against.
    pub fn with_idempotent_repositories(
        mut self,
        repos: Arc<HashMap<String, Arc<dyn IdempotentRepository>>>,
    ) -> Self {
        self.repositories = repos;
        self
    }
}

impl Component for MailComponent {
    fn scheme(&self) -> &str {
        self.scheme.as_str()
    }

    fn metadata(&self) -> ComponentMetadata {
        match self.scheme {
            MailScheme::Smtp | MailScheme::Smtps => metadata::smtp_metadata(self.scheme),
            MailScheme::Imap | MailScheme::Imaps => metadata::imap_metadata(self.scheme),
        }
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let parsed = camel_component_api::parse_uri(uri)?;
        if parsed.scheme != self.scheme.as_str() {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme '{}', got '{}'",
                self.scheme.as_str(),
                parsed.scheme
            )));
        }
        let requested = parsed.params.get("account").map(String::as_str);
        if requested.is_none()
            && self.config.accounts.len() == 1
            && config::uri_has_no_host(&parsed.path)
        {
            // One account and no host to match: implicit selection can never
            // succeed, so the typed binding error wins over the generic
            // authority-parse text.
            config::bind_account(&self.config.accounts, requested, None)?;
        }
        match self.scheme {
            MailScheme::Smtp | MailScheme::Smtps => {
                let cfg = SmtpEndpointConfig::from_uri(uri)?;
                let account = config::bind_account(
                    &self.config.accounts,
                    cfg.account.as_deref(),
                    Some(&cfg.server.host),
                )?
                .map(|(_, account)| account);
                Ok(Box::new(SmtpEndpoint {
                    uri: uri.to_string(),
                    config: Arc::new(cfg),
                    account,
                }))
            }
            MailScheme::Imap | MailScheme::Imaps => {
                let cfg = ImapEndpointConfig::from_uri(uri)?;
                let account = config::bind_account(
                    &self.config.accounts,
                    cfg.account.as_deref(),
                    Some(&cfg.server.host),
                )?
                .map(|(_, account)| account)
                .ok_or_else(|| {
                    CamelError::EndpointCreationFailed(
                        "imap endpoints require an account in [components.mail.accounts]"
                            .to_string(),
                    )
                })?;
                let idempotent = cfg
                    .idempotent_repository
                    .as_deref()
                    .map(|name| self.repository(name))
                    .transpose()?;
                let connector = AsyncImapConnector::from_endpoint(&cfg, &account)
                    .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;
                let retry = self
                    .config
                    .reconnect
                    .clone()
                    .unwrap_or_else(config::mail_reconnect_default);
                let stale_after = cfg.delay * 2;
                let endpoint = ImapEndpoint {
                    uri: uri.to_string(),
                    mailbox: Mailbox {
                        config: Arc::new(cfg),
                        connector: Arc::new(connector),
                        idempotent,
                    },
                    retry,
                    health: MailHealthState::new(),
                };
                ctx.register_current_route_health_check(Arc::new(MailHealthCheck::new(
                    Arc::clone(&endpoint.mailbox.connector),
                    Arc::clone(&endpoint.health),
                    stale_after,
                )));
                Ok(Box::new(endpoint))
            }
        }
    }
}

impl MailComponent {
    fn repository(&self, name: &str) -> Result<Arc<dyn IdempotentRepository>, CamelError> {
        self.repositories.get(name).cloned().ok_or_else(|| {
            let mut names: Vec<&str> = self.repositories.keys().map(String::as_str).collect();
            names.sort_unstable();
            CamelError::EndpointCreationFailed(format!(
                "idempotentRepository '{name}' is not registered; available: {}",
                names.join(", ")
            ))
        })
    }
}

/// Endpoint receiving mail over IMAP.
struct ImapEndpoint {
    uri: String,
    mailbox: Mailbox,
    retry: NetworkRetryPolicy,
    health: Arc<MailHealthState>,
}

impl Endpoint for ImapEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        self.health.attach();
        Ok(Box::new(MailConsumer::new(
            self.mailbox.clone(),
            self.retry.clone(),
            Arc::clone(&self.health),
            rt,
        )))
    }

    fn polling_consumer(&self) -> Option<Box<dyn PollingConsumer>> {
        Some(Box::new(MailPollingConsumer::new(self.mailbox.clone())))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed(format!(
            "{} endpoints only receive mail; use smtp: or smtps: to send",
            self.mailbox.config.scheme.as_str()
        )))
    }
}

struct SmtpEndpoint {
    uri: String,
    config: Arc<SmtpEndpointConfig>,
    account: Option<MailAccountConfig>,
}

impl Endpoint for SmtpEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(format!(
            "{} endpoints only send mail; use imap: or imaps: to receive",
            self.config.scheme.as_str()
        )))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let transport = LettreSmtpTransport::new(&self.config, self.account.as_ref())
            .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;
        Ok(BoxProcessor::new(SmtpProducer::new(
            Arc::clone(&self.config),
            Arc::new(transport),
            rt,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SecretString;
    use camel_component_api::NoOpComponentContext;
    use camel_component_api::test_support::PanicRuntimeObservability;
    use std::collections::HashMap;
    use std::io;
    use std::sync::Mutex;
    use std::sync::PoisonError;

    fn account_with_hosts(hosts: &[&str]) -> MailAccountConfig {
        MailAccountConfig {
            username: Some("u".to_string()),
            password: Some(SecretString::from("pw")),
            ca_file: None,
            allowed_hosts: hosts.iter().map(|h| h.to_string()).collect(),
        }
    }

    fn account() -> MailAccountConfig {
        account_with_hosts(&["h"])
    }

    fn component_with(accounts: HashMap<String, MailAccountConfig>) -> MailComponent {
        MailComponent::new(
            MailScheme::Smtp,
            Arc::new(MailComponentConfig {
                accounts,
                reconnect: None,
            }),
        )
    }

    fn one_account(account: MailAccountConfig) -> MailComponent {
        component_with(HashMap::from([("main".to_string(), account)]))
    }

    fn component(names: &[&str]) -> MailComponent {
        let accounts: HashMap<String, MailAccountConfig> =
            names.iter().map(|n| (n.to_string(), account())).collect();
        component_with(accounts)
    }

    const URI: &str = "smtp://h?from=a@x.test&to=b@x.test";

    #[test]
    fn create_endpoint_selects_single_account() {
        assert!(
            component(&["main"])
                .create_endpoint(URI, &NoOpComponentContext)
                .is_ok()
        );
    }

    #[test]
    fn create_endpoint_explicit_account_skips_allowed_hosts() {
        let ep = one_account(account_with_hosts(&[]))
            .create_endpoint(
                "smtp://h?account=main&from=a@x.test&to=b@x.test",
                &NoOpComponentContext,
            )
            .expect("explicit selection is the documented escape hatch");
        assert_eq!(ep.uri(), "smtp://h?account=main&from=a@x.test&to=b@x.test");
    }

    #[test]
    fn create_endpoint_rejects_ambiguous_account() {
        let e = component(&["a", "b"])
            .create_endpoint(URI, &NoOpComponentContext)
            .err()
            .expect("err");
        assert!(e.to_string().contains("?account="), "{e}");
    }

    #[test]
    fn unknown_repository_fails_endpoint_creation() {
        let accounts: HashMap<String, MailAccountConfig> =
            HashMap::from([("main".to_string(), account())]);
        let comp = MailComponent::new(
            MailScheme::Imap,
            Arc::new(MailComponentConfig {
                accounts,
                reconnect: None,
            }),
        )
        .with_idempotent_repositories(Arc::new(HashMap::new()));
        let e = comp
            .create_endpoint(
                "imap://h?allowCleartext=true&idempotentRepository=ghost",
                &NoOpComponentContext,
            )
            .err()
            .expect("err");
        assert!(e.to_string().contains("ghost"), "{e}");
    }

    #[test]
    fn create_endpoint_rejects_userinfo() {
        let e = component(&["main"])
            .create_endpoint(
                "smtp://u:secret@h?from=a@x.test&to=b@x.test",
                &NoOpComponentContext,
            )
            .err()
            .expect("err");
        let s = e.to_string();
        assert!(s.contains("[components.mail.accounts]"), "{s}");
        assert!(!s.contains("secret"), "{s}");
    }

    #[test]
    fn smtp_endpoint_refuses_consumer() {
        let ep = component(&["main"])
            .create_endpoint(URI, &NoOpComponentContext)
            .expect("ep");
        let e = ep
            .create_consumer(Arc::new(PanicRuntimeObservability))
            .err()
            .expect("err");
        assert!(e.to_string().contains("imap"), "{e}");
    }

    fn component_for(scheme: MailScheme, names: &[&str]) -> MailComponent {
        let accounts: HashMap<String, MailAccountConfig> =
            names.iter().map(|n| (n.to_string(), account())).collect();
        MailComponent::new(
            scheme,
            Arc::new(MailComponentConfig {
                accounts,
                reconnect: None,
            }),
        )
    }

    #[test]
    fn default_ports_on_endpoints() {
        for (scheme, uri) in [
            (MailScheme::Smtp, "smtp://h?from=a@x.test"),
            (MailScheme::Smtps, "smtps://h?from=a@x.test"),
            (MailScheme::Imap, "imap://h"),
            (MailScheme::Imaps, "imaps://h"),
        ] {
            assert!(
                component_for(scheme, &["main"])
                    .create_endpoint(uri, &NoOpComponentContext)
                    .is_ok(),
                "{uri}"
            );
        }
        assert_eq!(
            SmtpEndpointConfig::from_uri("smtp://h?from=a@x.test")
                .expect("smtp")
                .server
                .port,
            587
        );
        assert_eq!(
            SmtpEndpointConfig::from_uri("smtps://h?from=a@x.test")
                .expect("smtps")
                .server
                .port,
            465
        );
        assert_eq!(
            ImapEndpointConfig::from_uri("imap://h")
                .expect("imap")
                .server
                .port,
            143
        );
        assert_eq!(
            ImapEndpointConfig::from_uri("imaps://h")
                .expect("imaps")
                .server
                .port,
            993
        );
    }

    #[test]
    fn smtp_endpoint_has_no_polling_consumer() {
        let ep = component(&["main"])
            .create_endpoint(URI, &NoOpComponentContext)
            .expect("ep");
        assert!(ep.polling_consumer().is_none());
    }

    #[test]
    fn imap_endpoint_requires_account() {
        let e = component_for(MailScheme::Imap, &[])
            .create_endpoint("imap://h", &NoOpComponentContext)
            .err()
            .expect("err");
        assert!(e.to_string().contains("[components.mail.accounts]"), "{e}");
    }

    #[test]
    fn imap_endpoint_refuses_producer() {
        let ep = component_for(MailScheme::Imap, &["main"])
            .create_endpoint("imap://h", &NoOpComponentContext)
            .expect("ep");
        let ctx = ProducerContext::new();
        let e = ep
            .create_producer(Arc::new(PanicRuntimeObservability), &ctx)
            .expect_err("err");
        assert!(e.to_string().contains("smtp"), "{e}");
    }

    fn binding_error(component: &MailComponent, uri: &str) -> CamelError {
        let e = component
            .create_endpoint(uri, &NoOpComponentContext)
            .err()
            .expect("endpoint creation must fail");
        assert!(
            matches!(&e, CamelError::EndpointCreationFailed(_)),
            "expected EndpointCreationFailed, got {e:?}"
        );
        e
    }

    #[test]
    fn implicit_single_account_without_allowed_hosts_fails() {
        let e = binding_error(
            &one_account(account_with_hosts(&[])),
            "smtp://mx.example.com?from=a@x.test&to=b@x.test",
        );
        let s = e.to_string();
        for needle in [
            "main",
            "?account=",
            "allowedHosts",
            "[components.mail.accounts.main]",
        ] {
            assert!(s.contains(needle), "missing {needle:?} in {s}");
        }
        assert!(!s.contains("mx.example.com"), "{s}");
    }

    #[test]
    fn implicit_single_account_host_in_allowed_hosts_succeeds() {
        assert!(
            one_account(account_with_hosts(&["mx.example.com"]))
                .create_endpoint(
                    "smtp://mx.example.com?from=a@x.test&to=b@x.test",
                    &NoOpComponentContext,
                )
                .is_ok()
        );
    }

    #[test]
    fn implicit_single_account_host_mismatch_fails() {
        let e = binding_error(
            &one_account(account_with_hosts(&["other.example.com"])),
            "smtp://mx.example.com?from=a@x.test&to=b@x.test",
        );
        let s = e.to_string();
        assert!(s.contains("main"), "{s}");
        assert!(s.contains("?account="), "{s}");
        assert!(s.contains("allowedHosts"), "{s}");
        assert!(!s.contains("mx.example.com"), "{s}");
    }

    #[test]
    fn allowed_hosts_match_is_case_insensitive() {
        assert!(
            one_account(account_with_hosts(&["MX.Example.COM"]))
                .create_endpoint("smtp://mx.example.com?from=a@x.test", &NoOpComponentContext)
                .is_ok()
        );
    }

    #[test]
    fn explicit_account_ignores_allowed_hosts() {
        assert!(
            one_account(account_with_hosts(&[]))
                .create_endpoint(
                    "smtp://mx.example.com?account=main&from=a@x.test",
                    &NoOpComponentContext,
                )
                .is_ok()
        );
    }

    #[test]
    fn implicit_selection_without_uri_host_fails() {
        let comp = one_account(account_with_hosts(&["mx.example.com"]));
        for uri in ["smtp:/x?from=a@x.test", "smtp://[]?from=a@x.test"] {
            let e = binding_error(&comp, uri);
            let s = e.to_string();
            for needle in ["main", "?account=", "allowedHosts"] {
                assert!(s.contains(needle), "{uri}: missing {needle:?} in {s}");
            }
            assert!(!s.contains("mail URIs use scheme://host"), "{uri}: {s}");
        }
    }

    #[test]
    fn multi_account_without_account_param_still_fails() {
        let e = component(&["a", "b"])
            .create_endpoint("smtp://h?from=a@x.test", &NoOpComponentContext)
            .err()
            .expect("err");
        let s = e.to_string();
        assert!(s.contains("?account="), "{s}");
        assert!(s.contains('2'), "{s}");
    }

    fn ensure_global_tracing_default() {
        static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if INIT.set(()).is_ok() {
            let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
        }
    }

    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
        type Writer = BufWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_sink() -> (tracing::subscriber::DefaultGuard, Arc<Mutex<Vec<u8>>>) {
        ensure_global_tracing_default();
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sub = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(BufWriter(Arc::clone(&buf)))
            .finish();
        (tracing::subscriber::set_default(sub), buf)
    }

    fn log_text(buf: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8_lossy(&buf.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }

    #[test]
    fn error_and_log_never_contain_host() {
        let (_guard, buf) = capture_sink();
        let e = binding_error(
            &one_account(account_with_hosts(&["other.example.com"])),
            "smtp://mx.example.com?from=a@x.test",
        );
        let s = e.to_string();
        assert!(s.contains("?account="), "{s}");
        assert!(s.contains("allowedHosts"), "{s}");
        assert!(!s.contains("mx.example.com"), "{s}");
        let logs = log_text(&buf);
        assert!(!logs.contains("mx.example.com"), "{logs}");
    }
}
