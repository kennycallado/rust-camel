use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};

use camel_api::CamelError;

use crate::lifecycle::domain::{RouteRuntimeAggregate, RouteRuntimeState, RuntimeEvent};
use crate::lifecycle::ports::{
    CommandDedupPort, EventPublisherPort, ProjectionStorePort, RouteRepositoryPort,
    RouteStatusProjection, RuntimeEventJournalPort, RuntimeUnitOfWorkPort,
};

#[derive(Default, Clone)]
pub struct InMemoryRouteRepository {
    routes: Arc<RwLock<HashMap<String, RouteRuntimeAggregate>>>,
}

#[async_trait]
impl RouteRepositoryPort for InMemoryRouteRepository {
    async fn load(&self, route_id: &str) -> Result<Option<RouteRuntimeAggregate>, CamelError> {
        let routes = self.routes.read().await;
        Ok(routes.get(route_id).cloned())
    }

    async fn save(&self, aggregate: RouteRuntimeAggregate) -> Result<(), CamelError> {
        let mut routes = self.routes.write().await;
        routes.insert(aggregate.route_id().to_string(), aggregate);
        Ok(())
    }

    async fn save_if_version(
        &self,
        aggregate: RouteRuntimeAggregate,
        expected_version: u64,
    ) -> Result<(), CamelError> {
        let mut routes = self.routes.write().await;
        let route_id = aggregate.route_id().to_string();
        let current = routes.get(&route_id).ok_or_else(|| {
            CamelError::RouteError(format!(
                "optimistic lock conflict for route '{route_id}': route not found"
            ))
        })?;

        if current.version() != expected_version {
            return Err(CamelError::RouteError(format!(
                "optimistic lock conflict for route '{route_id}': expected version {expected_version}, actual {}",
                current.version()
            )));
        }

        routes.insert(route_id, aggregate);
        Ok(())
    }

    async fn delete(&self, route_id: &str) -> Result<(), CamelError> {
        let mut routes = self.routes.write().await;
        routes.remove(route_id);
        Ok(())
    }
}

#[derive(Default, Clone)]
pub struct InMemoryProjectionStore {
    statuses: Arc<RwLock<HashMap<String, RouteStatusProjection>>>,
}

#[async_trait]
impl ProjectionStorePort for InMemoryProjectionStore {
    async fn upsert_status(&self, status: RouteStatusProjection) -> Result<(), CamelError> {
        let mut statuses = self.statuses.write().await;
        statuses.insert(status.route_id.clone(), status);
        Ok(())
    }

    async fn get_status(
        &self,
        route_id: &str,
    ) -> Result<Option<RouteStatusProjection>, CamelError> {
        let statuses = self.statuses.read().await;
        Ok(statuses.get(route_id).cloned())
    }

    async fn list_statuses(&self) -> Result<Vec<RouteStatusProjection>, CamelError> {
        let statuses = self.statuses.read().await;
        Ok(statuses.values().cloned().collect())
    }

    async fn remove_status(&self, route_id: &str) -> Result<(), CamelError> {
        let mut statuses = self.statuses.write().await;
        statuses.remove(route_id);
        Ok(())
    }
}

#[derive(Default, Clone)]
pub struct InMemoryEventPublisher {
    events: Arc<RwLock<Vec<RuntimeEvent>>>,
}

impl InMemoryEventPublisher {
    pub async fn snapshot(&self) -> Vec<RuntimeEvent> {
        self.events.read().await.clone()
    }
}

#[async_trait]
impl EventPublisherPort for InMemoryEventPublisher {
    async fn publish(&self, events: &[RuntimeEvent]) -> Result<(), CamelError> {
        let mut stored = self.events.write().await;
        stored.extend(events.iter().cloned());
        Ok(())
    }
}

#[derive(Default, Clone)]
pub struct InMemoryCommandDedup {
    seen: Arc<RwLock<HashSet<String>>>,
}

