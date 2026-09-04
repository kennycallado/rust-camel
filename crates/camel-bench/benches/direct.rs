use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use camel_api::{BoxProcessor, Exchange, Message, OpaqueProcessor};
use camel_component_api::NoOpComponentContext;
use camel_component_direct::DirectComponent;
use camel_core::{BuilderStep, CamelContext, RouteDefinition};
use criterion::{Criterion, criterion_group, criterion_main};
use tower::ServiceExt;

/// Runtime observability stub for `create_producer`.
fn bench_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(NoOpComponentContext)
}

/// Boot a real `direct:hop` route: the consumer is started through
/// camel-core's route controller (real pipeline, cancellation, startup
/// handshake) — never a hand-constructed `ConsumerContext` — and the bench
/// owns the producer. A second route, `direct:proof`, carries the single
/// task-id-recording step used by the untimed Phase-3 inline-path proof;
/// it is never part of the timed measurement.
#[allow(clippy::type_complexity)]
fn boot_direct_hop(
    rt: &tokio::runtime::Runtime,
) -> (
    CamelContext,
    BoxProcessor,
    Arc<Mutex<Option<tokio::task::Id>>>,
    BoxProcessor,
) {
    rt.block_on(async {
        let mut ctx = CamelContext::builder().build().await.unwrap();
        ctx.register_component(DirectComponent::new());
        // Empty step list: the pipeline is a no-op, so the measurement is
        // the hop itself, not any processor work.
        ctx.add_route_definition(
            RouteDefinition::new("direct:hop", vec![]).with_route_id("direct-hop-bench"),
        )
        .await
        .unwrap();

        // Proof route: the "no-op" step records the tokio task id executing
        // the pipeline. Same context, same component, same dispatch
        // mechanics as `direct:hop` — only this route carries the recorder.
        let pipeline_task: Arc<Mutex<Option<tokio::task::Id>>> = Arc::new(Mutex::new(None));
        ctx.add_route_definition(
            RouteDefinition::new(
                "direct:proof",
                vec![BuilderStep::Processor(OpaqueProcessor(BoxProcessor::new(
                    TaskIdRecorder {
                        slot: Arc::clone(&pipeline_task),
                    },
                )))],
            )
            .with_route_id("direct-proof-bench"),
        )
        .await
        .unwrap();
        ctx.start().await.unwrap();

        let component = ctx.registry().get("direct").unwrap();
        let producer_ctx = ctx.producer_context();
        let endpoint = component.create_endpoint("direct:hop", &ctx).unwrap();
        let producer = endpoint.create_producer(bench_rt(), &producer_ctx).unwrap();

        // Warm dispatch: force consumer registration lookup, channel wiring,
        // and first-use allocations outside the measured loop.
        let _warm = producer
            .clone()
            .oneshot(Exchange::new(Message::new("warm")))
            .await
            .unwrap();

        let proof_endpoint = component.create_endpoint("direct:proof", &ctx).unwrap();
        let proof_producer = proof_endpoint
            .create_producer(bench_rt(), &producer_ctx)
            .unwrap();

        (ctx, producer, pipeline_task, proof_producer)
    })
}

/// Pipeline step for the Phase-3 proof: records the tokio task id executing
/// the step into a shared slot, then passes the exchange through untouched.
#[derive(Clone)]
struct TaskIdRecorder {
    slot: Arc<Mutex<Option<tokio::task::Id>>>,
}

impl tower::Service<Exchange> for TaskIdRecorder {
    type Response = Exchange;
    type Error = camel_component_api::CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let slot = Arc::clone(&self.slot);
        Box::pin(async move {
            *slot.lock().unwrap() = tokio::task::try_id();
            Ok(exchange)
        })
    }
}

/// UNTIMED Phase-3 inline-path proof (runs once, before the criterion
/// loop): the `direct:proof` pipeline step must observe the SAME tokio task
/// id as the producer's dispatch task. Inline dispatch runs the consumer
/// pipeline on the producer's task; a channel fallback runs it on the
/// controller pipeline task — a different id. Panics on mismatch so the
/// gate can never silently measure the channel path.
fn assert_inline_dispatch(
    rt: &tokio::runtime::Runtime,
    proof_producer: BoxProcessor,
    pipeline_task: Arc<Mutex<Option<tokio::task::Id>>>,
) {
    rt.block_on(async move {
        // Run inside a real task so the producer side observes a genuine
        // task id (block_on contexts have none).
        let (producer_id, _reply) = tokio::spawn(async move {
            let producer_id = tokio::task::try_id();
            let reply = proof_producer
                .oneshot(Exchange::new(Message::new("proof")))
                .await
                .unwrap();
            (producer_id, reply)
        })
        .await
        .unwrap();
        let pipeline_id = pipeline_task.lock().unwrap().take();

        assert_eq!(
            producer_id, pipeline_id,
            "channel fallback detected: the direct:proof pipeline executed on \
             task {pipeline_id:?} while the producer dispatched from task \
             {producer_id:?} — the Phase-3 gate would be measuring the \
             channel path, not inline dispatch"
        );
    });
}

/// Benchmarks one full direct hop: producer dispatch → controller-driven
/// consumer with a no-op pipeline → reply. Phase-0 baseline for the
/// direct-inline-dispatch change (see openspec/changes/direct-inline-dispatch).
fn bench_direct_hop(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (_ctx, producer, pipeline_task, proof_producer) = boot_direct_hop(&rt);

    assert_inline_dispatch(&rt, proof_producer, pipeline_task);

    c.bench_function("direct_hop", |b| {
        b.to_async(&rt).iter(|| {
            let producer = producer.clone();
            let ex = Exchange::new(Message::new("hop"));
            async move { producer.oneshot(ex).await.unwrap() }
        })
    });
}

criterion_group!(benches, bench_direct_hop);
criterion_main!(benches);
