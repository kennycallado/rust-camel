mod support;

use std::sync::Arc;
use std::time::Duration;

use camel_api::CamelError;
use camel_component_api::{
    Component, Consumer, ConsumerContext, ExchangeEnvelope, NoOpComponentContext,
    NoopRuntimeObservability,
};
use camel_component_mail::{MailComponent, MailComponentConfig, MailScheme};
use support::docker::nanos;
use support::greenmail::GreenMailFixture;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

type Received = Arc<Mutex<mpsc::Receiver<camel_api::Exchange>>>;

fn imap_component() -> MailComponent {
    // FIXTURE-ONLY login for the GreenMail container.
    let cfg: MailComponentConfig =
        toml::from_str("[accounts.main]\nusername = \"user\"\npassword = \"pass\"\n")
            .expect("account config");
    MailComponent::new(MailScheme::Imap, Arc::new(cfg))
}

struct Running {
    token: CancellationToken,
    consumer: JoinHandle<Result<(), CamelError>>,
    route: JoinHandle<()>,
    received: Received,
}

fn start_consumer(uri: &str, route_fails: bool) -> Running {
    let endpoint = imap_component()
        .create_endpoint(uri, &NoOpComponentContext)
        .expect("imap endpoint");
    let mut consumer: Box<dyn Consumer> = endpoint
        .create_consumer(Arc::new(NoopRuntimeObservability))
        .expect("consumer");
    let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let token = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, token.clone(), "greenmail-route".to_string());
    let (seen_tx, seen_rx) = mpsc::channel(16);
    let route = tokio::spawn(async move {
        while let Ok(Some(env)) = timeout(Duration::from_secs(120), rx.recv()).await {
            let _ = seen_tx.send(env.exchange.clone()).await;
            if let Some(reply) = env.reply_tx {
                let result = if route_fails {
                    Err(CamelError::ProcessorError("route failed".into()))
                } else {
                    Ok(env.exchange)
                };
                let _ = reply.send(result);
            }
        }
    });
    let consumer = tokio::spawn(async move { consumer.start(ctx).await });
    Running {
        token,
        consumer,
        route,
        received: Arc::new(Mutex::new(seen_rx)),
    }
}

async fn stop(running: Running) -> Result<(), CamelError> {
    running.token.cancel();
    let start_result = timeout(Duration::from_secs(30), running.consumer)
        .await
        .expect("consumer join timeout")
        .expect("consumer join");
    timeout(Duration::from_secs(30), running.route)
        .await
        .expect("route join timeout")
        .expect("route join");
    start_result
}

async fn start_fixture() -> GreenMailFixture {
    timeout(Duration::from_secs(120), GreenMailFixture::start())
        .await
        .expect("infra-unavailable: greenmail start timeout")
}

fn consumer_uri(fx: &GreenMailFixture) -> String {
    format!(
        "imap://127.0.0.1:{}?allowCleartext=true&initialDelay=0&delay=500",
        fx.imap_port
    )
}

#[tokio::test]
async fn greenmail_success_sets_seen() {
    if !support::docker::gate_active() {
        return;
    }
    let fx = start_fixture().await;
    let subject = format!("ok-{}", nanos());
    fx.seed(&subject).await;
    let running = start_consumer(&consumer_uri(&fx), false);
    let ex = support::recv_with_timeout(&running.received, Duration::from_secs(30))
        .await
        .expect("exchange within 30s");
    assert_eq!(
        ex.input
            .header("CamelMailInSubject")
            .and_then(|v| v.as_str().map(str::to_string)),
        Some(subject.clone())
    );
    assert_eq!(fx.wait_seen(&subject).await, Some(true));
    let _ = stop(running).await;
}

#[tokio::test]
async fn greenmail_failed_route_leaves_unseen() {
    if !support::docker::gate_active() {
        return;
    }
    let fx = start_fixture().await;
    let subject = format!("fail-{}", nanos());
    fx.seed(&subject).await;
    let running = start_consumer(&consumer_uri(&fx), true);
    let ex = support::recv_with_timeout(&running.received, Duration::from_secs(30))
        .await
        .expect("exchange within 30s");
    assert_eq!(
        ex.input
            .header("CamelMailInSubject")
            .and_then(|v| v.as_str().map(str::to_string)),
        Some(subject.clone())
    );
    let _ = stop(running).await;
    assert_eq!(fx.seen_flags(&subject).await, Some(false));
}

