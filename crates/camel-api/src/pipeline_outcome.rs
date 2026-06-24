//! Outcome of executing a full route pipeline (multiple steps).
//!
//! `PipelineOutcome` lives ONE LAYER ABOVE Tower. Individual processors and
//! producers continue to return `Result<Exchange, CamelError>`; `PipelineOutcome`
//! is produced only by `run_steps` (the pipeline executor) and consumed by the
//! pipeline's `Service<Exchange>` impl, which translates it back to
//! `Result<Exchange, CamelError>`. See ADR-0024.

use crate::error::CamelError;
use crate::exchange::Exchange;

/// Result of executing a full route pipeline.
///
/// Produced by `run_steps`; consumed by the pipeline's `Service<Exchange>` impl
/// and (transitively) by the route controller and consumer reply channels.
#[derive(Debug, Clone)]
pub enum PipelineOutcome {
    /// Normal end of pipeline (all steps completed, or handler returned `Handled`).
    Completed(Exchange),
    /// `CompiledStep::Stop` was hit. The Exchange is the response state
    /// (NOT discarded). Stop is successful control flow, not an error.
    Stopped(Exchange),
    /// Unhandled error escaped the pipeline (handler returned `Propagate`,
    /// or no handler was configured and a step errored).
    Failed(CamelError),
}

impl PipelineOutcome {
    /// Translate to the Tower-layer `Result<Exchange, CamelError>`.
    ///
    /// This is the canonical PUBLIC reply-channel adapter from ADR-0024 §3.5:
    /// `Completed(ex)` and `Stopped(ex)` both become `Ok(ex)` — indistinguishable
    /// to the consumer, which is the core fix for Bug B. `Failed(err)` becomes
    /// `Err(err)`.
    ///
    /// This is the ONLY `PipelineOutcome → Result` translation site for
    /// `Service<Exchange>::Response`. Code review MUST reject any new
    /// translation sites.
    pub fn into_tower_result(self) -> Result<Exchange, CamelError> {
        match self {
            PipelineOutcome::Completed(ex) | PipelineOutcome::Stopped(ex) => Ok(ex),
            PipelineOutcome::Failed(err) => Err(err),
        }
    }

    /// `true` if the pipeline reached a successful end state (`Completed` or `Stopped`).
    pub fn is_success(&self) -> bool {
        matches!(
            self,
            PipelineOutcome::Completed(_) | PipelineOutcome::Stopped(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

    fn ex(body: &str) -> Exchange {
        Exchange::new(Message::new(body))
    }

    #[test]
    fn completed_maps_to_ok() {
        let outcome = PipelineOutcome::Completed(ex("done"));
        assert!(outcome.into_tower_result().is_ok());
    }

    #[test]
    fn stopped_maps_to_ok_indistinguishable_from_completed() {
        // The core Bug B invariant: consumer cannot tell Completed from Stopped.
        let stopped = PipelineOutcome::Stopped(ex("halted")).into_tower_result();
        let completed = PipelineOutcome::Completed(ex("halted")).into_tower_result();
        assert!(stopped.is_ok());
        assert!(completed.is_ok());
        // Bodies identical — single reply path covers both.
        assert_eq!(
            stopped.unwrap().input.body.as_text(),
            completed.unwrap().input.body.as_text()
        );
    }

    #[test]
    fn failed_maps_to_err() {
        let outcome = PipelineOutcome::Failed(CamelError::ProcessorError("boom".into()));
        assert!(outcome.into_tower_result().is_err());
    }

    #[test]
    fn is_success_true_for_completed_and_stopped() {
        assert!(PipelineOutcome::Completed(ex("x")).is_success());
        assert!(PipelineOutcome::Stopped(ex("x")).is_success());
        assert!(!PipelineOutcome::Failed(CamelError::ChannelClosed).is_success());
    }
}
