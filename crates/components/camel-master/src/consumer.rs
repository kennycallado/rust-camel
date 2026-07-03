use std::sync::Arc;
use std::time::Duration;

use camel_api::{CamelError, MetricsCollector, PlatformService};
use camel_component_api::{Component, NetworkRetryPolicy};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(crate) struct MasterConsumer {
    pub(crate) lock_name: String,
    pub(crate) delegate_uri: String,
    pub(crate) delegate_component: Arc<dyn Component>,
    // TODO(MST-001): MetricsCollector is stored here but never used for emission.
    // Wire into reconcile_event to record leadership transitions and delegate lifecycle.
    pub(crate) metrics: Arc<dyn MetricsCollector>,
    pub(crate) platform_service: Arc<dyn PlatformService>,
    pub(crate) drain_timeout: Duration,
    pub(crate) reconnect: NetworkRetryPolicy,
    pub(crate) leadership_task: Option<JoinHandle<Result<(), CamelError>>>,
    pub(crate) stop_token: Option<CancellationToken>,
    pub(crate) runtime: Arc<dyn camel_component_api::RuntimeObservability>,
}

impl MasterConsumer {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        lock_name: String,
        delegate_uri: String,
        delegate_component: Arc<dyn Component>,
        metrics: Arc<dyn MetricsCollector>,
        platform_service: Arc<dyn PlatformService>,
        drain_timeout: Duration,
        reconnect: NetworkRetryPolicy,
        runtime: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Self {
        Self {
            lock_name,
            delegate_uri,
            delegate_component,
            metrics,
            platform_service,
            drain_timeout,
            reconnect,
            leadership_task: None,
            stop_token: None,
            runtime,
        }
    }
}

pub(crate) enum DelegateState {
    Inactive,
    Active {
        run_token: CancellationToken,
        handle: JoinHandle<Result<(), CamelError>>,
        /// Handle to the epoch-stamping bridge task. Joined by `stop_delegate`
        /// within `drain_timeout`. Aborted on timeout to prevent detached
        /// tasks from sending stale exchanges.
        bridge_handle: Option<JoinHandle<()>>,
    },
}
