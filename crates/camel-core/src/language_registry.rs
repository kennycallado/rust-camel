//! Helper for constructing a `SharedLanguageRegistry` from `LanguagesConfig`.
//!
//! This is the single source of truth for building language registries — called
//! by `CamelContextBuilder::built_in_languages` (which passes
//! `LanguagesConfig::default()`) and by `camel-cli` / programmatic embedders
//! that want to apply `Camel.toml` limits at registration time.
//!
//! Engines are only included when their cargo feature is enabled
//! (`lang-js`, `lang-rhai`). The `simple` language is always registered.
//! Languages without configurable limits (jsonpath, xpath) are registered
//! with their default constructor.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use camel_language_api::Language;
use camel_language_api::LanguagesConfig;
use thiserror::Error;

use crate::lifecycle::adapters::route_controller::SharedLanguageRegistry;

/// Error type for language registration operations on [`CamelContext`](crate::CamelContext).
///
/// This is distinct from [`camel_language_api::error::LanguageError`], which
/// covers language-evaluation concerns (parse, eval, unknown variable, etc.).
/// Registration is a context-configuration invariant, not a language concern.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LanguageRegistryError {
    #[error("language '{name}' is already registered")]
    AlreadyRegistered { name: String },
}

/// Build a language registry from `[languages.*]` config blocks.
///
/// Engines are only included when their cargo feature is enabled
/// (`lang-rhai`, `lang-js`). The `simple` language is always registered
/// (no limits apply). Languages without configurable limits (jsonpath, xpath)
/// are registered with their default constructor so this helper is the single
/// source of truth consumed by `built_in_languages()`.
pub fn from_config(_config: &LanguagesConfig) -> SharedLanguageRegistry {
    let mut languages: HashMap<String, Arc<dyn Language>> = HashMap::new();

    languages.insert(
        "simple".to_string(),
        Arc::new(camel_language_simple::SimpleLanguage::new()),
    );

    #[cfg(feature = "lang-js")]
    {
        let js_lang = camel_language_js::JsLanguage::with_limits(_config.js.limits.clone());
        languages.insert("js".to_string(), Arc::new(js_lang.clone()));
        languages.insert("javascript".to_string(), Arc::new(js_lang));
    }

    #[cfg(feature = "lang-rhai")]
    {
        let rhai_lang = camel_language_rhai::RhaiLanguage::with_limits(_config.rhai.limits.clone());
        languages.insert("rhai".to_string(), Arc::new(rhai_lang));
    }

    // Languages without configurable limits — registered with ::new().
    // Included here so built_in_languages() can delegate to from_config()
    // as a single source of truth.
    #[cfg(feature = "lang-jsonpath")]
    {
        languages.insert(
            "jsonpath".to_string(),
            Arc::new(camel_language_jsonpath::JsonPathLanguage::new()),
        );
    }

    #[cfg(feature = "lang-xpath")]
    {
        languages.insert(
            "xpath".to_string(),
            Arc::new(camel_language_xpath::XPathLanguage::new()),
        );
    }

    Arc::new(Mutex::new(languages))
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_language_api::{JsLimitsConfig, LanguagesConfig, RhaiLimitsConfig};

    #[test]
    fn from_config_includes_simple_always() {
        let registry = from_config(&LanguagesConfig::default());
        let registry = registry.lock().expect("lock registry");
        assert!(registry.contains_key("simple"));
    }

    #[test]
    fn from_config_succeeds_with_rhai_limits_when_feature_enabled() {
        let cfg = LanguagesConfig {
            rhai: camel_language_api::RhaiEngineConfig {
                limits: RhaiLimitsConfig {
                    max_operations: Some(10),
                    ..Default::default()
                },
            },
            ..Default::default()
        };
        let registry = from_config(&cfg);
        let registry = registry.lock().expect("lock registry");
        #[cfg(feature = "lang-rhai")]
        {
            let lang = registry.get("rhai").expect("rhai present");
            // We can't easily introspect limits on the trait object; this test
            // just confirms the construction succeeds with custom limits.
            assert_eq!(lang.name(), "rhai");
        }
        #[cfg(not(feature = "lang-rhai"))]
        {
            assert!(!registry.contains_key("rhai"));
        }
    }

    #[test]
    fn from_config_succeeds_with_js_limits_when_feature_enabled() {
        let cfg = LanguagesConfig {
            js: camel_language_api::JsEngineConfig {
                limits: JsLimitsConfig {
                    max_loop_iterations: Some(1_000),
                    ..Default::default()
                },
            },
            ..Default::default()
        };
        let registry = from_config(&cfg);
        let registry = registry.lock().expect("lock registry");
        #[cfg(feature = "lang-js")]
        {
            let lang = registry.get("js").expect("js present");
            assert_eq!(lang.name(), "js");
        }
        #[cfg(not(feature = "lang-js"))]
        {
            assert!(!registry.contains_key("js"));
        }
    }
}
