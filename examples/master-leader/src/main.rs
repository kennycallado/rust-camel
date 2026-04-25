use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use camel_api::{
    CamelError, LeadershipEvent, LeadershipHandle, LeadershipService, NoopReadinessGate,
    PlatformError, PlatformIdentity, PlatformService, ReadinessGate,
};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_controlbus::ControlBusComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use camel_master::MasterBundle;
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

/// Simulated leadership that broadcasts events to all subscribers.
struct SimulatedLeadershipService {
    senders: Mutex<Vec<watch::Sender<Option<LeadershipEvent>>>>,
    is_leader: Arc<AtomicBool>,
}

impl SimulatedLeadershipService {
    fn new() -> Self {
        Self {
            senders: Mutex::new(Vec::new()),
            is_leader: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn emit(&self, event: LeadershipEvent) {
        self.is_leader.store(
            matches!(event, LeadershipEvent::StartedLeading),
            Ordering::Release,
        );

        match event {
            LeadershipEvent::StartedLeading => eprintln!("[platform] leadership acquired"),
            LeadershipEvent::StoppedLeading => eprintln!("[platform] leadership lost"),
        }

        let mut senders = self.senders.lock().await;
        senders.retain(|tx| tx.send(Some(event.clone())).is_ok());
    }
}

#[async_trait::async_trait]
impl LeadershipService for SimulatedLeadershipService {
    async fn start(&self, lock_name: &str) -> Result<LeadershipHandle, PlatformError> {
        eprintln!("[platform] start leadership for lock={lock_name}");

        let (tx, rx) = watch::channel(None);
        self.senders.lock().await.push(tx);

        let cancel = CancellationToken::new();
        let cancel_wait = cancel.clone();
        let (term_tx, term_rx) = tokio::sync::oneshot::channel();
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

struct SimulatedPlatformService {
    identity: PlatformIdentity,
    readiness_gate: Arc<dyn ReadinessGate>,
    leadership: Arc<SimulatedLeadershipService>,
}

impl PlatformService for SimulatedPlatformService {
    fn identity(&self) -> PlatformIdentity {
        self.identity.clone()
    }

    fn readiness_gate(&self) -> Arc<dyn ReadinessGate> {
        Arc::clone(&self.readiness_gate)
    }

    fn leadership(&self) -> Arc<dyn LeadershipService> {
        Arc::clone(&self.leadership) as Arc<dyn LeadershipService>
    }
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let leadership = Arc::new(SimulatedLeadershipService::new());
    let platform = Arc::new(SimulatedPlatformService {
        identity: PlatformIdentity::local("example-master-node"),
        readiness_gate: Arc::new(NoopReadinessGate),
        leadership: Arc::clone(&leadership),
    });

    let mut ctx = CamelContext::builder()
        .platform_service(platform.clone())
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

    let platform_task = {
        let leadership = Arc::clone(&leadership);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                leadership.emit(LeadershipEvent::StartedLeading).await;

                tokio::time::sleep(Duration::from_secs(6)).await;
                leadership.emit(LeadershipEvent::StoppedLeading).await;

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

    platform_task.abort();

    ctx.stop().await?;
    Ok(())
}
