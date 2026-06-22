//! Control-flow step compilers: Loop, DeclarativeLoop, Filter, DeclarativeFilter, Choice, DeclarativeChoice.
//!
//! These are the structural EIP patterns that evaluate predicates to decide
//! which sub-pipeline executes. All recursively compile child steps.

use camel_api::{
    BoxProcessor, CamelError,
    loop_eip::{LoopConfig, LoopMode},
};

use camel_processor::do_try::{CatchClause, CatchMatcher, DoTryService};
use camel_processor::{ChoiceService, WhenClause};

use super::{
    CompilationContext, CompiledStep, StepCompileResult, StepCompiler, StepCompilerRegistry,
};
use crate::lifecycle::adapters::route_compiler::compose_pipeline;
use crate::lifecycle::adapters::step_resolution::compile_filter_predicate;
use crate::lifecycle::application::route_definition::BuilderStep;

pub(crate) struct ControlFlowCompiler;

impl StepCompiler for ControlFlowCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        ctx: &CompilationContext,
        registry: &StepCompilerRegistry,
    ) -> StepCompileResult {
        match step {
            // ── Loop (programmatic) ──
            BuilderStep::Loop { config, steps } => {
                let sub_pairs = match ctx.compile_children(steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_processors: Vec<CompiledStep> = sub_pairs
                    .into_iter()
                    .map(|c| match c {
                        CompiledStep::Process { .. } => c,
                        CompiledStep::Stop => CompiledStep::Process {
                            processor: BoxProcessor::new(camel_processor::StopService),
                            body_contract: None,
                        },
                    })
                    .collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::loop_eip::LoopService::new(config, sub_pipeline);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            // ── DeclarativeLoop ──
            BuilderStep::DeclarativeLoop {
                count,
                while_predicate,
                steps,
            } => {
                let mode = match (count, while_predicate) {
                    (Some(n), None) => LoopMode::Count(n),
                    (None, Some(pred)) => {
                        let predicate = match compile_filter_predicate(ctx.languages, &pred) {
                            Ok(p) => p,
                            Err(e) => return StepCompileResult::Matched(Err(e)),
                        };
                        LoopMode::While(predicate)
                    }
                    (Some(_), Some(_)) => {
                        return StepCompileResult::Matched(Err(CamelError::RouteError(
                            "loop: cannot specify both 'count' and 'while'".into(),
                        )));
                    }
                    (None, None) => {
                        return StepCompileResult::Matched(Err(CamelError::RouteError(
                            "loop: must specify either 'count' or 'while'".into(),
                        )));
                    }
                };
                let sub_pairs = match ctx.compile_children(steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_processors: Vec<CompiledStep> = sub_pairs
                    .into_iter()
                    .map(|c| match c {
                        CompiledStep::Process { .. } => c,
                        CompiledStep::Stop => CompiledStep::Process {
                            processor: BoxProcessor::new(camel_processor::StopService),
                            body_contract: None,
                        },
                    })
                    .collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let config = LoopConfig { mode };
                let svc = camel_processor::loop_eip::LoopService::new(config, sub_pipeline);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            // ── Filter (programmatic) ──
            BuilderStep::Filter { predicate, steps } => {
                let sub_pairs = match ctx.compile_children(steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_processors: Vec<CompiledStep> = sub_pairs
                    .into_iter()
                    .map(|c| match c {
                        CompiledStep::Process { .. } => c,
                        CompiledStep::Stop => CompiledStep::Process {
                            processor: BoxProcessor::new(camel_processor::StopService),
                            body_contract: None,
                        },
                    })
                    .collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::FilterService::from_predicate(predicate, sub_pipeline);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            // ── DeclarativeFilter ──
            BuilderStep::DeclarativeFilter { predicate, steps } => {
                let predicate = match compile_filter_predicate(ctx.languages, &predicate) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_pairs = match ctx.compile_children(steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_processors: Vec<CompiledStep> = sub_pairs
                    .into_iter()
                    .map(|c| match c {
                        CompiledStep::Process { .. } => c,
                        CompiledStep::Stop => CompiledStep::Process {
                            processor: BoxProcessor::new(camel_processor::StopService),
                            body_contract: None,
                        },
                    })
                    .collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::FilterService::from_predicate(predicate, sub_pipeline);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            // ── Choice (programmatic) ──
            BuilderStep::Choice { whens, otherwise } => {
                let mut when_clauses = Vec::new();
                for when_step in whens {
                    let sub_pairs = match ctx.compile_children(when_step.steps, registry) {
                        Ok(p) => p,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    let sub_processors: Vec<CompiledStep> = sub_pairs
                        .into_iter()
                        .map(|c| match c {
                            CompiledStep::Process { .. } => c,
                            CompiledStep::Stop => CompiledStep::Process {
                                processor: BoxProcessor::new(camel_processor::StopService),
                                body_contract: None,
                            },
                        })
                        .collect();
                    let pipeline = compose_pipeline(sub_processors);
                    when_clauses.push(WhenClause {
                        predicate: when_step.predicate,
                        pipeline,
                    });
                }
                let otherwise_pipeline = if let Some(otherwise_steps) = otherwise {
                    let sub_pairs = match ctx.compile_children(otherwise_steps, registry) {
                        Ok(p) => p,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    Some({
                        let sub_processors: Vec<CompiledStep> = sub_pairs
                            .into_iter()
                            .map(|c| match c {
                                CompiledStep::Process { .. } => c,
                                CompiledStep::Stop => CompiledStep::Process {
                                    processor: BoxProcessor::new(camel_processor::StopService),
                                    body_contract: None,
                                },
                            })
                            .collect();
                        compose_pipeline(sub_processors)
                    })
                } else {
                    None
                };
                let svc = ChoiceService::new(when_clauses, otherwise_pipeline);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            // ── DeclarativeChoice ──
            BuilderStep::DeclarativeChoice { whens, otherwise } => {
                let mut when_clauses = Vec::new();
                for when_step in whens {
                    let predicate =
                        match compile_filter_predicate(ctx.languages, &when_step.predicate) {
                            Ok(p) => p,
                            Err(e) => return StepCompileResult::Matched(Err(e)),
                        };
                    let sub_pairs = match ctx.compile_children(when_step.steps, registry) {
                        Ok(p) => p,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    let sub_processors: Vec<CompiledStep> = sub_pairs
                        .into_iter()
                        .map(|c| match c {
                            CompiledStep::Process { .. } => c,
                            CompiledStep::Stop => CompiledStep::Process {
                                processor: BoxProcessor::new(camel_processor::StopService),
                                body_contract: None,
                            },
                        })
                        .collect();
                    let pipeline = compose_pipeline(sub_processors);
                    when_clauses.push(WhenClause {
                        predicate,
                        pipeline,
                    });
                }
                let otherwise_pipeline = if let Some(otherwise_steps) = otherwise {
                    let sub_pairs = match ctx.compile_children(otherwise_steps, registry) {
                        Ok(p) => p,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    Some({
                        let sub_processors: Vec<CompiledStep> = sub_pairs
                            .into_iter()
                            .map(|c| match c {
                                CompiledStep::Process { .. } => c,
                                CompiledStep::Stop => CompiledStep::Process {
                                    processor: BoxProcessor::new(camel_processor::StopService),
                                    body_contract: None,
                                },
                            })
                            .collect();
                        compose_pipeline(sub_processors)
                    })
                } else {
                    None
                };
                let svc = ChoiceService::new(when_clauses, otherwise_pipeline);
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            // ── DeclarativeDoTry ──
            BuilderStep::DeclarativeDoTry {
                try_steps,
                catch,
                finally,
            } => {
                let try_pairs = match ctx.compile_children(try_steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let try_processors: Vec<BoxProcessor> = try_pairs
                    .into_iter()
                    .map(|c| match c {
                        CompiledStep::Process { processor, .. } => processor,
                        CompiledStep::Stop => BoxProcessor::new(camel_processor::StopService),
                    })
                    .collect();

                let mut catch_clauses = Vec::with_capacity(catch.len());
                for c in catch {
                    let matcher = match (c.exception, c.when) {
                        (Some(names), None) => CatchMatcher::ByVariant(names),
                        (None, Some(expr)) => {
                            let predicate = match compile_filter_predicate(ctx.languages, &expr) {
                                Ok(p) => p,
                                Err(e) => {
                                    return StepCompileResult::Matched(Err(e));
                                }
                            };
                            CatchMatcher::Predicate(predicate)
                        }
                        (Some(_), Some(_)) => {
                            return StepCompileResult::Matched(Err(CamelError::Config(
                                "doTry catch clause must specify either `exception` or \
                                 `when`, not both"
                                    .into(),
                            )));
                        }
                        (None, None) => {
                            return StepCompileResult::Matched(Err(CamelError::Config(
                                "doTry catch clause must specify either `exception` or `when`"
                                    .into(),
                            )));
                        }
                    };
                    let on_when = match c.on_when {
                        Some(expr) => match compile_filter_predicate(ctx.languages, &expr) {
                            Ok(p) => Some(p),
                            Err(e) => {
                                return StepCompileResult::Matched(Err(e));
                            }
                        },
                        None => None,
                    };
                    let clause_pairs = match ctx.compile_children(c.steps, registry) {
                        Ok(p) => p,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    let clause_processors: Vec<BoxProcessor> = clause_pairs
                        .into_iter()
                        .map(|c| match c {
                            CompiledStep::Process { processor, .. } => processor,
                            CompiledStep::Stop => BoxProcessor::new(camel_processor::StopService),
                        })
                        .collect();
                    catch_clauses.push(CatchClause {
                        matcher,
                        on_when,
                        steps: clause_processors,
                        disposition: c.disposition,
                    });
                }

                let (finally_steps, finally_on_when) = if let Some(f) = finally {
                    let on_when = match f.on_when {
                        Some(expr) => match compile_filter_predicate(ctx.languages, &expr) {
                            Ok(p) => Some(p),
                            Err(e) => {
                                return StepCompileResult::Matched(Err(e));
                            }
                        },
                        None => None,
                    };
                    let f_pairs = match ctx.compile_children(f.steps, registry) {
                        Ok(p) => p,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    let f_processors: Vec<BoxProcessor> = f_pairs
                        .into_iter()
                        .map(|c| match c {
                            CompiledStep::Process { processor, .. } => processor,
                            CompiledStep::Stop => BoxProcessor::new(camel_processor::StopService),
                        })
                        .collect();
                    (f_processors, on_when)
                } else {
                    (Vec::new(), None)
                };

                let svc = DoTryService::with_catch_and_finally(
                    try_processors,
                    catch_clauses,
                    finally_steps,
                    finally_on_when,
                );
                StepCompileResult::Matched(Ok(CompiledStep::Process {
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                }))
            }

            _ => StepCompileResult::NotHandled(step),
        }
    }
}
