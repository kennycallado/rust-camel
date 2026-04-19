use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use camel_api::{
    CamelError, LeaderElector, LeadershipEvent, LeadershipHandle, PlatformError, PlatformIdentity,
};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_controlbus::ControlBusComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use camel_master::MasterBundle;
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

struct ContendedLeaderElector {
    tx: Mutex<Option<watch::Sender<Option<LeadershipEvent>>>>,
    is_leader: Arc<AtomicBool>,
}

impl ContendedLeaderElector {
    fn new() -> Self {
        Self {
            tx: Mutex::new(None),
            is_leader: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn emit(&self, event: LeadershipEvent) {
        self.is_leader.store(
            matches!(event, LeadershipEvent::StartedLeading),
            Ordering::Release,
        );

        match event {
            LeadershipEvent::StartedLeading => eprintln!("[elector] leadership acquired"),
            LeadershipEvent::StoppedLeading => eprintln!("[elector] leadership lost"),
        }

        if let Ok(lock) = self.tx.lock() {
            if let Some(tx) = lock.as_ref() {
                let _ = tx.send(Some(event));
            }
        }
    }
}

#[async_trait::async_trait]
impl LeaderElector for ContendedLeaderElector {
    async fn start(&self, identity: PlatformIdentity) -> Result<LeadershipHandle, PlatformError> {
        eprintln!(
            "[elector] start for node={} (initially follower)",
            identity.node_id
        );

        let mut lock = self
            .tx
            .lock()
            .map_err(|_| PlatformError::NotAvailable("elector lock poisoned".to_string()))?;

        if lock.is_some() {
            return Err(PlatformError::AlreadyStarted);
        }

        let (tx, rx) = watch::channel(None);
        *lock = Some(tx);
        drop(lock);

        let cancel = CancellationToken::new();
        let cancel_wait = cancel.clone();
        let (term_tx, term_rx) = oneshot::channel();
        tokio::spawn(async move {
            cancel_wait.cancelled().await;
            let _ = term_tx.send(());
        });

        Ok(LeadershipHandle::new(
            rx,
            Arc::clone(&self.is_leader),
            cancel,
            term_rx,
        ))
    }
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let elector = Arc::new(ContendedLeaderElector::new());

    let mut ctx = CamelContext::builder()
        .leader_elector(elector.clone())
        .platform_identity(PlatformIdentity::local("example-master-node"))
        .build()
        .await?;

    MasterBundle::from_toml(toml::Value::Table(toml::map::Map::new()))?.register_all(&mut ctx);

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(ControlBusComponent::new());

    let route_1 = RouteBuilder::from("master:mylock:timer:tick?period=1000")
        .route_id("master-route")
        .to("log:info")
        .build()?;

    let route_2 = RouteBuilder::from("timer:status?period=5000")
        .route_id("status-route")
        .to("controlbus:route?routeId=master-route&action=status")
        .to("log:info")
        .build()?;

    ctx.add_route_definition(route_1).await?;
    ctx.add_route_definition(route_2).await?;

    let elector_task = {
        let elector = Arc::clone(&elector);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                elector.emit(LeadershipEvent::StartedLeading).await;

                tokio::time::sleep(Duration::from_secs(6)).await;
                elector.emit(LeadershipEvent::StoppedLeading).await;

                tokio::time::sleep(Duration::from_secs(4)).await;
            }
        })
    };

    ctx.start().await?;

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║   Master Leader + Contention + ControlBus (single process)    ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!("master-route: master:mylock:timer:tick -> log:info");
    println!("status-route: timer:status?period=5000 -> controlbus(status) -> log:info");
    println!("Leadership cycle: follower -> leader -> follower -> leader (repeats)");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();

    elector_task.abort();

    ctx.stop().await?;
    Ok(())
}
