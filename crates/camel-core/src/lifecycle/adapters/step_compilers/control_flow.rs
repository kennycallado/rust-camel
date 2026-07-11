//! Control-flow step compilers: Loop, DeclarativeLoop, Filter, DeclarativeFilter, Choice, DeclarativeChoice.
//!
//! These are the structural EIP patterns that evaluate predicates to decide
//! which sub-pipeline executes. All recursively compile child steps.

use camel_api::{
    CamelError, ConfigValidationError,
    loop_eip::{LoopConfig, LoopMode},
};

use super::{
    CompilationContext, CompiledStep, StepCompileResult, StepCompiler, StepCompilerRegistry,
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
    ) -> StepCompileResult {
        match step {
            // ── Loop (programmatic) ──
            BuilderStep::Loop { config, steps } => {
                let (sub_segments, lifecycles) =
                    match ctx.compile_children_segments(steps, registry) {
                        Ok(pair) => pair,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                let body = compose_outcome_segment(sub_segments);
                let loop_segment = camel_processor::LoopSegment { config, body };
                StepCompileResult::Matched(Ok(CompiledStep::Segment {
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
                        let predicate = match compile_filter_predicate(ctx.languages, &pred) {
                            Ok(p) => p,
                            Err(e) => return StepCompileResult::Matched(Err(e)),
                        };
                        LoopMode::While(predicate)
                    }
                    (Some(_), Some(_)) => {
                        return StepCompileResult::Matched(Err(CamelError::from(
                            ConfigValidationError::LoopConflictingCountAndWhile,
                        )));
                    }
                    (None, None) => {
                        return StepCompileResult::Matched(Err(CamelError::from(
                            ConfigValidationError::LoopMissingCountOrWhile,
                        )));
                    }
                };
                let (sub_segments, lifecycles) =
                    match ctx.compile_children_segments(steps, registry) {
                        Ok(pair) => pair,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                let body = compose_outcome_segment(sub_segments);
                let mut config = LoopConfig::new(mode);
                if let Some(mi) = max_iterations {
                    config = config.with_max_iterations(mi);
                }
                let loop_segment = camel_processor::LoopSegment { config, body };
                StepCompileResult::Matched(Ok(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(loop_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                }))
            }

            // ── Filter (programmatic) ──
            BuilderStep::Filter { predicate, steps } => {
                let (sub_segments, lifecycles) =
                    match ctx.compile_children_segments(steps, registry) {
                        Ok(pair) => pair,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                let body_segment = compose_outcome_segment(sub_segments);
                let filter_segment = camel_processor::FilterSegment {
                    predicate,
                    body: body_segment,
                };
                StepCompileResult::Matched(Ok(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(filter_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(lifecycles),
                }))
            }

            // ── DeclarativeFilter ──
            BuilderStep::DeclarativeFilter { predicate, steps } => {
                let predicate = match compile_filter_predicate(ctx.languages, &predicate) {
                    Ok(p) => p,
                    Err(e) => return StepCompileResult::Matched(Err(e)),
                };
                let (sub_segments, lifecycles) =
                    match ctx.compile_children_segments(steps, registry) {
                        Ok(pair) => pair,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                let body_segment = compose_outcome_segment(sub_segments);
                let filter_segment = camel_processor::FilterSegment {
                    predicate,
                    body: body_segment,
                };
                StepCompileResult::Matched(Ok(CompiledStep::Segment {
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
                        match ctx.compile_children_segments(when_step.steps, registry) {
                            Ok(pair) => pair,
                            Err(e) => return StepCompileResult::Matched(Err(e)),
                        };
                    all_lifecycles.extend(lifecycles);
                    let body = compose_outcome_segment(sub_segments);
                    when_segments.push(camel_processor::WhenClauseSegment {
                        predicate: when_step.predicate,
                        body,
                    });
                }
                let otherwise_seg = if let Some(otherwise_steps) = otherwise {
                    let (sub_segments, lifecycles) =
                        match ctx.compile_children_segments(otherwise_steps, registry) {
                            Ok(pair) => pair,
                            Err(e) => return StepCompileResult::Matched(Err(e)),
                        };
                    all_lifecycles.extend(lifecycles);
                    Some(compose_outcome_segment(sub_segments))
                } else {
                    None
                };
                let choice_segment = camel_processor::ChoiceSegment {
                    clauses: when_segments,
                    otherwise: otherwise_seg,
                };
                StepCompileResult::Matched(Ok(CompiledStep::Segment {
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
                    let predicate =
                        match compile_filter_predicate(ctx.languages, &when_step.predicate) {
                            Ok(p) => p,
                            Err(e) => return StepCompileResult::Matched(Err(e)),
                        };
                    let (sub_segments, lifecycles) =
                        match ctx.compile_children_segments(when_step.steps, registry) {
                            Ok(pair) => pair,
                            Err(e) => return StepCompileResult::Matched(Err(e)),
                        };
                    all_lifecycles.extend(lifecycles);
                    let body = compose_outcome_segment(sub_segments);
                    when_segments.push(camel_processor::WhenClauseSegment { predicate, body });
                }
                let otherwise_seg = if let Some(otherwise_steps) = otherwise {
                    let (sub_segments, lifecycles) =
                        match ctx.compile_children_segments(otherwise_steps, registry) {
                            Ok(pair) => pair,
                            Err(e) => return StepCompileResult::Matched(Err(e)),
                        };
                    all_lifecycles.extend(lifecycles);
                    Some(compose_outcome_segment(sub_segments))
                } else {
                    None
                };
                let choice_segment = camel_processor::ChoiceSegment {
                    clauses: when_segments,
                    otherwise: otherwise_seg,
                };
                StepCompileResult::Matched(Ok(CompiledStep::Segment {
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
                    match ctx.compile_children_segments(try_steps, registry) {
                        Ok(pair) => pair,
                        Err(e) => return StepCompileResult::Matched(Err(e)),
                    };
                let try_body = compose_outcome_segment(try_sub_segments);

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
                    let (clause_sub_segments, lifecycles) =
                        match ctx.compile_children_segments(c.steps, registry) {
                            Ok(pair) => pair,
                            Err(e) => return StepCompileResult::Matched(Err(e)),
                        };
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
                        Some(expr) => match compile_filter_predicate(ctx.languages, &expr) {
                            Ok(p) => Some(p),
                            Err(e) => {
                                return StepCompileResult::Matched(Err(e));
                            }
                        },
                        None => None,
                    };
                    let (f_sub_segments, lifecycle) =
                        match ctx.compile_children_segments(f.steps, registry) {
                            Ok(pair) => pair,
                            Err(e) => return StepCompileResult::Matched(Err(e)),
                        };
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
                StepCompileResult::Matched(Ok(CompiledStep::Segment {
                    segment: camel_api::OutcomeSegment::new(Box::new(do_try_segment)),
                    body_contract: None,
                    lifecycle: pack_lifecycles(all_lifecycles),
                }))
            }

            _ => StepCompileResult::NotHandled(step),
        }
    }
}
