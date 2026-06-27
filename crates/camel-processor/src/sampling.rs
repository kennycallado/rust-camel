use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CAMEL_STOP, CamelError, Exchange, Value};

/// A sampling processor that passes 1 of every N exchanges (deterministic,
/// counter-based). Non-sampled exchanges get `CamelStop=true` set and return
/// `Ok(exchange)` (drop semantics consistent with `ThrottleStrategy::Drop`).
///
/// # Which exchange passes
///
/// The counter starts at 0 and is incremented **before** the modulo check.
/// Exchange #N passes (when `counter % period == 0`), then #2N, #3N, etc.
///
/// # Lifecycle
///
/// `SamplingService` does NOT implement `StepLifecycle` — the counter is
/// route-scoped and reset on hot-swap (per ADR-0004). No background timers,
/// nothing to drain.
#[derive(Clone)]
pub struct SamplingService {
    period: usize,
    counter: std::sync::Arc<Mutex<usize>>,
}

impl SamplingService {
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "sampling period must be > 0");
        Self {
            period,
            counter: std::sync::Arc::new(Mutex::new(0)),
        }
    }
}

impl Service<Exchange> for SamplingService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let period = self.period;
        let counter = std::sync::Arc::clone(&self.counter);

        Box::pin(async move {
            let passes = {
                let mut c = counter.lock().unwrap(); // allow-unwrap
                *c += 1;
                (*c).is_multiple_of(period)
            };

            if passes {
                Ok(exchange)
            } else {
                exchange.set_property(CAMEL_STOP, Value::Bool(true));
                Ok(exchange)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;
    use tower::ServiceExt;

    fn make_exchange() -> Exchange {
        Exchange::new(Message::new("test"))
    }

    #[test]
    fn period_0_rejected() {
        let result = std::panic::catch_unwind(|| {
            SamplingService::new(0);
        });
        assert!(result.is_err(), "zero period should panic");
    }

    #[tokio::test]
    async fn period_1_passes_all() {
        let mut svc = SamplingService::new(1);

        for _ in 0..5 {
            let result = svc.ready().await.unwrap().call(make_exchange()).await;
            assert!(result.is_ok(), "all exchanges should pass with period=1");
            let ex = result.unwrap();
            assert_ne!(
                ex.property(CAMEL_STOP),
                Some(&Value::Bool(true)),
                "passing exchange should NOT have CamelStop"
            );
        }
    }

    #[tokio::test]
    async fn period_3_passes_every_3rd() {
        let mut svc = SamplingService::new(3);

        // Exchange 1 — should NOT pass (counter=1, 1%3=1 != 0)
        let ex = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange())
            .await
            .unwrap();
        assert_eq!(
            ex.property(CAMEL_STOP),
            Some(&Value::Bool(true)),
            "exchange 1 should be dropped (CamelStop=true)"
        );

        // Exchange 2 — should NOT pass (counter=2, 2%3=2 != 0)
        let ex = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange())
            .await
            .unwrap();
        assert_eq!(
            ex.property(CAMEL_STOP),
            Some(&Value::Bool(true)),
            "exchange 2 should be dropped (CamelStop=true)"
        );

        // Exchange 3 — should pass (counter=3, 3%3=0)
        let ex = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange())
            .await
            .unwrap();
        assert_ne!(
            ex.property(CAMEL_STOP),
            Some(&Value::Bool(true)),
            "exchange 3 should pass (CamelStop not set)"
        );

        // Exchange 4 — should NOT pass (counter=4, 4%3=1 != 0)
        let ex = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange())
            .await
            .unwrap();
        assert_eq!(
            ex.property(CAMEL_STOP),
            Some(&Value::Bool(true)),
            "exchange 4 should be dropped (CamelStop=true)"
        );

        // Exchange 5 — should NOT pass (counter=5, 5%3=2 != 0)
        let ex = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange())
            .await
            .unwrap();
        assert_eq!(
            ex.property(CAMEL_STOP),
            Some(&Value::Bool(true)),
            "exchange 5 should be dropped (CamelStop=true)"
        );

        // Exchange 6 — should pass (counter=6, 6%3=0)
        let ex = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange())
            .await
            .unwrap();
        assert_ne!(
            ex.property(CAMEL_STOP),
            Some(&Value::Bool(true)),
            "exchange 6 should pass (CamelStop not set)"
        );
    }

    #[tokio::test]
    async fn non_sampled_sets_camel_stop() {
        let mut svc = SamplingService::new(10);

        // First exchange never passes with period=10 (counter=1, 1%10=1)
        let ex = svc
            .ready()
            .await
            .unwrap()
            .call(make_exchange())
            .await
            .unwrap();
        assert_eq!(
            ex.property(CAMEL_STOP),
            Some(&Value::Bool(true)),
            "non-sampled exchange must have CamelStop=true"
        );

        // Verify the exchange is otherwise intact
        if let camel_api::body::Body::Text(ref t) = ex.input.body {
            assert_eq!(t, "test", "exchange body should be preserved");
        } else {
            panic!("expected Text body");
        }
    }
}
