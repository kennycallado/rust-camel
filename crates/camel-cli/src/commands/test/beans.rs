//! Stub beans for declarative test documents: in-process
//! [`BeanProcessor`](camel_bean::BeanProcessor) implementations (echo,
//! setBody, fail) built from the document's `beans:` block and registered in
//! a [`camel_bean::BeanRegistry`] before the context boots, so `bean:` steps
//! in the routes resolve against them.
//!
//! Spec: openspec/changes/bean-test-registry (design D2, Task 3).

use camel_api::{Body, CamelError, Exchange};
use camel_bean::BeanProcessor;
use camel_core::route::{BuilderStep, RouteDefinition};

use super::document::{BeanDeclDoc, BeanKindDoc};

/// Behavior of one stub bean.
#[derive(Debug)]
pub(crate) enum StubKind {
    /// Pass the exchange through untouched.
    Echo,
    /// Replace the input body with the configured string.
    SetBody { body: String },
    /// Fail with the final message: the configured `message` value, or
    /// exactly `fail bean <name>` when no message is configured.
    Fail { message: String },
}

/// One declared stub bean: name, method allowlist, and behavior.
pub(crate) struct StubBean {
    name: String,
    methods: Vec<String>,
    kind: StubKind,
}

#[async_trait::async_trait]
impl BeanProcessor for StubBean {
    async fn call(&self, _method: &str, exchange: &mut Exchange) -> Result<(), CamelError> {
        match &self.kind {
            StubKind::Echo => Ok(()),
            StubKind::SetBody { body } => {
                exchange.input.body = Body::Text(body.clone());
                Ok(())
            }
            StubKind::Fail { message } => Err(CamelError::ProcessorError(message.clone())),
        }
    }

    fn methods(&self) -> Vec<String> {
        self.methods.clone()
    }
}

impl std::fmt::Debug for StubBean {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StubBean")
            .field("name", &self.name)
            .field("methods", &self.methods)
            .field("kind", &self.kind)
            .finish()
    }
}

/// Build one [`StubBean`] from its document declaration.
///
/// The `methods` list comes from the declaration when `methods` is present;
/// otherwise it is the wildcard resolved from `invoked` — the methods the
/// routes call on this bean, deduplicated, in order of first appearance.
/// `setBody` requires `config.body` and `fail` accepts only
/// `config.message`; both were validated eagerly by
/// [`parse_test_document`](super::document::parse_test_document), so the
/// `setBody` lookup here is infallible in practice.
pub(crate) fn stub_from_decl(name: &str, decl: &BeanDeclDoc, invoked: &[String]) -> StubBean {
    let kind = match decl.kind {
        BeanKindDoc::Echo => StubKind::Echo,
        BeanKindDoc::SetBody => StubKind::SetBody {
            body: decl
                .config
                .as_ref()
                .and_then(|config| config.get("body").cloned())
                .expect("parse_test_document validated setBody config.body"), // allow-unwrap
        },
        BeanKindDoc::Fail => StubKind::Fail {
            message: decl
                .config
                .as_ref()
                .and_then(|config| config.get("message").cloned())
                .unwrap_or_else(|| format!("fail bean {name}")),
        },
    };
    let methods = match decl.methods.as_ref() {
        Some(declared) => declared.clone(),
        None => {
            let mut seen: Vec<String> = Vec::new();
            for method in invoked {
                if !seen.contains(method) {
                    seen.push(method.clone());
                }
            }
            seen
        }
    };
    StubBean {
        name: name.to_string(),
        methods,
        kind,
    }
}

/// Collect every `(bean name, method)` pair the routes invoke, in
/// definition/step order, from each definition's steps AND its circuit
/// breaker fallback.
pub(crate) fn collect_bean_calls(defs: &[RouteDefinition]) -> Vec<(String, String)> {
    let mut calls = Vec::new();
    for def in defs {
        collect_from_steps(def.steps(), &mut calls);
        collect_from_steps(def.circuit_breaker_fallback(), &mut calls);
    }
    calls
}

