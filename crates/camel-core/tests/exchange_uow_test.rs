//! Integration tests for Exchange UoW layer.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message};
use camel_core::ExchangeUoWLayer;
use tower::Layer;

fn identity() -> BoxProcessor {
    BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
}

fn set_error_processor() -> BoxProcessor {
    BoxProcessor::from_fn(|mut ex: Exchange| {
        Box::pin(async move {
            ex.set_error(CamelError::ProcessorError("forced error".into()));
            Ok(ex)
        })
    })
}

fn counter_hook(counter: Arc<AtomicU64>) -> BoxProcessor {
    BoxProcessor::from_fn(move |ex| {
        counter.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move { Ok(ex) })
    })
}

#[tokio::test]
async fn counter_is_zero_before_and_after_exchange() {
    let c = Arc::new(AtomicU64::new(0));
    let layer = ExchangeUoWLayer::new(Arc::clone(&c), None, None);
    assert_eq!(c.load(Ordering::Relaxed), 0);
    let _ = tower::ServiceExt::oneshot(layer.layer(identity()), Exchange::new(Message::new("x")))
        .await
        .unwrap();
    assert_eq!(c.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn on_complete_fires_exactly_once_on_success() {
    let fired = Arc::new(AtomicU64::new(0));
    let c = Arc::new(AtomicU64::new(0));
    let layer = ExchangeUoWLayer::new(Arc::clone(&c), Some(counter_hook(Arc::clone(&fired))), None);
    tower::ServiceExt::oneshot(layer.layer(identity()), Exchange::new(Message::new("ok")))
        .await
        .unwrap();
    assert_eq!(fired.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn on_failure_fires_when_exchange_has_error() {
    let fired = Arc::new(AtomicU64::new(0));
    let c = Arc::new(AtomicU64::new(0));
    let layer = ExchangeUoWLayer::new(Arc::clone(&c), None, Some(counter_hook(Arc::clone(&fired))));
    tower::ServiceExt::oneshot(
        layer.layer(set_error_processor()),
        Exchange::new(Message::new("e")),
    )
    .await
    .unwrap();
    assert_eq!(fired.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn on_complete_does_not_fire_when_exchange_has_error() {
    let fired = Arc::new(AtomicU64::new(0));
    let c = Arc::new(AtomicU64::new(0));
    let layer = ExchangeUoWLayer::new(Arc::clone(&c), Some(counter_hook(Arc::clone(&fired))), None);
    tower::ServiceExt::oneshot(
        layer.layer(set_error_processor()),
        Exchange::new(Message::new("e")),
    )
    .await
    .unwrap();
    assert_eq!(fired.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn hook_failure_does_not_fail_exchange() {
    let bad_hook = BoxProcessor::from_fn(|_| {
        Box::pin(async { Err(CamelError::ProcessorError("bad hook".into())) })
    });
    let c = Arc::new(AtomicU64::new(0));
    let layer = ExchangeUoWLayer::new(Arc::clone(&c), Some(bad_hook), None);
    let result =
        tower::ServiceExt::oneshot(layer.layer(identity()), Exchange::new(Message::new("ok")))
            .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn multiple_sequential_exchanges_counter_returns_to_zero() {
    let c = Arc::new(AtomicU64::new(0));
    let complete_count = Arc::new(AtomicU64::new(0));
    let layer = ExchangeUoWLayer::new(
        Arc::clone(&c),
        Some(counter_hook(Arc::clone(&complete_count))),
        None,
    );
    for _ in 0..5 {
        tower::ServiceExt::oneshot(
            layer.clone().layer(identity()),
            Exchange::new(Message::new("x")),
        )
        .await
        .unwrap();
    }
    assert_eq!(c.load(Ordering::Relaxed), 0);
    assert_eq!(complete_count.load(Ordering::Relaxed), 5);
}
