//! S2: Unactivated bare controller parks dispatch.
//!
//! Spec: openspec/specs/consumer-activation/spec.md
//! Requirement: "Bare-controller activation of the cohort barrier"
//! Scenario: "Unactivated bare controller parks dispatch"
//!
//! GIVEN a bare DefaultRouteController with a route added and started,
//! and the activation method never called;
//! WHEN a consumer sends an exchange;
//! THEN pipeline dispatch stays parked (barrier contract unchanged) —
//! activation is the bare consumer's explicit responsibility.

use std::sync::Arc;
use std::sync::Mutex;

use camel_api::{Exchange, Message, RouteController};
use camel_component_direct::DirectComponent;
use camel_component_mock::MockComponent;
use camel_core::route::BuilderStep;
use camel_core::route_controller::DefaultRouteController;
use camel_core::{Registry, RouteDefinition};
use tower::ServiceExt;

use crate::common::{TEST_TIMEOUT, test_rt};

/// S2: Unactivated bare controller parks dispatch.
///
/// Sends an exchange into a started route WITHOUT calling
/// `activate_cohort()`. The dispatch must stay parked on the closed cohort
/// gate (proven by a bounded 200ms window where the send task is still
/// pending). After `activate_cohort()` the parked dispatch proceeds and
/// the exchange arrives at the mock endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s2_unactivated_bare_controller_parks_dispatch() {
    let mock = MockComponent::new();
    let registry = Arc::new(Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(Arc::new(mock.clone()));
        guard.register(Arc::new(DirectComponent::new()));
    }
    let mut controller = DefaultRouteController::new(
        Arc::clone(&registry),
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    controller
        .add_route(
            RouteDefinition::new("direct:in", vec![BuilderStep::To("mock:arrival".into())])
                .with_route_id("s2-route"),
        )
        .await
        .expect("route must register");
    controller
        .start_route("s2-route")
        .await
        .expect("route must start");
    // NOTE: activate_cohort() is deliberately NOT called — this is the S2 scenario.

    let direct = registry
        .lock()
        .expect("registry lock")
        .get("direct")
        .expect("direct component registered");
    let endpoint = direct
        .create_endpoint("direct:in", &camel_component_api::NoOpComponentContext)
        .expect("create direct endpoint");
    let producer = endpoint
        .create_producer(test_rt(), &camel_component_api::ProducerContext::new())
        .expect("create direct producer");

    // Spawn the send as a background task so we can probe its completion state.
    let send_task = tokio::spawn(async move {
        producer
            .oneshot(Exchange::new(Message::new("s2-parked")))
            .await
    });

    // Phase 1 — park proof: without activation the dispatch must stay
    // parked on the closed cohort gate. 200ms is ample time for the
    // direct consumer to pick up the exchange and hit the gate; if the
    // gate were open the send would complete well within this window.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        !send_task.is_finished(),
        "send must still be parked on the closed cohort gate without activate_cohort()"
    );

    // Phase 2 — release proof: activate the cohort and the parked dispatch
    // must proceed to the mock endpoint.
    controller.activate_cohort();

    let result = tokio::time::timeout(TEST_TIMEOUT, send_task)
        .await
        .expect("send must complete after cohort activation")
        .expect("send task must not panic");
    result.expect("send must succeed after activation");

    let arrival = mock
        .get_endpoint("arrival")
        .expect("mock endpoint 'arrival' must exist");
    arrival.assert_exchange_count(1).await;
}
