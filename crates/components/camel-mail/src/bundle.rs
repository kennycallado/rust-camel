//! `MailBundle`: registers the `smtp`, `smtps`, `imap` and `imaps` schemes from `[components.mail]`.

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::IdempotentRepository;
use camel_component_api::{CamelError, ComponentBundle, ComponentRegistrar};

use crate::component::MailComponent;
use crate::config::{MailComponentConfig, MailScheme};

/// Bundle owning the `smtp`, `smtps`, `imap` and `imaps` schemes.
pub struct MailBundle {
    config: Arc<MailComponentConfig>,
    idempotent_repositories: HashMap<String, Arc<dyn IdempotentRepository>>,
}

impl MailBundle {
    /// Hand over the idempotent repositories resolved at boot, by name.
    pub fn with_idempotent_repositories(
        mut self,
        repos: HashMap<String, Arc<dyn IdempotentRepository>>,
    ) -> Self {
        self.idempotent_repositories = repos;
        self
    }
}

impl ComponentBundle for MailBundle {
    fn config_key() -> &'static str {
        "mail"
    }

    fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
        let config: MailComponentConfig = value
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self {
            config: Arc::new(config),
            idempotent_repositories: HashMap::new(),
        })
    }

    fn register_all(self, ctx: &mut dyn ComponentRegistrar) {
        let repositories = Arc::new(self.idempotent_repositories);
        for scheme in [
            MailScheme::Smtp,
            MailScheme::Smtps,
            MailScheme::Imap,
            MailScheme::Imaps,
        ] {
            ctx.register_component_dyn(Arc::new(
                MailComponent::new(scheme, Arc::clone(&self.config))
                    .with_idempotent_repositories(Arc::clone(&repositories)),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::Component;

    struct TestRegistrar {
        schemes: Vec<String>,
    }

    impl ComponentRegistrar for TestRegistrar {
        fn register_component_dyn(&mut self, component: Arc<dyn Component>) {
            self.schemes.push(component.scheme().to_string());
        }
    }

    #[test]
    fn bundle_registers_four_schemes() {
        let bundle = MailBundle::from_toml(toml::Value::Table(Default::default())).expect("ok");
        let mut reg = TestRegistrar { schemes: vec![] };
        bundle.register_all(&mut reg);
        assert_eq!(reg.schemes, vec!["smtp", "smtps", "imap", "imaps"]);
    }

    #[test]
    fn bundle_rejects_unknown_key() {
        let value: toml::Value = toml::from_str("bogus = 1").expect("toml");
        assert!(MailBundle::from_toml(value).is_err());
    }
}
