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
};
use crate::lifecycle::adapters::step_resolution::{
    FunctionStagingMode, await_eval, compile_language_expression, resolve_language,
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
        _registry: &StepCompilerRegistry,
    ) -> StepCompileResult {
        match step {
            // ── Pre-built processor ──
            BuilderStep::Processor(svc) => StepCompileResult::Matched(Ok(CompiledStep::Process {
                processor: svc,
                body_contract: None,
            })),

            // ── Stop ──
            BuilderStep::Stop => StepCompileResult::Matched(Ok(CompiledStep::Stop)),

            // ── Delay ──
            BuilderStep::Delay { config } => {
                let svc = camel_processor::delayer::DelayerService::new(config);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            // ── Static Log ──
            BuilderStep::Log { level, message } => {
                let svc = LogProcessor::new(level, message);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
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
                }))
            }

            // ── SetHeader (declarative) ──
            BuilderStep::DeclarativeSetHeader { key, value } => match value {
                ValueSourceDef::Literal(value) => {
                    let svc = camel_processor::SetHeader::new(IdentityProcessor, key, value);
                    StepCompileResult::Matched(Ok(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
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
