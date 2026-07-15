//! Core step compilers: Log, SetBody, SetHeader, SetProperty, Bean, Script, etc.
//!
//! These are the simplest compilers — none recursively compile child steps.

use std::sync::Arc;

use camel_api::{BoxProcessor, CamelError, Exchange, IdentityProcessor, Value, body::Body};
use camel_language_api::LanguageError;
use camel_processor::claim_check::ClaimCheckFilter;
use camel_processor::{LogProcessor, log::DynamicLog, script_mutator::ScriptMutator, set_property};
use tracing::warn;

use super::{
    CompilationContext, CompileOutcome, CompiledStep, StepCompiler, StepCompilerRegistry,
    pack_lifecycles,
};
use crate::lifecycle::adapters::step_resolution::{
    FunctionStagingMode, await_eval, compile_filter_predicate, compile_language_expression,
    compile_message_id_expression, compile_sort_expression, resolve_language,
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
    ) -> Result<CompileOutcome, CamelError> {
        match step {
            // ── Pre-built processor ──
            BuilderStep::Processor(op) => Ok(CompileOutcome::Matched(CompiledStep::Process {
                processor: op.0,
                body_contract: None,
                lifecycle: None,
            })),

            // ── Stop ──
            BuilderStep::Stop => Ok(CompileOutcome::Matched(CompiledStep::Stop)),

            // ── Resequence (N4: must be top-level only) ──
            BuilderStep::Resequence { .. } => Err(CamelError::RouteError(
                "resequence must be a top-level step".into(),
            )),

            // ── Delay ──
            BuilderStep::Delay { config } => {
                let svc = camel_processor::delayer::DelayerService::new(config);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Validate ──
            BuilderStep::Validate { predicate } => {
                let predicate_arc = compile_filter_predicate(ctx.languages, &predicate)?;
                let expression_source = predicate.source.clone();
                let svc = camel_processor::ValidateService::from_predicate(
                    predicate_arc,
                    expression_source,
                );
                Ok(CompileOutcome::Matched(CompiledStep::Process {
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

                let repo = ctx
                    .idempotent_repositories
                    .get(&repository)
                    .ok_or_else(|| {
                        CamelError::ComponentNotFound(format!(
                            "idempotent_consumer: repository '{repository}' is not registered"
                        ))
                    })?;
                let message_id = compile_message_id_expression(ctx.languages, &expression)?;
                let (child_segments, child_lifecycles) =
                    ctx.compile_children_segments(steps, registry)?;
                let child_pipeline = compose_outcome_segment(child_segments);
                let svc = camel_processor::IdempotentConsumerSegment::new(
                    repo,
                    message_id,
                    child_pipeline,
                    eager,
                    remove_on_failure,
                );
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(svc)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(child_lifecycles),
                }))
            }

            // ── Static Log ──
            BuilderStep::Log { level, message } => {
                let svc = LogProcessor::new(level, message);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
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
                let expression = compile_language_expression(ctx.languages, &expression)?;
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
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── SetHeader (declarative) ──
            BuilderStep::DeclarativeSetHeader { key, value } => match value {
                ValueSourceDef::Literal(value) => {
                    let svc = camel_processor::SetHeader::new(IdentityProcessor, key, value);
                    Ok(CompileOutcome::Matched(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
                ValueSourceDef::Expression(expression) => {
                    let expression = compile_language_expression(ctx.languages, &expression)?;
                    let svc = camel_processor::DynamicSetHeader::new(
                        IdentityProcessor,
                        key,
                        move |exchange: &Exchange| await_eval(&expression, exchange),
                    );
                    Ok(CompileOutcome::Matched(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
            },

            // ── SetHeaderIfAbsent (declarative, internal-only) ──
            BuilderStep::DeclarativeSetHeaderIfAbsent { key, value } => match value {
                ValueSourceDef::Literal(value) => {
                    let svc =
                        camel_processor::SetHeaderIfAbsent::new(IdentityProcessor, key, value);
                    Ok(CompileOutcome::Matched(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
                ValueSourceDef::Expression(expression) => {
                    let expression = compile_language_expression(ctx.languages, &expression)?;
                    let svc = camel_processor::DynamicSetHeaderIfAbsent::new(
                        IdentityProcessor,
                        key,
                        move |exchange: &Exchange| await_eval(&expression, exchange),
                    );
                    Ok(CompileOutcome::Matched(CompiledStep::Process {
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
                    Ok(CompileOutcome::Matched(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
                ValueSourceDef::Expression(expression) => {
                    let expression = compile_language_expression(ctx.languages, &expression)?;
                    let svc = camel_processor::DynamicSetProperty::new(
                        IdentityProcessor,
                        key,
                        move |exchange: &Exchange| await_eval(&expression, exchange),
                    );
                    Ok(CompileOutcome::Matched(CompiledStep::Process {
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
                    Ok(CompileOutcome::Matched(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
                ValueSourceDef::Expression(expression) => {
                    let expression = compile_language_expression(ctx.languages, &expression)?;
                    let svc = camel_processor::SetBody::new(
                        IdentityProcessor,
                        move |exchange: &Exchange| {
                            let value = await_eval(&expression, exchange);
                            value_to_body(value)
                        },
                    );
                    Ok(CompileOutcome::Matched(CompiledStep::Process {
                        processor: BoxProcessor::new(svc),
                        body_contract: None,
                        lifecycle: None,
                    }))
                }
            },

            // ── Declarative Script (graceful degradation: try mutating, fallback to SetBody) ──
            BuilderStep::DeclarativeScript { expression } => {
                let lang = resolve_language(ctx.languages, &expression.language)?;
                match lang.create_mutating_expression(&expression.source) {
                    Ok(mut_expr) => Ok(CompileOutcome::Matched(CompiledStep::Process {
                        processor: BoxProcessor::new(ScriptMutator::new(mut_expr)),
                        body_contract: None,
                        lifecycle: None,
                    })),
                    Err(LanguageError::NotSupported { .. }) => {
                        // Graceful degradation: fall back to read-only Expression → SetBody
                        let expression = compile_language_expression(ctx.languages, &expression)?;
                        let svc = camel_processor::SetBody::new(
                            IdentityProcessor,
                            move |exchange: &Exchange| {
                                let value = await_eval(&expression, exchange);
                                value_to_body(value)
                            },
                        );
                        Ok(CompileOutcome::Matched(CompiledStep::Process {
                            processor: BoxProcessor::new(svc),
                            body_contract: None,
                            lifecycle: None,
                        }))
                    }
                    Err(e) => Err(CamelError::RouteError(format!(
                        "Failed to create mutating expression for language '{}': {}",
                        expression.language, e
                    ))),
                }
            }

            // ── Function step ──
            BuilderStep::DeclarativeFunction { mut definition } => {
                let Some(invoker) = ctx.function_invoker.clone() else {
                    return Err(CamelError::Config(
                        "function: step requires FunctionRuntimeService registered via with_lifecycle"
                            .into(),
                    ));
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
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(step),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Bean invocation ──
            BuilderStep::Bean { name, method } => {
                let beans = ctx
                    .beans
                    .lock()
                    .map_err(|_| CamelError::ProcessorError("beans mutex poisoned".into()))?;
                let bean = beans.get(&name).ok_or_else(|| {
                    CamelError::ProcessorError(format!("Bean not found: {}", name))
                })?;
                let processor = tower::service_fn(move |mut exchange: Exchange| {
                    let bean = Arc::clone(&bean);
                    let method = method.clone();
                    async move {
                        // R3-L6: method-allowlist check (mirrors BeanRegistry::invoke).
                        if !bean.methods().iter().any(|m| m == &method) {
                            return Err(
                                camel_bean::BeanError::MethodNotFound(method.clone()).into()
                            );
                        }
                        bean.call(&method, &mut exchange).await?;
                        Ok(exchange)
                    }
                });
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(processor),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Script (hard error on NotSupported) ──
            BuilderStep::Script { language, script } => {
                let lang = resolve_language(ctx.languages, &language)?;
                match lang.create_mutating_expression(&script) {
                    Ok(mut_expr) => Ok(CompileOutcome::Matched(CompiledStep::Process {
                        processor: BoxProcessor::new(ScriptMutator::new(mut_expr)),
                        body_contract: None,
                        lifecycle: None,
                    })),
                    Err(LanguageError::NotSupported {
                        feature,
                        language: ref lang_name,
                    }) => Err(CamelError::RouteError(format!(
                        "Language '{}' does not support {} (required for .script() step)",
                        lang_name, feature
                    ))),
                    Err(e) => Err(CamelError::RouteError(format!(
                        "Failed to create mutating expression for language '{}': {}",
                        language, e
                    ))),
                }
            }

            // ── ClaimCheck ──
            BuilderStep::ClaimCheck {
                repository,
                operation,
                key,
                filter,
            } => {
                let repo = ctx
                    .claim_check_repositories
                    .get(&repository)
                    .ok_or_else(|| {
                        CamelError::ComponentNotFound(format!(
                            "claim_check: repository '{repository}' is not registered"
                        ))
                    })?;
                let op = match operation.as_str() {
                    "set" => camel_processor::claim_check::ClaimCheckOp::Set,
                    "get" => camel_processor::claim_check::ClaimCheckOp::Get,
                    "get_and_remove" => camel_processor::claim_check::ClaimCheckOp::GetAndRemove,
                    "push" => camel_processor::claim_check::ClaimCheckOp::Push,
                    "pop" => camel_processor::claim_check::ClaimCheckOp::Pop,
                    other => {
                        return Err(CamelError::RouteError(format!(
                            "claim_check: unsupported operation '{other}'"
                        )));
                    }
                };
                let key_expr = crate::lifecycle::adapters::step_resolution::compile_key_expression(
                    ctx.languages,
                    &key,
                )?;
                let mut svc = camel_processor::ClaimCheckService::new(repo, op, key_expr);
                if let Some(filter_str) = filter {
                    let f = ClaimCheckFilter::parse(&filter_str).map_err(|e| {
                        CamelError::RouteError(format!(
                            "claim_check: invalid filter '{filter_str}': {e}"
                        ))
                    })?;
                    svc = svc.with_filter(f);
                }
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Sampling (Process-mode, stateless, no StepLifecycle) ──
            BuilderStep::Sampling { period } => {
                let svc = camel_processor::SamplingService::new(period);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Sort (Process-mode, stateless) ──
            BuilderStep::Sort {
                expression,
                reverse,
            } => {
                let sort_expr = compile_sort_expression(ctx.languages, &expression)?;
                let svc = camel_processor::SortService::new(sort_expr, reverse);
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle: None,
                }))
            }

            // ── Everything else: not handled ──
            _ => Ok(CompileOutcome::NotHandled(step)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use camel_api::{CamelError, Exchange, LanguageExpressionDef, Message, ProducerContext};
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
        let claim_check_repositories = crate::ClaimCheckRegistry::new();

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
            claim_check_repositories: &claim_check_repositories,
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
            .expect("compilation should succeed")
            .expect("should match");

        assert!(
            matches!(compiled, CompiledStep::Segment { .. }),
            "Expected CompiledStep::Segment, got {compiled:?}"
        );
    }

    #[test]
    fn nested_resequence_rejected_at_compile_children() {
        use camel_api::CamelError;

        // N4: any Resequence reaching the compiler registry (i.e., not intercepted
        // at the top-level by the route controller) must be rejected.
        let mut reg = StepCompilerRegistry::new();
        reg.register(Box::new(super::CoreCompiler));

        let step = BuilderStep::Resequence {
            policy_config: camel_api::ResequencePolicyConfig::default(),
        };

        let result = reg.compile_step(step, 0, &dummy_context());
        let err = result.expect_err("compiler should produce an error for nested resequence");
        assert!(
            matches!(err, CamelError::RouteError(_)),
            "should be RouteError, got {err:?}"
        );
        assert!(
            err.to_string().contains("top-level"),
            "error should mention 'top-level', got: {err}"
        );
    }

    /// A bean whose `call` accepts any method, but whose `methods()` allowlist
    /// is restricted. Used to verify the compiler-level allowlist check fires
    /// before `bean.call()` is reached.
    struct PermissiveBean;

    #[async_trait::async_trait]
    impl camel_bean::BeanProcessor for PermissiveBean {
        async fn call(
            &self,
            _method: &str,
            _exchange: &mut camel_api::Exchange,
        ) -> Result<(), CamelError> {
            // Always succeeds — the allowlist check must catch unauthorized
            // methods before we get here.
            Ok(())
        }

        fn methods(&self) -> Vec<String> {
            vec!["handle".to_string()]
        }
    }

    /// R3-L6: bean step compiler must reject methods not in the allowlist
    /// before invoking `bean.call()`.
    #[tokio::test]
    async fn bean_step_rejects_method_not_in_allowlist() {
        use tower::ServiceExt;

        // Register a bean with allowlist ["handle"].
        let beans = BeanRegistry::new();
        beans.register("test_bean", PermissiveBean).unwrap();
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(beans));

        // Build a CompilationContext referencing the bean registry.
        let producer_ctx: &'static ProducerContext =
            Box::leak(Box::new(ProducerContext::default()));
        let staging: &'static FunctionStagingMode =
            Box::leak(Box::new(FunctionStagingMode::DirectAdd));
        let languages: &'static SharedLanguageRegistry =
            Box::leak(Box::new(Arc::new(Mutex::new(HashMap::new()))));
        let beans_ref: &'static Arc<Mutex<BeanRegistry>> = Box::leak(Box::new(beans));
        let idempotent: &'static crate::IdempotentRegistry =
            Box::leak(Box::new(crate::IdempotentRegistry::new()));
        let claim_check: &'static crate::ClaimCheckRegistry =
            Box::leak(Box::new(crate::ClaimCheckRegistry::new()));

        let ctx = CompilationContext {
            producer_ctx,
            rt: Arc::new(NoopRuntimeObservability),
            languages,
            beans: beans_ref,
            function_invoker: None,
            component_ctx: Arc::new(NoOpComponentContext),
            route_id: None,
            staging_mode: staging,
            idempotent_repositories: idempotent,
            claim_check_repositories: claim_check,
        };

        let mut reg = StepCompilerRegistry::new();
        reg.register(Box::new(super::CoreCompiler));

        // Compile a Bean step calling an unauthorized method.
        let step = BuilderStep::Bean {
            name: "test_bean".into(),
            method: "unauthorized_method".into(),
        };
        let result = reg.compile_step(step, 0, &ctx);
        let compiled = result
            .expect("compilation should succeed")
            .expect("should match");

        // Extract the processor and invoke it.
        let processor = match compiled {
            CompiledStep::Process { processor, .. } => processor,
            other => panic!("expected CompiledStep::Process, got {other:?}"),
        };

        let exchange = Exchange::new(Message::default());
        let result = processor.oneshot(exchange).await;

        assert!(
            result.is_err(),
            "bean step should reject method not in allowlist"
        );
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unauthorized_method") && msg.contains("not found"),
            "error should mention method name and 'not found', got: {msg}"
        );
    }

    /// Build a minimal `CompilationContext` for unit tests.
    fn dummy_context() -> CompilationContext<'static> {
        use std::collections::HashMap;
        use std::sync::Mutex;

        use camel_bean::BeanRegistry;
        use camel_component_api::{NoOpComponentContext, test_support::NoopRuntimeObservability};

        use crate::lifecycle::adapters::step_resolution::FunctionStagingMode;

        // Leak on-stack data to satisfy 'static lifetime (test-only safe).
        // SAFETY: these leaks are bounded — used once in a short-lived test.
        let producer_ctx: &'static ProducerContext =
            Box::leak(Box::new(ProducerContext::default()));
        let staging: &'static FunctionStagingMode =
            Box::leak(Box::new(FunctionStagingMode::DirectAdd));
        let languages: &'static SharedLanguageRegistry =
            Box::leak(Box::new(Arc::new(Mutex::new(HashMap::new()))));
        let beans: &'static Arc<Mutex<BeanRegistry>> =
            Box::leak(Box::new(Arc::new(Mutex::new(BeanRegistry::new()))));
        let idempotent: &'static crate::IdempotentRegistry =
            Box::leak(Box::new(crate::IdempotentRegistry::new()));
        let claim_check: &'static crate::ClaimCheckRegistry =
            Box::leak(Box::new(crate::ClaimCheckRegistry::new()));

        CompilationContext {
            producer_ctx,
            rt: Arc::new(NoopRuntimeObservability),
            languages,
            beans,
            function_invoker: None,
            component_ctx: Arc::new(NoOpComponentContext),
            route_id: None,
            staging_mode: staging,
            idempotent_repositories: idempotent,
            claim_check_repositories: claim_check,
        }
    }
}
