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
    }
}