#[async_trait]
impl CommandDedupPort for InMemoryCommandDedup {
    async fn first_seen(&self, command_id: &str) -> Result<bool, CamelError> {
        let mut seen = self.seen.write().await;
        Ok(seen.insert(command_id.to_string()))
    }

    async fn forget_seen(&self, command_id: &str) -> Result<(), CamelError> {
        let mut seen = self.seen.write().await;
        seen.remove(command_id);
        Ok(())
    }
}

#[derive(Clone)]
pub struct InMemoryRuntimeStore {
    inner: Arc<Mutex<RuntimeStoreState>>,
    journal: Option<Arc<dyn RuntimeEventJournalPort>>,
}

#[derive(Default)]
struct RuntimeStoreState {
    routes: HashMap<String, RouteRuntimeAggregate>,
    statuses: HashMap<String, RouteStatusProjection>,
    events: Vec<RuntimeEvent>,
    seen: HashSet<String>,
}

impl InMemoryRuntimeStore {
    pub fn with_journal(mut self, journal: Arc<dyn RuntimeEventJournalPort>) -> Self {
        self.journal = Some(journal);
        self
    }

    pub async fn snapshot_events(&self) -> Vec<RuntimeEvent> {
        self.inner.lock().await.events.clone()
    }
}

fn upsert_replayed_route(
    state: &mut RuntimeStoreState,
    route_id: &str,
    next_state: RouteRuntimeState,
    status: &str,
    increment_version: bool,
) {
    let current_version = state
        .routes
        .get(route_id)
        .map(|agg| agg.version())
        .unwrap_or(0);
    let next_version = if increment_version {
        current_version.saturating_add(1)
    } else {
        current_version
    };
    state.routes.insert(
        route_id.to_string(),
        RouteRuntimeAggregate::from_snapshot(route_id, next_state, next_version),
    );
    state.statuses.insert(
        route_id.to_string(),
        RouteStatusProjection {
            route_id: route_id.to_string(),
            status: status.to_string(),
        },
    );
}

fn apply_replayed_event(state: &mut RuntimeStoreState, event: &RuntimeEvent) {
    match event {
        RuntimeEvent::RouteRegistered { route_id } => {
            state.routes.insert(
                route_id.clone(),
                RouteRuntimeAggregate::new(route_id.clone()),
            );
            state.statuses.insert(
                route_id.clone(),
                RouteStatusProjection {
                    route_id: route_id.clone(),
                    status: "Registered".to_string(),
                },
            );
        }
        RuntimeEvent::RouteStartRequested { route_id } => {
            upsert_replayed_route(
                state,
                route_id,
                RouteRuntimeState::Starting,
                "Starting",
                true,
            );
        }
        RuntimeEvent::RouteStarted { route_id } => {
            let increment_version = !matches!(
                state.routes.get(route_id).map(|agg| agg.state()),
                Some(RouteRuntimeState::Starting)
            );
            upsert_replayed_route(
                state,
                route_id,
                RouteRuntimeState::Started,
                "Started",
                increment_version,
            );
        }
        RuntimeEvent::RouteFailed { route_id, error } => {
            upsert_replayed_route(
                state,
                route_id,
                RouteRuntimeState::Failed(error.clone()),
                "Failed",
                true,
            );
        }
        RuntimeEvent::RouteStopped { route_id } => {
            upsert_replayed_route(state, route_id, RouteRuntimeState::Stopped, "Stopped", true);
        }
        RuntimeEvent::RouteSuspended { route_id } => {
            upsert_replayed_route(
                state,
                route_id,
                RouteRuntimeState::Suspended,
                "Suspended",
                true,
            );
        }
        RuntimeEvent::RouteResumed { route_id } => {
            upsert_replayed_route(state, route_id, RouteRuntimeState::Started, "Started", true);
        }
        RuntimeEvent::RouteReloaded { route_id } => {
            upsert_replayed_route(state, route_id, RouteRuntimeState::Started, "Started", true);
        }
        RuntimeEvent::RouteRemoved { route_id } => {
            state.routes.remove(route_id);
            state.statuses.remove(route_id);
        }
    }
}

