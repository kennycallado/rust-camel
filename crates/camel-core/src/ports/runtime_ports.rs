use async_trait::async_trait;

use crate::CamelError;

use crate::application::route_types::RouteDefinition;
use crate::domain::{RouteRuntimeAggregate, RuntimeEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteStatusProjection {
    pub route_id: String,
    pub status: String,
}

#[async_trait]
pub trait RouteRepositoryPort: Send + Sync {
    async fn load(&self, route_id: &str) -> Result<Option<RouteRuntimeAggregate>, CamelError>;
    async fn save(&self, aggregate: RouteRuntimeAggregate) -> Result<(), CamelError>;
    async fn save_if_version(
        &self,
        aggregate: RouteRuntimeAggregate,
        expected_version: u64,
    ) -> Result<(), CamelError>;
    async fn delete(&self, route_id: &str) -> Result<(), CamelError>;
}

#[async_trait]
pub trait ProjectionStorePort: Send + Sync {
    async fn upsert_status(&self, status: RouteStatusProjection) -> Result<(), CamelError>;
    async fn get_status(&self, route_id: &str)
    -> Result<Option<RouteStatusProjection>, CamelError>;
    async fn list_statuses(&self) -> Result<Vec<RouteStatusProjection>, CamelError>;
    async fn remove_status(&self, route_id: &str) -> Result<(), CamelError>;
}

#[async_trait]
pub trait EventPublisherPort: Send + Sync {
    async fn publish(&self, events: &[RuntimeEvent]) -> Result<(), CamelError>;
}

#[async_trait]
pub trait RuntimeEventJournalPort: Send + Sync {
    /// Append a batch of runtime events to a durable journal.
    async fn append_batch(&self, events: &[RuntimeEvent]) -> Result<(), CamelError>;

    /// Load the complete journal stream (append order).
    async fn load_all(&self) -> Result<Vec<RuntimeEvent>, CamelError>;

    /// Append a "command seen" marker to durable dedup journal.
    ///
    /// Default implementation is a no-op for adapters without durable dedup support.
    async fn append_command_id(&self, _command_id: &str) -> Result<(), CamelError> {
        Ok(())
    }

    /// Append a "command forgotten" marker to durable dedup journal.
    ///
    /// Used to rollback provisional reservations when command execution fails.
    async fn remove_command_id(&self, _command_id: &str) -> Result<(), CamelError> {
        Ok(())
    }

    /// Load all accepted command IDs from durable dedup journal.
    ///
    /// Default implementation returns no IDs for adapters without durable dedup support.
    async fn load_command_ids(&self) -> Result<Vec<String>, CamelError> {
        Ok(Vec::new())
    }
}

#[async_trait]
pub trait CommandDedupPort: Send + Sync {
    /// Returns true when the command ID is seen for the first time.
    /// Returns false when the command ID was already processed/reserved.
    async fn first_seen(&self, command_id: &str) -> Result<bool, CamelError>;

    /// Remove command ID reservation/marker.
    /// Used as compensation when command execution fails.
    async fn forget_seen(&self, command_id: &str) -> Result<(), CamelError>;
}

#[async_trait]
pub trait RuntimeUnitOfWorkPort: Send + Sync {
    /// Atomically persist aggregate + projection + events for an upsert transition.
    async fn persist_upsert(
        &self,
        aggregate: RouteRuntimeAggregate,
        expected_version: Option<u64>,
        projection: RouteStatusProjection,
        events: &[RuntimeEvent],
    ) -> Result<(), CamelError>;

    /// Atomically persist delete transition (remove aggregate/projection + append events).
    async fn persist_delete(
        &self,
        route_id: &str,
        events: &[RuntimeEvent],
    ) -> Result<(), CamelError>;

    /// Recover in-memory runtime state from durable journal, if available.
    ///
    /// Implementations without durable journal support should return `Ok(())`.
    async fn recover_from_journal(&self) -> Result<(), CamelError> {
        Ok(())
    }
}

#[async_trait]
pub trait RuntimeExecutionPort: Send + Sync {
    async fn register_route(&self, definition: RouteDefinition) -> Result<(), CamelError>;
    async fn start_route(&self, route_id: &str) -> Result<(), CamelError>;
    async fn stop_route(&self, route_id: &str) -> Result<(), CamelError>;
    async fn suspend_route(&self, route_id: &str) -> Result<(), CamelError>;
    async fn resume_route(&self, route_id: &str) -> Result<(), CamelError>;
    async fn reload_route(&self, route_id: &str) -> Result<(), CamelError>;
    async fn remove_route(&self, route_id: &str) -> Result<(), CamelError>;
}
