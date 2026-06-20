//! Builder types for the `doTry` / `doCatch` / `doFinally` EIP pattern.
//!
//! These builders provide a fluent API for constructing doTry scopes within
//! a Camel route. Example:
//!
//! ```ignore
//! RouteBuilder::from("direct:start")
//!     .do_try()
//!         .process(try_step)
//!         .do_catch_exception(&["SomeError"])
//!             .process(catch_step)
//!         .end_do_catch()
//!     .end_do_try()
//! ```

use crate::RouteBuilder;
use camel_api::error_handler::ExceptionDisposition;
use camel_api::{BoxProcessor, FilterPredicate};
use camel_core::route::BuilderStep;
use camel_processor::{CatchClause, CatchMatcher, DoTryService};

// ── doTry / doCatch / doFinally builders ────────────────────────────────────

/// Builder for a `.do_try()` ... `.end_do_try()` block.
pub struct DoTryBuilder {
    parent: RouteBuilder,
    try_steps: Vec<BoxProcessor>,
    catch_clauses: Vec<CatchClause>,
    finally_steps: Vec<BoxProcessor>,
    finally_on_when: Option<FilterPredicate>,
    finally_set: bool,
}

/// Builder for a `.do_catch_exception()` / `.do_catch_when()` / `.do_catch_all()` clause.
pub struct DoCatchBuilder {
    parent: DoTryBuilder,
    matcher: CatchMatcher,
    on_when: Option<FilterPredicate>,
    steps: Vec<BoxProcessor>,
    disposition: ExceptionDisposition,
}

/// Builder for a `.do_finally()` ... `.end_do_finally()` block.
pub struct DoFinallyBuilder {
    parent: DoTryBuilder,
    steps: Vec<BoxProcessor>,
    on_when: Option<FilterPredicate>,
}

impl RouteBuilder {
    /// Open a `doTry` scope. Steps inside are protected by catch and finally clauses.
    pub fn do_try(self) -> DoTryBuilder {
        DoTryBuilder {
            parent: self,
            try_steps: Vec::new(),
            catch_clauses: Vec::new(),
            finally_steps: Vec::new(),
            finally_on_when: None,
            finally_set: false,
        }
    }
}

impl DoTryBuilder {
    /// Add a step to the try block.
    pub fn process(mut self, processor: BoxProcessor) -> Self {
        self.try_steps.push(processor);
        self
    }

    /// Open a catch clause that matches errors by variant name(s).
    ///
    /// Use `"*"` to match any variant (catch-all).
    pub fn do_catch_exception(self, variants: &[&str]) -> DoCatchBuilder {
        DoCatchBuilder {
            parent: self,
            matcher: CatchMatcher::ByVariant(variants.iter().map(|s| (*s).to_string()).collect()),
            on_when: None,
            steps: Vec::new(),
            disposition: ExceptionDisposition::Handled,
        }
    }

    /// Open a catch clause that matches errors by a predicate over the exchange.
    pub fn do_catch_when(self, predicate: FilterPredicate) -> DoCatchBuilder {
        DoCatchBuilder {
            parent: self,
            matcher: CatchMatcher::Predicate(predicate),
            on_when: None,
            steps: Vec::new(),
            disposition: ExceptionDisposition::Handled,
        }
    }

    /// Open a catch-all clause (matches any error variant).
    pub fn do_catch_all(self) -> DoCatchBuilder {
        self.do_catch_exception(&["*"])
    }

    /// Open a `doFinally` block.
    ///
    /// # Panics
    /// Panics if `do_finally` has already been called on this scope.
    pub fn do_finally(self) -> DoFinallyBuilder {
        if self.finally_set {
            panic!("do_finally can only be called once per do_try scope");
        }
        DoFinallyBuilder {
            parent: self,
            steps: Vec::new(),
            on_when: None,
        }
    }

    /// Close the `doTry` scope and return the parent `RouteBuilder`.
    pub fn end_do_try(self) -> RouteBuilder {
        let do_try = DoTryService {
            try_steps: self.try_steps,
            catch_clauses: self.catch_clauses,
            finally_steps: self.finally_steps,
            finally_on_when: self.finally_on_when,
        };
        let mut parent = self.parent;
        parent
            .steps
            .push(BuilderStep::Processor(BoxProcessor::new(do_try)));
        parent
    }
}

impl DoCatchBuilder {
    /// Add a step to the catch clause's sub-pipeline.
    pub fn process(mut self, processor: BoxProcessor) -> Self {
        self.steps.push(processor);
        self
    }

    /// Set an additional predicate that must also match for this catch clause to fire.
    pub fn on_when(mut self, predicate: FilterPredicate) -> Self {
        self.on_when = Some(predicate);
        self
    }