/// Walk one step list, recursing into every nested step list.
///
/// The match is exhaustive with NO `_` catch-all arm: a future
/// [`BuilderStep`] variant that holds nested steps becomes a compile error
/// here instead of a silently un-walked location.
fn collect_from_steps(steps: &[BuilderStep], calls: &mut Vec<(String, String)>) {
    for step in steps {
        match step {
            BuilderStep::Bean { name, method } => calls.push((name.clone(), method.clone())),
            // Single flat `steps` child list.
            BuilderStep::DeclarativeFilter { steps, .. }
            | BuilderStep::DeclarativeSplit { steps, .. }
            | BuilderStep::DeclarativeStreamSplit { steps, .. }
            | BuilderStep::Split { steps, .. }
            | BuilderStep::Filter { steps, .. }
            | BuilderStep::Multicast { steps, .. }
            | BuilderStep::Throttle { steps, .. }
            | BuilderStep::LoadBalance { steps, .. }
            | BuilderStep::Loop { steps, .. }
            | BuilderStep::DeclarativeLoop { steps, .. }
            | BuilderStep::IdempotentConsumer { steps, .. } => collect_from_steps(steps, calls),
            // Choice shapes: when-clause sub-pipelines plus optional
            // otherwise branch (declarative and programmatic forms).
            BuilderStep::DeclarativeChoice { whens, otherwise } => {
                for when in whens {
                    collect_from_steps(&when.steps, calls);
                }
                if let Some(steps) = otherwise {
                    collect_from_steps(steps, calls);
                }
            }
            BuilderStep::Choice { whens, otherwise } => {
                for when in whens {
                    collect_from_steps(&when.steps, calls);
                }
                if let Some(steps) = otherwise {
                    collect_from_steps(steps, calls);
                }
            }
            BuilderStep::Cache { on_miss, .. } => collect_from_steps(on_miss, calls),
            BuilderStep::DeclarativeDoTry {
                try_steps,
                catch,
                finally,
            } => {
                collect_from_steps(try_steps, calls);
                for clause in catch {
                    collect_from_steps(&clause.steps, calls);
                }
                if let Some(finally) = finally {
                    collect_from_steps(&finally.steps, calls);
                }
            }
            // Leaf variants — hold no nested step lists.
            BuilderStep::Processor(_)
            | BuilderStep::To(_)
            | BuilderStep::Stop
            | BuilderStep::Log { .. }
            | BuilderStep::DeclarativeSetHeader { .. }
            | BuilderStep::DeclarativeSetHeaderIfAbsent { .. }
            | BuilderStep::DeclarativeRemoveHeader { .. }
            | BuilderStep::DeclarativeSetProperty { .. }
            | BuilderStep::DeclarativeSetBody { .. }
            | BuilderStep::DeclarativeScript { .. }
            | BuilderStep::DeclarativeFunction { .. }
            | BuilderStep::DeclarativeDynamicRouter { .. }
            | BuilderStep::DeclarativeRoutingSlip { .. }
            | BuilderStep::Aggregate { .. }
            | BuilderStep::WireTap { .. }
            | BuilderStep::DeclarativeLog { .. }
            | BuilderStep::Script { .. }
            | BuilderStep::DynamicRouter { .. }
            | BuilderStep::RoutingSlip { .. }
            | BuilderStep::RecipientList { .. }
            | BuilderStep::DeclarativeRecipientList { .. }
            | BuilderStep::Delay { .. }
            | BuilderStep::Enrich { .. }
            | BuilderStep::PollEnrich { .. }
            | BuilderStep::Validate { .. }
            | BuilderStep::ClaimCheck { .. }
            | BuilderStep::Sampling { .. }
            | BuilderStep::Sort { .. }
            | BuilderStep::CacheInvalidate { .. }
            | BuilderStep::CacheClear { .. }
            | BuilderStep::CacheStats { .. }
            | BuilderStep::CachePeekStale { .. }
            | BuilderStep::Resequence { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;
    use camel_api::declarative::LanguageExpressionDef;

    fn echo_decl(methods: Option<Vec<String>>) -> BeanDeclDoc {
        BeanDeclDoc {
            kind: BeanKindDoc::Echo,
            methods,
            config: None,
        }
    }

    /// An explicit `methods` declaration wins regardless of what the routes
    /// actually invoke.
    #[test]
    fn stub_from_decl_explicit_methods_uses_declaration() {
        let decl = echo_decl(Some(vec!["a".to_string(), "b".to_string()]));
        let invoked = vec!["x".to_string(), "y".to_string()];
        let bean = stub_from_decl("enricher", &decl, &invoked);
        assert_eq!(bean.methods(), vec!["a".to_string(), "b".to_string()]);
    }

    /// Without `methods`, the invoked list is the allowlist: deduplicated,
    /// order of first appearance.
    #[test]
    fn stub_from_decl_wildcard_dedupes_invoked() {
        let decl = echo_decl(None);
        let invoked = vec!["m1".to_string(), "m2".to_string(), "m1".to_string()];
        let bean = stub_from_decl("gate", &decl, &invoked);
        assert_eq!(bean.methods(), vec!["m1".to_string(), "m2".to_string()]);
    }

    /// Bean calls inside a `Cache.on_miss` child pipeline and the circuit
    /// breaker fallback are collected alongside top-level steps.
    #[test]
    fn collect_bean_calls_walks_fallback_and_cache_on_miss() {
        let def = RouteDefinition::new(
            "direct:start",
            vec![
                BuilderStep::Bean {
                    name: "x".to_string(),
                    method: "m".to_string(),
                },
                BuilderStep::Cache {
                    repository: None,
                    key: LanguageExpressionDef {
                        language: "simple".to_string(),
                        source: "${body}".to_string(),
                    },
                    ttl: None,
                    max_entry_bytes: None,
                    coalesce_misses: false,
                    on_miss: vec![BuilderStep::Bean {
                        name: "y".to_string(),
                        method: "n".to_string(),
                    }],
                },
            ],
        )
        .with_circuit_breaker_fallback(vec![BuilderStep::Bean {
            name: "z".to_string(),
            method: "o".to_string(),
        }]);
        let calls = collect_bean_calls(&[def]);
        assert_eq!(
            calls,
            vec![
                ("x".to_string(), "m".to_string()),
                ("y".to_string(), "n".to_string()),
                ("z".to_string(), "o".to_string()),
            ]
        );
    }

    /// A `fail` stub without `config.message` fails with exactly
    /// `fail bean <name>`.
    #[tokio::test]
    async fn fail_default_message_exact() {
        let decl = BeanDeclDoc {
            kind: BeanKindDoc::Fail,
            methods: None,
            config: None,
        };
        let bean = stub_from_decl("gate", &decl, &[]);
        let mut exchange = Exchange::new(Message::new(Body::Text("orig".to_string())));
        let err = bean
            .call("check", &mut exchange)
            .await
            .expect_err("fail stub must error"); // allow-unwrap
        match err {
            CamelError::ProcessorError(message) => {
                assert_eq!(message, "fail bean gate");
            }
            other => panic!("expected ProcessorError, got {other}"),
        }
    }
}
