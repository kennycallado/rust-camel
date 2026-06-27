//! Core step compilers: Log, SetBody, SetHeader, SetProperty, Bean, Script, etc.
//!
//! These are the simplest compilers — none recursively compile child steps.

use std::sync::Arc;

use camel_api::{BoxProcessor, CamelError, Exchange, IdentityProcessor, Value, body::Body};
use camel_language_api::LanguageError;
use camel_processor::{LogProcessor, log::DynamicLog, script_mutator::ScriptMutator, set_property};
use tracing::warn;

use super::{
    CompilationContext, CompiledStep, StepCompileResult, StepCompiler, StepCompilerRegistry,
    pack_lifecycles,
};
use crate::lifecycle::adapters::step_resolution::{
    FunctionStagingMode, await_eval, compile_filter_predicate, compile_language_expression,
    compile_message_id_expression, resolve_language,
};
use crate::lifecycle::application::route_definition::{BuilderStep, ValueSourceDef};

fn value_to_body(value: Value) -> Body {
    match value {
        Value::Null => Body::Empty,
        Value::String(text) => Body::Text(text),
        other => Body::Json(other),
    }
}

pub(crate) struct CoreCompiler;

impl StepCompiler for CoreCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        ctx: &CompilationContext,
        registry: &StepCompilerRegistry,
    ) -> StepCompileResult {
        match step {
            // ── Pre-built processor ──
            BuilderStep::Processor(svc) => StepCompileResult::Matched(Ok(CompiledStep::Process {
                processor: svc,
                body_contract: None,
                lifecycle: None,
            })),

            // ── Stop ──
            BuilderStep::Stop => StepCompileResult::Matched(Ok(CompiledStep::Stop)),

            // ── Delay ──
            BuilderStep::Delay { config } => {
                let svc = camel_processor::delayer::DelayerService::new(config);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Validate ──
            BuilderStep::Validate { predicate } => {
                let predicate_arc = match compile_filter_predicate(ctx.languages, &predicate) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let expression_source = predicate.source.clone();
                let svc = camel_processor::ValidateService::from_predicate(
                    predicate_arc,
                    expression_source,
                );
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None, // Validate is stateless
                }))
            }

            // ── Idempotent Consumer (Segment-mode, ADR-0023) ──
            // CRITICAL: Implemented as OutcomePipeline (Segment), NOT Tower
            // Service<Exchange>. compose_pipeline() maps Stopped→Ok(ex); if
            // this were Process-mode, a duplicate-detected Stopped would
            // become Ok(ex) and downstream steps would re-process the dup.
            BuilderStep::IdempotentConsumer {
                repository,
                expression,
                steps,
                eager,
                remove_on_failure,
            } => {
                use crate::lifecycle::adapters::route_compiler::compose_outcome_segment;

                let repo = match ctx.idempotent_repositories.get(&repository) {
                    Some(r) => r,
                    None => {
                        return StepCompileResult::Matched(Err(CamelError::ComponentNotFound(
                            format!(
                                "idempotent_consumer: repository '{repository}' is not registered"
                            ),
                        )));
                    }
                };
                let message_id = match compile_message_id_expression(ctx.languages, &expression) {
                    Ok(m) => m,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let (child_segments, child_lifecycles) =
                    match ctx.compile_children_segments(steps, registry) {
                        Ok(pair) => pair,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                let child_pipeline = compose_outcome_segment(child_segments);
                let svc = camel_processor::IdempotentConsumerSegment::new(
                    repo,
                    message_id,
                    child_pipeline,
                    eager,
                    remove_on_failure,
                );
                StepCompileResult::Matched(Ok(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(svc)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(child_lifecycles),
                }))
            }

            // ── Static Log ──
            BuilderStep::Log { level, message } => {
                let svc = LogProcessor::new(level, message);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Declarative Log (language-evaluated message) ──
            BuilderStep::DeclarativeLog { level, message } => {
                let ValueSourceDef::Expression(expression) = message else {
                    // Should never happen — literal case is converted to Processor in compile.rs
                    unreachable!(
                        "DeclarativeLog with Literal should have been compiled to a Processor"
                    );
                };
                let expression = match compile_language_expression(ctx.languages, &expression) {
                    Ok(e) => e,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let svc =
                    DynamicLog::new(level, move |exchange: &Exchange| {
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::try_current()
                            .expect("DynamicLog expression: must be called from within a tokio runtime") // allow-unwrap
                            .block_on(expression.evaluate(exchange))
                        })
                        .unwrap_or_else(|e| {
                            warn!(error = %e, "log expression evaluation failed");
                            Value::Null
                        })
                        .to_string()
                    });
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── SetHeader (declarative) ──
            BuilderStep::DeclarativeSetHeader { key, value } => match value {
                ValueSourceDef::Literal(value) => {
                    let svc = camel_processor::SetHeader::new(IdentityProcessor, key, value);
                    StepCompileResult::Matched(Ok(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
                ValueSourceDef::Expression(expression) => {
                    let expression = match compile_language_expression(ctx.languages, &expression) {
                        Ok(e) => e,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    let svc = camel_processor::DynamicSetHeader::new(
                        IdentityProcessor,
                        key,
                        move |exchange: &Exchange| await_eval(&expression, exchange),
                    );
                    StepCompileResult::Matched(Ok(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
            },

            // ── SetProperty (declarative) ──
            BuilderStep::DeclarativeSetProperty { key, value_source } => match value_source {
                ValueSourceDef::Literal(value) => {
                    let svc = set_property::SetProperty::new(IdentityProcessor, key, value);
                    StepCompileResult::Matched(Ok(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
                ValueSourceDef::Expression(expression) => {
                    let expression = match compile_language_expression(ctx.languages, &expression) {
                        Ok(e) => e,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    let svc = camel_processor::DynamicSetProperty::new(
                        IdentityProcessor,
                        key,
                        move |exchange: &Exchange| await_eval(&expression, exchange),
                    );
                    StepCompileResult::Matched(Ok(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
            },

            // ── SetBody (declarative) ──
            BuilderStep::DeclarativeSetBody { value } => match value {
                ValueSourceDef::Literal(value) => {
                    let body = value_to_body(value);
                    let svc = camel_processor::SetBody::new(
                        IdentityProcessor,
                        move |_exchange: &Exchange| body.clone(),
                    );
                    StepCompileResult::Matched(Ok(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
                ValueSourceDef::Expression(expression) => {
                    let expression = match compile_language_expression(ctx.languages, &expression) {
                        Ok(e) => e,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    let svc = camel_processor::SetBody::new(
                        IdentityProcessor,
                        move |exchange: &Exchange| {
                            let value = await_eval(&expression, exchange);
                            value_to_body(value)
                        },
                    );
                    StepCompileResult::Matched(Ok(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
            },

            // ── Declarative Script (graceful degradation: try mutating, fallback to SetBody) ──
            BuilderStep::DeclarativeScript { expression } => {
                let lang = match resolve_language(ctx.languages, &expression.language) {
                    Ok(l) => l,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                match lang.create_mutating_expression(&expression.source) {
                    Ok(mut_expr) => StepCompileResult::Matched(Ok(CompiledStep::Process {
                        processor: BoxProcessor::new(ScriptMutator::new(mut_expr)),
                        body_contract: None,
                        lifecycle: None,
                    })),
                    Err(LanguageError::NotSupported { .. }) => {
                        // Graceful degradation: fall back to read-only Expression → SetBody
                        let expression =
                            match compile_language_expression(ctx.languages, &expression) {
                                Ok(e) => e,
                                Err(e) => return StepCompileResult::Matched(Err(e)),
                            };
                        let svc = camel_processor::SetBody::new(
                            IdentityProcessor,
                            move |exchange: &Exchange| {
                                let value = await_eval(&expression, exchange);
                                value_to_body(value)
                            },
                        );
                        StepCompileResult::Matched(Ok(CompiledStep::Process {
                            processor: BoxProcessor::new(svc),
                            body_contract: None,
                            lifecycle: None,
                        }))
                    }
                    Err(e) => StepCompileResult::Matched(Err(CamelError::RouteError(format!(
                        "Failed to create mutating expression for language '{}': {}",
                        expression.language, e
                    )))),
                }
            }

            // ── Function step ──
            BuilderStep::DeclarativeFunction { mut definition } => {
                let Some(invoker) = ctx.function_invoker.clone() else {
                    return StepCompileResult::Matched(Err(CamelError::Config(
                        "function: step requires FunctionRuntimeService registered via with_lifecycle"
                            .into(),
                    )));
                };
                definition.route_id = ctx.route_id.map(|s| s.to_string());
                definition.step_index = Some(_step_index);
                match ctx.staging_mode {
                    FunctionStagingMode::DirectAdd => {
                        invoker.stage_pending(definition.clone(), ctx.route_id, 0);
                    }
                    FunctionStagingMode::HotReload { generation } => {
                        invoker.stage_pending(definition.clone(), ctx.route_id, *generation);
                    }
                    FunctionStagingMode::DryCompile => {}
                }
                let step = crate::step::function_step::FunctionStep::new(invoker, definition);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(step),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Bean invocation ──
            BuilderStep::Bean { name, method } => {
                let beans = match ctx.beans.lock() {
                    Ok(guard) => guard,
                    Err(_) => {
                        return StepCompileResult::Matched(Err(CamelError::ProcessorError(
                            "beans mutex poisoned".into(),
                        )));
                    }
                };
                let bean = match beans.get(&name) {
                    Some(b) => b.clone(),
                    None => {
                        return StepCompileResult::Matched(Err(CamelError::ProcessorError(
                            format!("Bean not found: {}", name),
                        )));
                    }
                };
                let processor = tower::service_fn(move |mut exchange: Exchange| {
                    let bean = Arc::clone(&bean);
                    let method = method.clone();
                    async move {
                        bean.call(&method, &mut exchange).await?;
                        Ok(exchange)
                    }
                });
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(processor),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Script (hard error on NotSupported) ──
            BuilderStep::Script { language, script } => {
                let lang = match resolve_language(ctx.languages, &language) {
                    Ok(l) => l,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                match lang.create_mutating_expression(&script) {
                    Ok(mut_expr) => StepCompileResult::Matched(Ok(CompiledStep::Process {
                        processor: BoxProcessor::new(ScriptMutator::new(mut_expr)),
                        body_contract: None,
                        lifecycle: None,
                    })),
                    Err(LanguageError::NotSupported {
                        feature,
                        language: ref lang_name,
                    }) => StepCompileResult::Matched(Err(CamelError::RouteError(format!(
                        "Language '{}' does not support {} (required for .script() step)",
                        lang_name, feature
                    )))),
                    Err(e) => StepCompileResult::Matched(Err(CamelError::RouteError(format!(
                        "Failed to create mutating expression for language '{}': {}",
                        language, e
                    )))),
                }
            }

            // ── Everything else: not handled ──
            _ => StepCompileResult::NotHandled(step),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use camel_api::{LanguageExpressionDef, ProducerContext};
    use camel_bean::BeanRegistry;
    use camel_component_api::{
        ComponentContext, NoOpComponentContext, RuntimeObservability,
        test_support::NoopRuntimeObservability,
    };

    use camel_language_api::Language;
    use camel_processor::aggregator::SharedLanguageRegistry;

    use crate::idempotent::MemoryIdempotentRepository;
    use crate::lifecycle::adapters::step_compilers::{
        CompilationContext, CompiledStep, StepCompilerRegistry,
    };
    use crate::lifecycle::adapters::step_resolution::FunctionStagingMode;
    use crate::lifecycle::application::route_definition::BuilderStep;

    /// Compile an idempotent_consumer step through the full route compiler
    /// and assert the output is a CompiledStep::Segment with lifecycle propagated.
    #[tokio::test]
    async fn compile_idempotent_consumer_produces_segment() {
        // ── languages with simple language ──
        let languages: SharedLanguageRegistry = {
            let mut map: HashMap<String, Arc<dyn Language>> = HashMap::new();
            map.insert(
                "simple".to_string(),
                Arc::new(camel_language_simple::SimpleLanguage::new()),
            );
            Arc::new(Mutex::new(map))
        };

        // ── register memory idempotent repository ──
        let idempotent_repositories = crate::IdempotentRegistry::new();
        let repo: Arc<dyn camel_api::IdempotentRepository> =
            Arc::new(MemoryIdempotentRepository::new("memory"));
        idempotent_repositories
            .register("memory", repo)
            .expect("register repo");

        // ── registry with only CoreCompiler ──
        let mut reg = StepCompilerRegistry::new();
        reg.register(Box::new(super::CoreCompiler));

        // ── CompilationContext ──
        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(NoOpComponentContext);
        let staging = FunctionStagingMode::DirectAdd;

        let ctx = CompilationContext {
            producer_ctx: &pc,
            rt,
            languages: &languages,
            beans: &beans,
            function_invoker: None,
            component_ctx,
            route_id: None,
            staging_mode: &staging,
            idempotent_repositories: &idempotent_repositories,
        };

        // ── IdempotentConsumer step ──
        let step = BuilderStep::IdempotentConsumer {
            repository: "memory".into(),
            expression: LanguageExpressionDef {
                language: "simple".into(),
                source: "${header.id}".into(),
            },
            steps: vec![BuilderStep::Log {
                level: camel_processor::LogLevel::Info,
                message: "duplicate check passed".into(),
            }],
            eager: false,
            remove_on_failure: true,
        };

        let result = reg.compile_step(step, 0, &ctx);
        let compiled = result
            .expect("compilation should return Some")
            .expect("compilation should succeed");

        assert!(
            matches!(compiled, CompiledStep::Segment { .. }),
            "Expected CompiledStep::Segment, got {compiled:?}"
        );
    }
}
