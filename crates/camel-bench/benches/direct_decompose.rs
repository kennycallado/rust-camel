//! PER-HOST CONTROL (rc-wijd) — `channel_roundtrip` measures the bare
//! `send_and_wait` mechanics (oneshot, mpsc send, cross-task wakeup, reply)
//! against an echo task with no pipeline: the per-host wakeup-cost baseline
//! for interpreting `direct_hop` gate ratios. The Phase-3 decomposition
//! variants that produced the phase-1 attribution were removed after the
//! gate decision (recorded in the change's bench/phase1.md addendum and
//! bench/phase3.md); this id stays as the ongoing control.

use camel_api::{Exchange, Message};
use camel_component_api::ExchangeEnvelope;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use tokio::sync::{mpsc, oneshot};

fn spawn_echo(rt: &tokio::runtime::Runtime) -> mpsc::Sender<ExchangeEnvelope> {
    let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(256);
    rt.spawn(async move {
        while let Some(envelope) = rx.recv().await {
            let ExchangeEnvelope { exchange, reply_tx } = envelope;
            if let Some(tx) = reply_tx {
                let _ = tx.send(Ok(exchange));
            }
        }
    });
    tx
}

fn bench_channel_control(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .unwrap();
    let echo_tx = spawn_echo(&rt);
    c.bench_function("channel_roundtrip", |b| {
        b.to_async(&rt).iter_batched(
            || Exchange::new(Message::new("hop")),
            |ex| {
                let echo_tx = echo_tx.clone();
                async move {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    echo_tx
                        .send(ExchangeEnvelope {
                            exchange: ex,
                            reply_tx: Some(reply_tx),
                        })
                        .await
                        .unwrap();
                    reply_rx.await.unwrap().unwrap()
                }
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, bench_channel_control);
criterion_main!(benches);