impl Default for InMemoryRuntimeStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RuntimeStoreState::default())),
            journal: None,
        }
    }
}

#[async_trait]
impl RouteRepositoryPort for InMemoryRuntimeStore {
    async fn load(&self, route_id: &str) -> Result<Option<RouteRuntimeAggregate>, CamelError> {
        let guard = self.inner.lock().await;
        Ok(guard.routes.get(route_id).cloned())
    }

    async fn save(&self, aggregate: RouteRuntimeAggregate) -> Result<(), CamelError> {
        let mut guard = self.inner.lock().await;
        guard
            .routes
            .insert(aggregate.route_id().to_string(), aggregate);
        Ok(())
    }

    async fn save_if_version(
        &self,
        aggregate: RouteRuntimeAggregate,
        expected_version: u64,
    ) -> Result<(), CamelError> {
        let mut guard = self.inner.lock().await;
        let route_id = aggregate.route_id().to_string();
        let current = guard.routes.get(&route_id).ok_or_else(|| {
            CamelError::RouteError(format!(
                "optimistic lock conflict for route '{route_id}': route not found"
            ))
        })?;

        if current.version() != expected_version {
            return Err(CamelError::RouteError(format!(
                "optimistic lock conflict for route '{route_id}': expected version {expected_version}, actual {}",
                current.version()
            )));
        }

        guard.routes.insert(route_id, aggregate);
        Ok(())
    }

    async fn delete(&self, route_id: &str) -> Result<(), CamelError> {
        let mut guard = self.inner.lock().await;
        guard.routes.remove(route_id);
        Ok(())
    }
}

#[async_trait]
impl ProjectionStorePort for InMemoryRuntimeStore {
    async fn upsert_status(&self, status: RouteStatusProjection) -> Result<(), CamelError> {
        let mut guard = self.inner.lock().await;
        guard.statuses.insert(status.route_id.clone(), status);
        Ok(())
    }

    async fn get_status(
        &self,
        route_id: &str,
    ) -> Result<Option<RouteStatusProjection>, CamelError> {
        let guard = self.inner.lock().await;
        Ok(guard.statuses.get(route_id).cloned())
    }

    async fn list_statuses(&self) -> Result<Vec<RouteStatusProjection>, CamelError> {
        let guard = self.inner.lock().await;
        Ok(guard.statuses.values().cloned().collect())
    }

    async fn remove_status(&self, route_id: &str) -> Result<(), CamelError> {
        let mut guard = self.inner.lock().await;
        guard.statuses.remove(route_id);
        Ok(())
    }
}

#[async_trait]
impl EventPublisherPort for InMemoryRuntimeStore {
    async fn publish(&self, events: &[RuntimeEvent]) -> Result<(), CamelError> {
        let mut guard = self.inner.lock().await;
        if let Some(journal) = &self.journal {
            journal.append_batch(events).await?;
        }
        guard.events.extend(events.iter().cloned());
        Ok(())
    }
}

#[async_trait]
impl CommandDedupPort for InMemoryRuntimeStore {
    async fn first_seen(&self, command_id: &str) -> Result<bool, CamelError> {
        let mut guard = self.inner.lock().await;
        if !guard.seen.insert(command_id.to_string()) {
            return Ok(false);
        }

        if let Some(journal) = &self.journal
            && let Err(err) = journal.append_command_id(command_id).await
        {
            guard.seen.remove(command_id);
            return Err(err);
        }

        Ok(true)
    }

