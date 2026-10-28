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
        match self.scheme {
            MailScheme::Smtp | MailScheme::Smtps => {
                let cfg = SmtpEndpointConfig::from_uri(uri)?;
                let account = self.account(cfg.account.as_deref())?;
                Ok(Box::new(SmtpEndpoint {
                    uri: uri.to_string(),
                    config: Arc::new(cfg),
                    account,
                }))
            }
            MailScheme::Imap | MailScheme::Imaps => {
                let cfg = ImapEndpointConfig::from_uri(uri)?;
                let account = self.account(cfg.account.as_deref())?.ok_or_else(|| {
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
    /// The account an endpoint uses (see [`config::resolve_account_name`]).
    fn account(&self, requested: Option<&str>) -> Result<Option<MailAccountConfig>, CamelError> {
        let name = config::resolve_account_name(&self.config.accounts, requested)?;
        Ok(name.and_then(|name| self.config.accounts.get(&name).cloned()))
    }

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

    fn account() -> MailAccountConfig {
        MailAccountConfig {
            username: Some("u".to_string()),
            password: Some(SecretString::from("pw")),
            ca_file: None,
        }
    }

    fn component(names: &[&str]) -> MailComponent {
        let accounts: HashMap<String, MailAccountConfig> =
            names.iter().map(|n| (n.to_string(), account())).collect();
        MailComponent::new(
            MailScheme::Smtp,
            Arc::new(MailComponentConfig {
                accounts,
                reconnect: None,
            }),
        )
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
}
