//! Control-flow step compilers: Loop, DeclarativeLoop, Filter, DeclarativeFilter, Choice, DeclarativeChoice.
//!
//! These are the structural EIP patterns that evaluate predicates to decide
//! which sub-pipeline executes. All recursively compile child steps.

use camel_api::{
    BoxProcessor, CamelError,
    loop_eip::{LoopConfig, LoopMode},
};

use camel_processor::{ChoiceService, WhenClause};

use super::{CompilationContext, StepCompileResult, StepCompiler, StepCompilerRegistry};
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
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::loop_eip::LoopService::new(config, sub_pipeline);
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
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
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let config = LoopConfig { mode };
                let svc = camel_processor::loop_eip::LoopService::new(config, sub_pipeline);
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            // ── Filter (programmatic) ──
            BuilderStep::Filter { predicate, steps } => {
                let sub_pairs = match ctx.compile_children(steps, registry) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::FilterService::from_predicate(predicate, sub_pipeline);
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
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
                let sub_processors: Vec<BoxProcessor> =
                    sub_pairs.into_iter().map(|(p, _)| p).collect();
                let sub_pipeline = compose_pipeline(sub_processors);
                let svc = camel_processor::FilterService::from_predicate(predicate, sub_pipeline);
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            // ── Choice (programmatic) ──
            BuilderStep::Choice { whens, otherwise } => {
                let mut when_clauses = Vec::new();
                for when_step in whens {
                    let sub_pairs = match ctx.compile_children(when_step.steps, registry) {
                        Ok(p) => p,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
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
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
                    Some(compose_pipeline(sub_processors))
                } else {
                    None
                };
                let svc = ChoiceService::new(when_clauses, otherwise_pipeline);
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
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
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
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
                    let sub_processors: Vec<BoxProcessor> =
                        sub_pairs.into_iter().map(|(p, _)| p).collect();
                    Some(compose_pipeline(sub_processors))
                } else {
                    None
                };
                let svc = ChoiceService::new(when_clauses, otherwise_pipeline);
                StepCompileResult::Matched(Ok((BoxProcessor::new(svc), None)))
            }

            _ => StepCompileResult::NotHandled(step),
        }
    }
}
