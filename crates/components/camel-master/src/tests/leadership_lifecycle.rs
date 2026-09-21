//! leadership lifecycle tests (delegate started/stopped/recreated on epochs). Split from tests.rs by concern (bd rc-ubk1v).
//! Shared mocks/helpers live in the tests module root (tests.rs).

use super::*;

fn build_master_consumer(
    platform_service: Arc<dyn PlatformService>,
    create_consumer_calls: Arc<AtomicUsize>,
    start_calls: Arc<AtomicUsize>,
    delegate_retry_max_attempts: Option<u32>,
) -> MasterConsumer {
    let reconnect = match delegate_retry_max_attempts {
        Some(max) => NetworkRetryPolicy {
            max_attempts: max,
            ..NetworkRetryPolicy::default()
        },
        None => NetworkRetryPolicy {
            max_attempts: 0,
            ..NetworkRetryPolicy::default()
        },
    };
    MasterConsumer::new(
        "lock-a".to_string(),
        "fake:delegate".to_string(),
        Arc::new(FakeDelegateComponent {
            create_consumer_calls,
            start_calls,
        }),
        Arc::new(NoOpMetrics),
        platform_service,
        Duration::from_millis(500),
        reconnect,
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    )
}

#[tokio::test]
async fn starts_delegate_only_after_started_leading() {
    let leadership = Arc::new(FakeLeadershipService::new(None));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let start_calls = Arc::new(AtomicUsize::new(0));
    let mut master = build_master_consumer(
        platform_service,
        Arc::clone(&create_consumer_calls),
        Arc::clone(&start_calls),
        Some(30),
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

    master.start(ctx).await.unwrap();

    sleep(Duration::from_millis(80)).await;
    assert!(rx.try_recv().is_err());
    assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 0);

    leadership.emit(LeadershipEvent::StartedLeading).await;

    let first = timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("epoch-1"));
    assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 1);
    assert_eq!(start_calls.load(Ordering::SeqCst), 1);

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn stops_delegate_on_stopped_leading() {
    let leadership = Arc::new(FakeLeadershipService::new(None));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let start_calls = Arc::new(AtomicUsize::new(0));
    let mut master = build_master_consumer(
        platform_service,
        Arc::clone(&create_consumer_calls),
        Arc::clone(&start_calls),
        Some(30),
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

    master.start(ctx).await.unwrap();
    leadership.emit(LeadershipEvent::StartedLeading).await;
    let _ = timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();

    leadership.emit(LeadershipEvent::StoppedLeading).await;
    sleep(Duration::from_millis(100)).await;
    while rx.try_recv().is_ok() {}
    assert!(
        timeout(Duration::from_millis(120), rx.recv())
            .await
            .is_err()
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn recreates_delegate_on_new_leadership_epoch() {
    let leadership = Arc::new(FakeLeadershipService::new(None));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let start_calls = Arc::new(AtomicUsize::new(0));
    let mut master = build_master_consumer(
        platform_service,
        Arc::clone(&create_consumer_calls),
        Arc::clone(&start_calls),
        Some(30),
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

    master.start(ctx).await.unwrap();

    leadership.emit(LeadershipEvent::StartedLeading).await;
    let first = timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("epoch-1"));

    leadership.emit(LeadershipEvent::StoppedLeading).await;
    sleep(Duration::from_millis(120)).await;

    leadership.emit(LeadershipEvent::StartedLeading).await;
    let second = timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.exchange.input.body.as_text(), Some("epoch-2"));

    assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 2);
    assert_eq!(start_calls.load(Ordering::SeqCst), 2);

    cancel.cancel();
    master.stop().await.unwrap();
}