    /// Set the disposition for this catch clause.
    ///
    /// # Panics
    /// Panics if `value` is `ExceptionDisposition::Continued`, which is not
    /// supported in doTry MVP (spec §3).
    pub fn disposition(mut self, value: ExceptionDisposition) -> Self {
        if matches!(value, ExceptionDisposition::Continued) {
            panic!(
                "ExceptionDisposition::Continued is not supported in doTry MVP (spec §3); \
                 use Handled or Propagate"
            );
        }
        self.disposition = value;
        self
    }

    /// Sugar for `disposition(ExceptionDisposition::Handled)`.
    ///
    /// The caught error is marked handled and the catch clause's exchange
    /// becomes the final result (no re-throw).
    pub fn handled(self) -> Self {
        self.disposition(ExceptionDisposition::Handled)
    }

    /// Sugar for `disposition(ExceptionDisposition::Propagate)`.
    ///
    /// The catch clause runs for side-effects and the original error is
    /// re-thrown.
    ///
    /// Note: `.continued()` is intentionally NOT provided — Continued is
    /// rejected at parse time for doTry MVP per spec §3 (semantically
    /// ambiguous at catch-clause scope).
    pub fn propagate(self) -> Self {
        self.disposition(ExceptionDisposition::Propagate)
    }

    /// Close the catch clause and return the parent `DoTryBuilder`.
    pub fn end_do_catch(self) -> DoTryBuilder {
        let mut parent = self.parent;
        parent.catch_clauses.push(CatchClause {
            matcher: self.matcher,
            on_when: self.on_when,
            steps: self.steps,
            disposition: self.disposition,
        });
        parent
    }
}

impl DoFinallyBuilder {
    /// Add a step to the finally block.
    pub fn process(mut self, processor: BoxProcessor) -> Self {
        self.steps.push(processor);
        self
    }

    /// Set an optional predicate that gates whether the finally block runs.
    pub fn on_when(mut self, predicate: FilterPredicate) -> Self {
        self.on_when = Some(predicate);
        self
    }

    /// Close the finally block and return the parent `DoTryBuilder`.
    pub fn end_do_finally(self) -> DoTryBuilder {
        let mut parent = self.parent;
        parent.finally_set = true;
        parent.finally_on_when = self.on_when;
        parent.finally_steps = self.steps;
        parent
    }
}

#[cfg(test)]
mod tests {
    use crate::RouteBuilder;
    use camel_api::error_handler::ExceptionDisposition;
    use camel_api::{BoxProcessor, BoxProcessorExt};
    use camel_core::route::BuilderStep;

    fn passthrough() -> BoxProcessor {
        BoxProcessor::from_fn(move |ex| Box::pin(async move { Ok(ex) }))
    }

    #[test]
    fn do_try_builder_assembles_correct_shape() {
        let route = RouteBuilder::from("direct:start")
            .route_id("do-try-shape")
            .do_try()
            .process(passthrough())
            .do_catch_exception(&["ProcessorError"])
            .disposition(ExceptionDisposition::Handled)
            .process(passthrough())
            .end_do_catch()
            .do_finally()
            .process(passthrough())
            .end_do_finally()
            .end_do_try();

        let config = route.build().unwrap();
        assert_eq!(
            config.steps().len(),
            1,
            "expected exactly one step (the DoTryService)"
        );
        assert!(
            matches!(config.steps().first(), Some(BuilderStep::Processor(_))),
            "the single step must be a Processor variant (the DoTryService)"
        );
    }

    #[test]
    fn do_try_builder_disposition_sugar_methods() {
        // .handled() and .propagate() are syntactic sugar for the two supported dispositions.
        // .continued() is intentionally NOT provided — Continued is rejected at parse time
        // for doTry MVP per spec §3.
        let _ = RouteBuilder::from("direct:a")
            .route_id("do-try-sugar-a")
            .do_try()
            .process(passthrough())
            .do_catch_exception(&["Io"])
            .handled()
            .end_do_catch()
            .end_do_try()
            .build()
            .unwrap();

        let _ = RouteBuilder::from("direct:b")
            .route_id("do-try-sugar-b")
            .do_try()
            .process(passthrough())
            .do_catch_exception(&["Io"])
            .propagate()
            .end_do_catch()
            .end_do_try()
            .build()
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "do_finally can only be called once per do_try scope")]
    fn do_finally_called_twice_panics() {
        let _ = RouteBuilder::from("direct:start")
            .route_id("do-try-double-finally")
            .do_try()
            .process(passthrough())
            .do_finally()
            .process(passthrough())
            .end_do_finally()
            .do_finally();
    }

    #[test]
    #[should_panic(expected = "ExceptionDisposition::Continued is not supported in doTry MVP")]
    fn disposition_continued_panics() {
        let _ = RouteBuilder::from("direct:start")
            .route_id("do-try-continued")
            .do_try()
            .process(passthrough())
            .do_catch_exception(&["ProcessorError"])
            .disposition(ExceptionDisposition::Continued)
            .end_do_catch()
            .end_do_try();
    }
}
