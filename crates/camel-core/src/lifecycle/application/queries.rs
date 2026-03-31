use std::sync::Arc;

use camel_api::{CamelError, RuntimeQuery, RuntimeQueryResult};

use crate::lifecycle::ports::ProjectionStorePort;

pub struct QueryDeps {
    pub projections: Arc<dyn ProjectionStorePort>,
}

pub async fn execute_query(
    deps: &QueryDeps,
    query: RuntimeQuery,
) -> Result<RuntimeQueryResult, CamelError> {
    match query {
        RuntimeQuery::GetRouteStatus { route_id } => {
            let status = deps
                .projections
                .get_status(&route_id)
                .await?
                .ok_or_else(|| CamelError::RouteError(format!("route '{route_id}' not found")))?;
            Ok(RuntimeQueryResult::RouteStatus {
                route_id: status.route_id,
                status: status.status,
            })
        }
        RuntimeQuery::ListRoutes => {
            let mut route_ids: Vec<String> = deps
                .projections
                .list_statuses()
                .await?
                .into_iter()
                .map(|s| s.route_id)
                .collect();
            route_ids.sort();
            Ok(RuntimeQueryResult::Routes { route_ids })
        }
        RuntimeQuery::InFlightCount { route_id } => {
            // NOTE: This arm exists only for exhaustiveness.
            // `RuntimeBus::ask` intercepts `InFlightCount` BEFORE calling `execute_query`
            // because `execute_query` has no access to the in-flight counter (held by
            // `RouteControllerInternal`). If this error is ever reached, it means the
            // intercept in `runtime_bus.rs` was accidentally removed.
            Err(CamelError::RouteError(format!(
                "InFlightCount for '{route_id}' must be handled by RuntimeBus, not execute_query"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::ports::{ProjectionStorePort, RouteStatusProjection};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct TestProjectionStore {
        statuses: Arc<Mutex<Vec<RouteStatusProjection>>>,
    }

    #[async_trait]
    impl ProjectionStorePort for TestProjectionStore {
        async fn upsert_status(&self, status: RouteStatusProjection) -> Result<(), CamelError> {
            self.statuses.lock().unwrap().push(status);
            Ok(())
        }

        async fn get_status(
            &self,
            route_id: &str,
        ) -> Result<Option<RouteStatusProjection>, CamelError> {
            Ok(self
                .statuses
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.route_id == route_id)
                .cloned())
        }

        async fn list_statuses(&self) -> Result<Vec<RouteStatusProjection>, CamelError> {
            Ok(self.statuses.lock().unwrap().clone())
        }

        async fn remove_status(&self, route_id: &str) -> Result<(), CamelError> {
            self.statuses
                .lock()
                .unwrap()
                .retain(|s| s.route_id != route_id);
            Ok(())
        }
    }

    #[tokio::test]
    async fn get_route_status_returns_projection() {
        let store = TestProjectionStore::default();
        store
            .upsert_status(RouteStatusProjection {
                route_id: "b".into(),
                status: "Started".into(),
            })
            .await
            .unwrap();

        let deps = QueryDeps {
            projections: Arc::new(store),
        };
        let result = execute_query(
            &deps,
            RuntimeQuery::GetRouteStatus {
                route_id: "b".into(),
            },
        )
        .await
        .unwrap();

        assert!(matches!(
            result,
            RuntimeQueryResult::RouteStatus { route_id, status }
            if route_id == "b" && status == "Started"
        ));
    }

    #[tokio::test]
    async fn get_route_status_not_found_returns_error() {
        let deps = QueryDeps {
            projections: Arc::new(TestProjectionStore::default()),
        };

        let err = execute_query(
            &deps,
            RuntimeQuery::GetRouteStatus {
                route_id: "missing".into(),
            },
        )
        .await
        .expect_err("missing route must fail");

        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn list_routes_returns_sorted_ids() {
        let store = TestProjectionStore::default();
        store
            .upsert_status(RouteStatusProjection {
                route_id: "z".into(),
                status: "Started".into(),
            })
            .await
            .unwrap();
        store
            .upsert_status(RouteStatusProjection {
                route_id: "a".into(),
                status: "Stopped".into(),
            })
            .await
            .unwrap();

        let deps = QueryDeps {
            projections: Arc::new(store),
        };
        let result = execute_query(&deps, RuntimeQuery::ListRoutes)
            .await
            .unwrap();

        assert!(matches!(
            result,
            RuntimeQueryResult::Routes { route_ids }
            if route_ids == vec!["a".to_string(), "z".to_string()]
        ));
    }
}
