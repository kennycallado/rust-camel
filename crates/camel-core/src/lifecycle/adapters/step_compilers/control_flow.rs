//! Control-flow step compilers: Loop, DeclarativeLoop, Filter, DeclarativeFilter, Choice, DeclarativeChoice.
//!
//! These are the structural EIP patterns that evaluate predicates to decide
//! which sub-pipeline executes. All recursively compile child steps.

use camel_api::{
    CamelError, ConfigValidationError,
    loop_eip::{LoopConfig, LoopMode},
};

use super::{
    CompilationContext, CompileOutcome, CompiledStep, StepCompiler, StepCompilerRegistry,
    pack_lifecycles,
};
use crate::lifecycle::adapters::route_compiler::compose_outcome_segment;
use crate::lifecycle::adapters::step_resolution::compile_filter_predicate;
use crate::lifecycle::application::route_definition::BuilderStep;
use camel_processor::{CatchClauseSegment, CatchMatcher, DoTrySegment, FinallyClauseSegment};

pub(crate) struct ControlFlowCompiler;

impl StepCompiler for ControlFlowCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        ctx: &CompilationContext,
        registry: &StepCompilerRegistry,
    ) -> Result<CompileOutcome, CamelError> {
        match step {
            // ── Loop (programmatic) ──
            BuilderStep::Loop { config, steps } => {
                let (sub_segments, lifecycles) = ctx.compile_children_segments(steps, registry)?;
                let body = compose_outcome_segment(sub_segments);
                let loop_segment = camel_processor::LoopSegment { config, body };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(loop_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                }))
            }

            // ── DeclarativeLoop ──
            BuilderStep::DeclarativeLoop {
                count,
                while_predicate,
                steps,
                max_iterations,
            } => {
                let mode = match (count, while_predicate) {
                    (Some(n), None) => LoopMode::Count(n),
                    (None, Some(pred)) => {
                        let predicate = compile_filter_predicate(ctx.languages, &pred)?;
                        LoopMode::While(predicate)
                    }
                    (Some(_), Some(_)) => {
                        return Err(CamelError::from(
                            ConfigValidationError::LoopConflictingCountAndWhile,
                        ));
                    }
                    (None, None) => {
                        return Err(CamelError::from(
                            ConfigValidationError::LoopMissingCountOrWhile,
                        ));
                    }
                };
                let (sub_segments, lifecycles) = ctx.compile_children_segments(steps, registry)?;
                let body = compose_outcome_segment(sub_segments);
                let mut config = LoopConfig::new(mode);
                if let Some(mi) = max_iterations {
                    config = config.with_max_iterations(mi);
                }
                let loop_segment = camel_processor::LoopSegment { config, body };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(loop_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                }))
            }

            // ── Filter (programmatic) ──
            BuilderStep::Filter { predicate, steps } => {
                let (sub_segments, lifecycles) = ctx.compile_children_segments(steps, registry)?;
                let body_segment = compose_outcome_segment(sub_segments);
                let filter_segment = camel_processor::FilterSegment {
                    predicate,
                    body: body_segment,
                };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(filter_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                }))
            }

            // ── DeclarativeFilter ──
            BuilderStep::DeclarativeFilter { predicate, steps } => {
                let predicate = compile_filter_predicate(ctx.languages, &predicate)?;
                let (sub_segments, lifecycles) = ctx.compile_children_segments(steps, registry)?;
                let body_segment = compose_outcome_segment(sub_segments);
                let filter_segment = camel_processor::FilterSegment {
                    predicate,
                    body: body_segment,
                };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(filter_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                }))
            }

            // ── Choice (programmatic) ──
            BuilderStep::Choice { whens, otherwise } => {
                let mut when_segments = Vec::new();
                let mut all_lifecycles = Vec::new();
                for when_step in whens {
                    let (sub_segments, lifecycles) =
                        ctx.compile_children_segments(when_step.steps, registry)?;
                    all_lifecycles.extend(lifecycles);
                    let body = compose_outcome_segment(sub_segments);
                    when_segments.push(camel_processor::WhenClauseSegment {
                        predicate: when_step.predicate,
                        body,
                    });
                }
                let otherwise_seg = if let Some(otherwise_steps) = otherwise {
                    let (sub_segments, lifecycles) =
                        ctx.compile_children_segments(otherwise_steps, registry)?;
                    all_lifecycles.extend(lifecycles);
                    Some(compose_outcome_segment(sub_segments))
                } else {
                    None
                };
                let choice_segment = camel_processor::ChoiceSegment {
                    clauses: when_segments,
                    otherwise: otherwise_seg,
                };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(choice_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(all_lifecycles),
                }))
            }

            // ── DeclarativeChoice ──
            BuilderStep::DeclarativeChoice { whens, otherwise } => {
                let mut when_segments = Vec::new();
                let mut all_lifecycles = Vec::new();
                for when_step in whens {
                    let predicate = compile_filter_predicate(ctx.languages, &when_step.predicate)?;
                    let (sub_segments, lifecycles) =
                        ctx.compile_children_segments(when_step.steps, registry)?;
                    all_lifecycles.extend(lifecycles);
                    let body = compose_outcome_segment(sub_segments);
                    when_segments.push(camel_processor::WhenClauseSegment { predicate, body });
                }
                let otherwise_seg = if let Some(otherwise_steps) = otherwise {
                    let (sub_segments, lifecycles) =
                        ctx.compile_children_segments(otherwise_steps, registry)?;
                    all_lifecycles.extend(lifecycles);
                    Some(compose_outcome_segment(sub_segments))
                } else {
                    None
                };
                let choice_segment = camel_processor::ChoiceSegment {
                    clauses: when_segments,
                    otherwise: otherwise_seg,
                };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(choice_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(all_lifecycles),
                }))
            }

            // ── DeclarativeDoTry ──
            BuilderStep::DeclarativeDoTry {
                try_steps,
                catch,
                finally,
            } => {
                let (try_sub_segments, mut all_lifecycles) =
                    ctx.compile_children_segments(try_steps, registry)?;
                let try_body = compose_outcome_segment(try_sub_segments);

                let mut catch_clauses = Vec::with_capacity(catch.len());
                for c in catch {
                    let matcher = match (c.exception, c.when) {
                        (Some(names), None) => CatchMatcher::ByVariant(names),
                        (None, Some(expr)) => {
                            let predicate = compile_filter_predicate(ctx.languages, &expr)?;
                            CatchMatcher::Predicate(predicate)
                        }
                        (Some(_), Some(_)) => {
                            return Err(CamelError::Config(
                                "doTry catch clause must specify either `exception` or \
                                 `when`, not both"
                                    .into(),
                            ));
                        }
                        (None, None) => {
                            return Err(CamelError::Config(
                                "doTry catch clause must specify either `exception` or `when`"
                                    .into(),
                            ));
                        }
                    };
                    let on_when = match c.on_when {
                        Some(expr) => Some(compile_filter_predicate(ctx.languages, &expr)?),
                        None => None,
                    };
                    let (clause_sub_segments, lifecycles) =
                        ctx.compile_children_segments(c.steps, registry)?;
                    all_lifecycles.extend(lifecycles);
                    let body = compose_outcome_segment(clause_sub_segments);
                    catch_clauses.push(CatchClauseSegment {
                        matcher,
                        on_when,
                        body,
                        disposition: c.disposition,
                    });
                }

                let finally = if let Some(f) = finally {
                    let on_when = match f.on_when {
                        Some(expr) => Some(compile_filter_predicate(ctx.languages, &expr)?),
                        None => None,
                    };
                    let (f_sub_segments, lifecycle) =
                        ctx.compile_children_segments(f.steps, registry)?;
                    all_lifecycles.extend(lifecycle);
                    let body = compose_outcome_segment(f_sub_segments);
                    Some(FinallyClauseSegment { on_when, body })
                } else {
                    None
                };

                let do_try_segment = DoTrySegment {
                    try_body,
                    catches: catch_clauses,
                    finally,
                };
                Ok(CompileOutcome::Matched(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(do_try_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(all_lifecycles),
                }))
            }

            _ => Ok(CompileOutcome::NotHandled(step)),
        }
    }
}