fn subject_header(ex: &camel_api::Exchange) -> Option<String> {
    ex.input
        .header("CamelMailInSubject")
        .and_then(|v| v.as_str().map(str::to_string))
}

#[tokio::test]
async fn greenmail_delete() {
    if !support::docker::gate_active() {
        return;
    }
    let fx = start_fixture().await;
    let (uidplus, _) = fx.capabilities().await;
    let subject = format!("del-{}", nanos());
    fx.seed(&subject).await;
    let uri = format!("{}&delete=true", consumer_uri(&fx));
    let running = start_consumer(&uri, false);
    if uidplus {
        let ex = support::recv_with_timeout(&running.received, Duration::from_secs(30))
            .await
            .expect("exchange within 30s");
        assert_eq!(subject_header(&ex), Some(subject.clone()));
        fx.wait_count("INBOX", &subject, 0).await;
        stop(running).await.expect("consumer start");
        assert_eq!(fx.count_subject("INBOX", &subject).await, 0);
    } else {
        let err = stop(running).await.expect_err("must fail closed");
        assert!(err.to_string().contains("UIDPLUS"), "{err}");
        assert_eq!(fx.count_subject("INBOX", &subject).await, 1);
    }
}

#[tokio::test]
async fn greenmail_move_to() {
    if !support::docker::gate_active() {
        return;
    }
    let fx = start_fixture().await;
    let (uidplus, mv) = fx.capabilities().await;
    eprintln!("greenmail caps: uidplus={uidplus} move={mv}");
    fx.create_folder("Done").await;
    let subject = format!("mv-{}", nanos());
    fx.seed(&subject).await;
    let uri = format!("{}&moveTo=Done", consumer_uri(&fx));
    let running = start_consumer(&uri, false);
    if mv || uidplus {
        let ex = support::recv_with_timeout(&running.received, Duration::from_secs(30))
            .await
            .expect("exchange within 30s");
        assert_eq!(subject_header(&ex), Some(subject.clone()));
        fx.wait_count("Done", &subject, 1).await;
        fx.wait_count("INBOX", &subject, 0).await;
        stop(running).await.expect("consumer start");
        assert_eq!(fx.count_subject("Done", &subject).await, 1);
        assert_eq!(fx.count_subject("INBOX", &subject).await, 0);
    } else {
        let err = stop(running).await.expect_err("must fail closed");
        assert!(err.to_string().contains("MOVE or UIDPLUS"), "{err}");
    }
}

#[tokio::test]
async fn greenmail_copy_to_then_seen() {
    if !support::docker::gate_active() {
        return;
    }
    let fx = start_fixture().await;
    fx.create_folder("Archive").await;
    let subject = format!("cp-{}", nanos());
    fx.seed(&subject).await;
    let uri = format!("{}&copyTo=Archive", consumer_uri(&fx));
    let running = start_consumer(&uri, false);
    let ex = support::recv_with_timeout(&running.received, Duration::from_secs(30))
        .await
        .expect("exchange within 30s");
    assert_eq!(subject_header(&ex), Some(subject.clone()));
    fx.wait_seen(&subject).await;
    stop(running).await.expect("consumer start");
    assert_eq!(fx.count_subject("Archive", &subject).await, 1);
    assert_eq!(fx.seen_flags(&subject).await, Some(true));
}

#[tokio::test]
async fn greenmail_poll_enrich() {
    if !support::docker::gate_active() {
        return;
    }
    let fx = start_fixture().await;
    let subject = format!("pe-{}", nanos());
    fx.seed(&subject).await;
    let uri = format!("imap://127.0.0.1:{}?allowCleartext=true", fx.imap_port);
    let endpoint = imap_component()
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("imap endpoint");
    let mut pc = endpoint.polling_consumer().expect("polling consumer");
    let ex = timeout(Duration::from_secs(30), pc.receive(Duration::from_secs(10)))
        .await
        .expect("receive bound")
        .expect("receive")
        .expect("one message");
    assert_eq!(subject_header(&ex), Some(subject.clone()));
    assert_eq!(fx.seen_flags(&subject).await, Some(true));
    let second = timeout(Duration::from_secs(30), pc.receive(Duration::from_secs(2)))
        .await
        .expect("receive bound")
        .expect("receive");
    assert!(second.is_none());
}
