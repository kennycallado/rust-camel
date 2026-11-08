use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use camel_api::CamelError;
use camel_component_api::Consumer;
use camel_component_api::ConsumerContext;
use camel_component_api::ConsumerStartupMode;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::Event;
use tracing::Subscriber;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

#[derive(Debug, Default)]
struct Warning {
    message: String,
    route_id: String,
    readiness_fallback: Option<bool>,
}

impl Visit for Warning {
    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "readiness_fallback" {
            self.readiness_fallback = Some(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "route_id" => self.route_id = format!("{value:?}"),
            _ => {}
        }
    }
}

#[derive(Default)]
struct Observations {
    warnings: Vec<Warning>,
    warnings_at_stop: Vec<usize>,
}

struct CaptureLayer(Arc<Mutex<Observations>>);

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().level() != &tracing::Level::WARN
            || event.metadata().target() != "camel_core::lifecycle::adapters::consumer_management"
        {
            return;
        }
        let mut warning = Warning::default();
        event.record(&mut warning);
        self.0.lock().unwrap().warnings.push(warning);
    }
}

#[derive(Clone, Copy)]
enum Readiness {
    Signalled,
    Missing,
}

#[derive(Clone, Copy)]
enum Completion {
    BeforeCancellation,
    AfterCancellation,
    BackgroundHandle,
}

#[derive(Clone, Copy)]
struct CompletionCase {
    startup_mode: ConsumerStartupMode,
    readiness: Readiness,
    completion: Completion,
}

struct CompletingConsumer {
    options: CompletionCase,
    observations: Arc<Mutex<Observations>>,
    background_handle: Option<JoinHandle<Result<(), CamelError>>>,
}

#[async_trait::async_trait]
impl Consumer for CompletingConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        if matches!(self.options.readiness, Readiness::Signalled) {
            ctx.mark_ready();
        }
        match self.options.completion {
            Completion::AfterCancellation => ctx.cancel_token().cancel(),
            Completion::BackgroundHandle => {
                self.background_handle = Some(tokio::spawn(async { Ok(()) }));
            }
            Completion::BeforeCancellation => {}
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        let mut observations = self.observations.lock().unwrap();
        let count = observations.warnings.len();
        observations.warnings_at_stop.push(count);
        Ok(())
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        self.options.startup_mode
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.background_handle.take()
    }
}

async fn run_completion(options: CompletionCase, is_resume: bool) -> Observations {
    use tracing_subscriber::prelude::*;

    // A bare global registry keeps shared callsites observable even when sibling
    // tests first execute them without a thread-local capture subscriber.
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    });
    let observations = Arc::new(Mutex::new(Observations::default()));
    let _guard = tracing_subscriber::registry()
        .with(CaptureLayer(Arc::clone(&observations)))
        .set_default();
    let route_id = if is_resume {
        "exit-resume"
    } else {
        "exit-start"
    };
    let (tx, _rx) = mpsc::channel(1);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), route_id.to_owned());
    let consumer = CompletingConsumer {
        options,
        observations: Arc::clone(&observations),
        background_handle: None,
    };
    let (handle, startup, _immediate, _outer, _budget) = super::spawn_consumer_task(
        route_id.to_owned(),
        Box::new(consumer),
        ctx,
        None,
        None,
        is_resume,
    );
    tokio::time::timeout(Duration::from_secs(5), startup.await_ready())
        .await
        .expect("startup must resolve within test budget")
        .expect("finite completion must preserve successful startup");
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("consumer must finish within test budget")
        .expect("consumer must not panic");
    let mut observations = observations.lock().unwrap();
    std::mem::take(&mut *observations)
}

async fn assert_warning(readiness: Readiness, readiness_fallback: bool) {
    for is_resume in [false, true] {
        let observations = self::run_completion(
            CompletionCase {
                startup_mode: ConsumerStartupMode::Explicit,
                readiness,
                completion: Completion::BeforeCancellation,
            },
            is_resume,
        )
        .await;
        assert_eq!(observations.warnings.len(), 1, "one completion warning");
        let warning = &observations.warnings[0];
        assert_eq!(
            warning.route_id,
            if is_resume {
                "exit-resume"
            } else {
                "exit-start"
            }
        );
        assert_eq!(warning.readiness_fallback, Some(readiness_fallback));
        for advice in [
            "stop()",
            "immediately",
            "start()",
            "cancellation",
            "background handle",
        ] {
            assert!(
                warning.message.contains(advice),
                "missing {advice}: {warning:?}"
            );
        }
        assert_eq!(
            observations.warnings_at_stop,
            vec![1],
            "warn before one stop"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_consumer_no_handle_warns_after_ready() {
    self::assert_warning(Readiness::Signalled, false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_consumer_no_handle_warns_with_readiness_fallback() {
    self::assert_warning(Readiness::Missing, true).await;
}

async fn assert_silent(options: CompletionCase) {
    for is_resume in [false, true] {
        let observations = self::run_completion(options, is_resume).await;
        assert!(observations.warnings.is_empty(), "no completion warning");
        assert_eq!(observations.warnings_at_stop, vec![0], "stop exactly once");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_explicit_consumer_no_handle_is_silent() {
    for readiness in [Readiness::Signalled, Readiness::Missing] {
        self::assert_silent(CompletionCase {
            startup_mode: ConsumerStartupMode::Explicit,
            readiness,
            completion: Completion::AfterCancellation,
        })
        .await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_consumer_background_handle_is_silent() {
    self::assert_silent(CompletionCase {
        startup_mode: ConsumerStartupMode::Explicit,
        readiness: Readiness::Signalled,
        completion: Completion::BackgroundHandle,
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn immediate_consumer_no_handle_is_silent() {
    self::assert_silent(CompletionCase {
        startup_mode: ConsumerStartupMode::Immediate,
        readiness: Readiness::Missing,
        completion: Completion::BeforeCancellation,
    })
    .await;
}
