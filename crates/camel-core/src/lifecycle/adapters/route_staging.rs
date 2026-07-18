//! F2 staging machinery for `DefaultRouteController`.
//!
//! Extracted from `route_controller.rs` (Task 2 F2 review Fix #5: file crossed
//! the 1k-line thermo-nuclear threshold). The four methods below are the only
//! callers of the `prepared_staging: HashMap<String, ManagedRoute>` field:
//!
//! - [`DefaultRouteController::prepare_route_definition_with_generation`] —
//!   two-phase reload entry; stages a fresh `ManagedRoute` keyed by `route_id`.
//! - [`DefaultRouteController::insert_prepared_route`] — commit path; drains
//!   the staged entry into the live `routes` map (re-stages on duplicate-id
//!   error so the caller can retry or discard explicitly).
//! - [`DefaultRouteController::discard_prepared_staging`] — error-path drain
//!   for `reload_actions::apply_*`; safe to drop (see doc comment).
//! - [`DefaultRouteController::prepared_staging_is_empty`] — test-only
//!   assertion used by the F2 regression tests.
//!
//! All four methods share a single `impl DefaultRouteController` block, which
//! Rust allows to span files (the type is the dispatch key, not the module).

use camel_api::CamelError;

use crate::lifecycle::application::route_definition::RouteDefinition;
use crate::lifecycle::domain::route_compilation::PreparedRoute;

impl crate::lifecycle::adapters::route_controller::DefaultRouteController {
    pub(crate) fn insert_prepared_route(
        &mut self,
        prepared: PreparedRoute,
    ) -> Result<(), CamelError> {
        let managed = self
            .prepared_staging
            .remove(&prepared.route_id)
            .ok_or_else(|| {
                CamelError::RouteError(format!(
                    "Prepared route '{}' not in staging (already consumed or never prepared)",
                    prepared.route_id
                ))
            })?;

        if self.routes.contains_key(&prepared.route_id) {
            // Re-insert into staging so the caller's error path can discard
            // explicitly OR retry. The orphan ManagedRoute is dropped on `remove`
            // above; we put it back to preserve the "staging has it" contract.
            self.prepared_staging
                .insert(prepared.route_id.clone(), managed);
            return Err(CamelError::RouteError(format!(
                "Route '{}' already exists",
                prepared.route_id
            )));
        }

        self.routes.insert(prepared.route_id, managed);
        Ok(())
    }

    /// Drain a single staged entry (called by `reload_actions` on insert-error paths).
    /// Safe to drop: `build_managed_route` initializes handles to `None` (no spawned
    /// tasks at prepare-time); the unused CancellationToken is a no-op on drop.
    pub(crate) fn discard_prepared_staging(&mut self, route_id: &str) {
        let _ = self.prepared_staging.remove(route_id);
    }

    #[cfg(test)]
    pub(super) fn prepared_staging_is_empty(&self) -> bool {
        self.prepared_staging.is_empty()
    }

    pub(crate) fn prepare_route_definition_with_generation(
        &mut self,
        definition: RouteDefinition,
        generation: u64,
    ) -> Result<PreparedRoute, CamelError> {
        let route_id = definition.route_id().to_string();
        if self.prepared_staging.contains_key(&route_id) {
            return Err(CamelError::RouteError(format!(
                "Route '{}' already has a prepared-but-uncommitted staging entry \
                 (call insert_prepared_route or discard_prepared_staging first)",
                route_id
            )));
        }
        let managed = self.build_managed_route(
            definition,
            &super::step_resolution::FunctionStagingMode::HotReload { generation },
        )?;
        self.prepared_staging.insert(route_id.clone(), managed);
        Ok(PreparedRoute { route_id })
    }
}
