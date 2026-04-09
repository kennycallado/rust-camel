use async_trait::async_trait;
use camel_api::RuntimeQueryResult;

use crate::lifecycle::domain::DomainError;
use crate::lifecycle::domain::{RouteRuntimeAggregate, RuntimeEvent};

use crate::lifecycle::application::route_definition::RouteDefinition;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteStatusProjection {
    pub route_id: String,
    pub status: String,
}

#[async_trait]
pub trait RouteRepositoryPort: Send + Sync {
    async fn load(&self, route_id: &str) -> Result<Option<RouteRuntimeAggregate>, DomainError>;
    async fn save(&self, aggregate: RouteRuntimeAggregate) -> Result<(), DomainError>;
    async fn save_if_version(
        &self,
        aggregate: RouteRuntimeAggregate,
        expected_version: u64,
    ) -> Result<(), DomainError>;
    async fn delete(&self, route_id: &str) -> Result<(), DomainError>;
}

#[async_trait]
pub trait ProjectionStorePort: Send + Sync {
    async fn upsert_status(&self, status: RouteStatusProjection) -> Result<(), DomainError>;
    async fn get_status(&self, route_id: &str)
    -> Result<Option<RouteStatusProjection>, DomainError>;
    async fn list_statuses(&self) -> Result<Vec<RouteStatusProjection>, DomainError>;
    async fn remove_status(&self, route_id: &str) -> Result<(), DomainError>;
}

#[async_trait]
pub trait EventPublisherPort: Send + Sync {
    async fn publish(&self, events: &[RuntimeEvent]) -> Result<(), DomainError>;
}

#[async_trait]
pub trait RuntimeEventJournalPort: Send + Sync {
    /// Append a batch of runtime events to a durable journal.
    async fn append_batch(&self, events: &[RuntimeEvent]) -> Result<(), DomainError>;

    /// Load the complete journal stream (append order).
    async fn load_all(&self) -> Result<Vec<RuntimeEvent>, DomainError>;

    /// Append a "command seen" marker to durable dedup journal.
    ///
    /// Default implementation is a no-op for adapters without durable dedup support.
    async fn append_command_id(&self, _command_id: &str) -> Result<(), DomainError> {
        Ok(())
    }

    /// Append a "command forgotten" marker to durable dedup journal.
    ///
    /// Used to rollback provisional reservations when command execution fails.
    async fn remove_command_id(&self, _command_id: &str) -> Result<(), DomainError> {
        Ok(())
    }

    /// Load all accepted command IDs from durable dedup journal.
    ///
    /// Default implementation returns no IDs for adapters without durable dedup support.
    async fn load_command_ids(&self) -> Result<Vec<String>, DomainError> {
        Ok(Vec::new())
    }
}

#[async_trait]
pub trait CommandDedupPort: Send + Sync {
    /// Returns true when the command ID is seen for the first time.
    /// Returns false when the command ID was already processed/reserved.
    async fn first_seen(&self, command_id: &str) -> Result<bool, DomainError>;

    /// Remove command ID reservation/marker.
    /// Used as compensation when command execution fails.
    async fn forget_seen(&self, command_id: &str) -> Result<(), DomainError>;
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
    ) -> Result<(), DomainError>;

    /// Atomically persist delete transition (remove aggregate/projection + append events).
    async fn persist_delete(
        &self,
        route_id: &str,
        events: &[RuntimeEvent],
    ) -> Result<(), DomainError>;

    /// Recover in-memory runtime state from durable journal, if available.
    ///
    /// Implementations without durable journal support should return `Ok(())`.
    async fn recover_from_journal(&self) -> Result<(), DomainError> {
        Ok(())
    }
}

#[async_trait]
pub trait RuntimeExecutionPort: Send + Sync {
    async fn register_route(&self, definition: RouteDefinition) -> Result<(), DomainError>;
    async fn start_route(&self, route_id: &str) -> Result<(), DomainError>;
    async fn stop_route(&self, route_id: &str) -> Result<(), DomainError>;
    async fn suspend_route(&self, route_id: &str) -> Result<(), DomainError>;
    async fn resume_route(&self, route_id: &str) -> Result<(), DomainError>;
    async fn reload_route(&self, route_id: &str) -> Result<(), DomainError>;
    async fn remove_route(&self, route_id: &str) -> Result<(), DomainError>;
    async fn in_flight_count(&self, route_id: &str) -> Result<RuntimeQueryResult, DomainError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyJournal;

    #[async_trait]
    impl RuntimeEventJournalPort for DummyJournal {
        async fn append_batch(&self, _events: &[RuntimeEvent]) -> Result<(), DomainError> {
            Ok(())
        }

        async fn load_all(&self) -> Result<Vec<RuntimeEvent>, DomainError> {
            Ok(Vec::new())
        }
    }

    struct DummyUow;

    #[async_trait]
    impl RuntimeUnitOfWorkPort for DummyUow {
        async fn persist_upsert(
            &self,
            _aggregate: RouteRuntimeAggregate,
            _expected_version: Option<u64>,
            _projection: RouteStatusProjection,
            _events: &[RuntimeEvent],
        ) -> Result<(), DomainError> {
            Ok(())
        }

        async fn persist_delete(
            &self,
            _route_id: &str,
            _events: &[RuntimeEvent],
        ) -> Result<(), DomainError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn default_journal_methods_are_noop_ok() {
        let journal = DummyJournal;
        journal.append_command_id("c1").await.unwrap();
        journal.remove_command_id("c1").await.unwrap();
        assert!(journal.load_command_ids().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn default_uow_recover_is_noop_ok() {
        let uow = DummyUow;
        uow.recover_from_journal().await.unwrap();
    }
}
