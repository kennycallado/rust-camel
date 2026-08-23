//! Task 3: the intercept-rules freeze contract.

use camel_api::{CamelError, Exchange, Message};
use camel_core::startup_validation::ConfigCheck;

use crate::common::{boot_context, direct_to_mock_route, send_to_direct, skip_to_mock_z};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setting_rules_after_the_first_route_registration_is_rejected() {
    let (mut ctx, mock) = boot_context().await;
    ctx.add_route_definition(direct_to_mock_route())
        .await
        .expect("route must register");

    let err = ctx
        .set_intercept_rules(skip_to_mock_z())
        .await
        .expect_err("rules must be rejected after a route registration");
    match &err {
        CamelError::Config(msg) => assert!(
            msg.contains("frozen"),
            "error must name the freeze reason, got: {msg}"
        ),
        other => panic!("expected Config error, got {other:?}"),
    }

    // The rejection left the route untouched: it still delivers
    // direct:in → mock:out.
    ctx.start().await.expect("context start failed");
    send_to_direct(&ctx, "direct:in", Exchange::new(Message::new("hello"))).await;

    let endpoint = mock
        .get_endpoint("out")
        .expect("mock endpoint 'out' must exist");
    endpoint.assert_exchange_count(1).await;
    let received = endpoint.get_received_exchanges().await;
    assert_eq!(received[0].input.body.as_text(), Some("hello"));

    ctx.stop().await.expect("context stop failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setting_rules_after_start_of_an_empty_context_is_rejected() {
    let (mut ctx, _mock) = boot_context().await;
    ctx.start().await.expect("empty context start failed");

    let err = ctx
        .set_intercept_rules(skip_to_mock_z())
        .await
        .expect_err("rules must be rejected after start with zero routes");
    match &err {
        CamelError::Config(msg) => assert!(
            msg.contains("frozen"),
            "error must name the freeze reason, got: {msg}"
        ),
        other => panic!("expected Config error, got {other:?}"),
    }

    ctx.stop().await.expect("context stop failed");
}

/// A startup check that always fails, used to make `start()` fail closed
/// before any route or start side effect.
struct AlwaysFailingCheck;

impl ConfigCheck for AlwaysFailingCheck {
    fn name(&self) -> &'static str {
        "always-failing"
    }
    fn description(&self) -> &'static str {
        "fails start() for the freeze contract test"
    }
    fn run(&self) -> Result<(), CamelError> {
        Err(CamelError::Config("startup check always fails".into()))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_start_does_not_freeze_rules() {
    let (mut ctx, _mock) = boot_context().await;
    ctx.add_startup_check(Box::new(AlwaysFailingCheck));

    let start_err = ctx.start().await;
    assert!(start_err.is_err(), "start must fail via the startup check");

    ctx.set_intercept_rules(skip_to_mock_z())
        .await
        .expect("rules must still be settable after a failed start");

    ctx.stop().await.expect("context stop failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_restart_does_not_unfreeze_rules() {
    let (mut ctx, _mock) = boot_context().await;
    ctx.add_route_definition(direct_to_mock_route())
        .await
        .expect("route must register");
    ctx.start().await.expect("context start failed");

    ctx.stop().await.expect("context stop failed");
    ctx.start().await.expect("context restart failed");

    let err = ctx
        .set_intercept_rules(skip_to_mock_z())
        .await
        .expect_err("rules must stay frozen across stop/restart");
    match &err {
        CamelError::Config(msg) => assert!(
            msg.contains("frozen"),
            "error must name the freeze reason, got: {msg}"
        ),
        other => panic!("expected Config error, got {other:?}"),
    }

    ctx.stop().await.expect("context stop failed");
}