    async fn forget_seen(&self, command_id: &str) -> Result<(), CamelError> {
        let mut guard = self.inner.lock().await;
        let removed = guard.seen.remove(command_id);
        if removed && let Some(journal) = &self.journal {
            journal.remove_command_id(command_id).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl RuntimeUnitOfWorkPort for InMemoryRuntimeStore {
    async fn persist_upsert(
        &self,
        aggregate: RouteRuntimeAggregate,
        expected_version: Option<u64>,
        projection: RouteStatusProjection,
        events: &[RuntimeEvent],
    ) -> Result<(), CamelError> {
        let mut guard = self.inner.lock().await;
        if let Some(expected) = expected_version {
            let route_id = aggregate.route_id().to_string();
            let current = guard.routes.get(&route_id).ok_or_else(|| {
                CamelError::RouteError(format!(
                    "optimistic lock conflict for route '{route_id}': route not found"
                ))
            })?;
            if current.version() != expected {
                return Err(CamelError::RouteError(format!(
                    "optimistic lock conflict for route '{route_id}': expected version {expected}, actual {}",
                    current.version()
                )));
            }
        }

        if let Some(journal) = &self.journal {
            journal.append_batch(events).await?;
        }

        guard
            .routes
            .insert(aggregate.route_id().to_string(), aggregate);
        guard
            .statuses
            .insert(projection.route_id.clone(), projection);
        guard.events.extend(events.iter().cloned());
        Ok(())
    }

    async fn persist_delete(
        &self,
        route_id: &str,
        events: &[RuntimeEvent],
    ) -> Result<(), CamelError> {
        let mut guard = self.inner.lock().await;
        if let Some(journal) = &self.journal {
            journal.append_batch(events).await?;
        }
        guard.routes.remove(route_id);
        guard.statuses.remove(route_id);
        guard.events.extend(events.iter().cloned());
        Ok(())
    }

    async fn recover_from_journal(&self) -> Result<(), CamelError> {
        let Some(journal) = &self.journal else {
            return Ok(());
        };

        let replayed_events = journal.load_all().await?;
        let replayed_command_ids = journal.load_command_ids().await?;

        let mut guard = self.inner.lock().await;
        guard.routes.clear();
        guard.statuses.clear();
        guard.events.clear();
        guard.seen.clear();

        for event in &replayed_events {
            apply_replayed_event(&mut guard, event);
        }
        guard.events = replayed_events;
        for command_id in replayed_command_ids {
            guard.seen.insert(command_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone)]
    struct ReplayJournal {
        events: Vec<RuntimeEvent>,
    }

    #[async_trait]
    impl RuntimeEventJournalPort for ReplayJournal {
        async fn append_batch(&self, _events: &[RuntimeEvent]) -> Result<(), CamelError> {
            Ok(())
        }

        async fn load_all(&self) -> Result<Vec<RuntimeEvent>, CamelError> {
            Ok(self.events.clone())
        }
    }

    #[tokio::test]
    async fn repo_roundtrip_works() {
        let repo = InMemoryRouteRepository::default();
        repo.save(RouteRuntimeAggregate::new("r1")).await.unwrap();
        assert!(repo.load("r1").await.unwrap().is_some());

        let updated = RouteRuntimeAggregate::from_snapshot(
            "r1",
            crate::lifecycle::domain::RouteRuntimeState::Started,
            1,
        );
        repo.save_if_version(updated.clone(), 0).await.unwrap();
        let loaded = repo.load("r1").await.unwrap().unwrap();
        assert_eq!(loaded.version(), 1);

        let conflict = repo.save_if_version(updated, 0).await.unwrap_err();
        assert!(
            conflict.to_string().contains("optimistic lock conflict"),
            "unexpected conflict error: {conflict}"
        );

        repo.delete("r1").await.unwrap();
        assert!(repo.load("r1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn projection_roundtrip_works() {
        let store = InMemoryProjectionStore::default();
        store
            .upsert_status(RouteStatusProjection {
                route_id: "r1".into(),
                status: "Started".into(),
            })
            .await
            .unwrap();

        let status = store.get_status("r1").await.unwrap();
        assert!(status.is_some());
        assert_eq!(status.unwrap().status, "Started");
        store.remove_status("r1").await.unwrap();
        assert!(store.get_status("r1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn event_publisher_stores_events() {
        let publisher = InMemoryEventPublisher::default();
        publisher
            .publish(&[RuntimeEvent::RouteStarted {
                route_id: "r1".into(),
            }])
            .await
            .unwrap();

        let events = publisher.snapshot().await;
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn command_dedup_detects_duplicates() {
        let dedup = InMemoryCommandDedup::default();
        assert!(dedup.first_seen("c1").await.unwrap());
        assert!(!dedup.first_seen("c1").await.unwrap());
        dedup.forget_seen("c1").await.unwrap();
        assert!(dedup.first_seen("c1").await.unwrap());
        assert!(dedup.first_seen("c2").await.unwrap());
    }

    #[tokio::test]
    async fn runtime_store_uow_persists_all_three_writes() {
        let store = InMemoryRuntimeStore::default();
        let aggregate = RouteRuntimeAggregate::new("uow-r1");
        let projection = RouteStatusProjection {
            route_id: "uow-r1".to_string(),
            status: "Registered".to_string(),
        };
        let events = vec![RuntimeEvent::RouteRegistered {
            route_id: "uow-r1".to_string(),
        }];

        store
            .persist_upsert(aggregate, None, projection.clone(), &events)
            .await
            .unwrap();

        assert!(store.load("uow-r1").await.unwrap().is_some());
        assert_eq!(
            store.get_status("uow-r1").await.unwrap().unwrap(),
            projection
        );
        assert_eq!(store.snapshot_events().await, events);
    }

    #[tokio::test]
    async fn runtime_store_uow_enforces_expected_version() {
        let store = InMemoryRuntimeStore::default();
        let initial = RouteRuntimeAggregate::new("uow-r2");
        let initial_projection = RouteStatusProjection {
            route_id: "uow-r2".to_string(),
            status: "Registered".to_string(),
        };
        store
            .persist_upsert(
                initial,
                None,
                initial_projection,
                &[RuntimeEvent::RouteRegistered {
                    route_id: "uow-r2".to_string(),
                }],
            )
            .await
            .unwrap();

        let started = RouteRuntimeAggregate::from_snapshot(
            "uow-r2",
            crate::lifecycle::domain::RouteRuntimeState::Started,
            1,
        );
        let err = store
            .persist_upsert(
                started,
                Some(99),
                RouteStatusProjection {
                    route_id: "uow-r2".to_string(),
                    status: "Started".to_string(),
                },
                &[RuntimeEvent::RouteStarted {
                    route_id: "uow-r2".to_string(),
                }],
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("optimistic lock conflict"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn replay_start_requested_only_advances_version_once() {
        let store = InMemoryRuntimeStore::default().with_journal(Arc::new(ReplayJournal {
            events: vec![
                RuntimeEvent::RouteRegistered {
                    route_id: "replay-r1".to_string(),
                },
                RuntimeEvent::RouteStartRequested {
                    route_id: "replay-r1".to_string(),
                },
            ],
        }));

        store.recover_from_journal().await.unwrap();
        let aggregate = store.load("replay-r1").await.unwrap().unwrap();

        assert_eq!(aggregate.state(), &RouteRuntimeState::Starting);
        assert_eq!(aggregate.version(), 1);
    }

    #[tokio::test]
    async fn replay_start_requested_then_started_keeps_single_command_version() {
        let store = InMemoryRuntimeStore::default().with_journal(Arc::new(ReplayJournal {
            events: vec![
                RuntimeEvent::RouteRegistered {
                    route_id: "replay-r2".to_string(),
                },
                RuntimeEvent::RouteStartRequested {
                    route_id: "replay-r2".to_string(),
                },
                RuntimeEvent::RouteStarted {
                    route_id: "replay-r2".to_string(),
                },
            ],
        }));

        store.recover_from_journal().await.unwrap();
        let aggregate = store.load("replay-r2").await.unwrap().unwrap();

        assert_eq!(aggregate.state(), &RouteRuntimeState::Started);
        assert_eq!(aggregate.version(), 1);
    }
}
