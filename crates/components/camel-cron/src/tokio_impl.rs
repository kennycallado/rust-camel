//! TokioCronService — default in-process CronService implementation.

use camel_component_api::{CamelError, CronCallback, CronFire, CronSchedule, CronService};
use tracing::debug;

/// Default in-process cron scheduler using tokio::time.
pub struct TokioCronService;

#[async_trait::async_trait]
impl CronService for TokioCronService {
    async fn run(
        &self,
        schedule: &CronSchedule,
        callback: CronCallback,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(), CamelError> {
        let cron: cron::Schedule = schedule.expression.parse().map_err(|e| {
            CamelError::EndpointCreationFailed(format!("invalid cron expression: {}", e))
        })?;
        let tz = schedule.time_zone;
        let mut counter: u64 = 0;

        debug!(
            trigger = schedule.trigger_id,
            schedule = schedule.expression,
            "cron service started"
        );

        loop {
            let now = chrono::Utc::now().with_timezone(&tz);
            let next = cron.upcoming(tz).next().ok_or_else(|| {
                CamelError::EndpointCreationFailed(
                    "cron schedule has no future fire times".to_string(),
                )
            })?;

            let delay = (next - now).to_std().map_err(|e| {
                CamelError::EndpointCreationFailed(format!("invalid schedule duration: {}", e))
            })?;

            debug!(
                trigger = schedule.trigger_id,
                next_fire = %next,
                delay_ms = delay.as_millis(),
                "cron waiting for next fire"
            );

            tokio::select! {
                _ = cancel.cancelled() => {
                    debug!(trigger = schedule.trigger_id, "cron cancelled, stopping");
                    return Ok(());
                }
                _ = tokio::time::sleep(delay) => {
                    counter += 1;
                    let fire = CronFire {
                        scheduled_at: next.with_timezone(&chrono::Utc),
                        fired_at: chrono::Utc::now(),
                        counter,
                    };
                    debug!(
                        trigger = schedule.trigger_id,
                        counter, "cron firing"
                    );
                    callback(fire).await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use chrono_tz::Tz;

    fn make_schedule(expr: &str) -> CronSchedule {
        // Normalize 5-field Unix to cron-crate 6-field format
        CronSchedule {
            expression: format!("0 {}", expr),
            time_zone: Tz::UTC,
            trigger_id: "test".to_string(),
            route_id: None,
        }
    }

    #[tokio::test]
    async fn test_cancel_returns_ok() {
        // Far-future schedule so no fire happens; cancel immediately.
        let schedule = make_schedule("0 0 1 1 *");
        let callback: CronCallback = Arc::new(|_fire| Box::pin(async { Ok(()) }));
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel2 = cancel.clone();

        let handle =
            tokio::spawn(async move { TokioCronService.run(&schedule, callback, cancel2).await });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        cancel.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_misfire_does_not_fire_immediately() {
        // Far-future schedule. Start, wait briefly, cancel. No fires.
        let schedule = make_schedule("0 0 1 1 *");
        let counter = Arc::new(AtomicU64::new(0));
        let cb_counter = counter.clone();
        let callback: CronCallback = Arc::new(move |_fire| {
            let c = cb_counter.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel2 = cancel.clone();

        let handle =
            tokio::spawn(async move { TokioCronService.run(&schedule, callback, cancel2).await });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        cancel.cancel();
        let _ = handle.await;

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "should not fire for future schedule"
        );
    }

    #[tokio::test]
    async fn test_next_fire_computed_from_now() {
        // For "* * * * *" normalized to "0 * * * * *" (every minute at sec=0),
        // next fire is within 60 seconds from now.
        let cron: cron::Schedule = "0 * * * * *".parse().unwrap();
        let now = chrono::Utc::now().with_timezone(&Tz::UTC);
        let next = cron.upcoming(Tz::UTC).next().unwrap();
        let delta = next - now;
        assert!(
            delta.num_seconds() >= 0 && delta.num_seconds() < 60,
            "next fire should be within 60s, got {}s",
            delta.num_seconds()
        );
    }

    #[tokio::test]
    async fn test_5_field_expression_parsed() {
        // Verify 5-field cron normalizes and produces upcoming times.
        // "0 2 * * *" → "0 0 2 * * *" (every day at 02:00 UTC)
        for (unix_expr, normalized) in &[
            ("0 2 * * *", "0 0 2 * * *"),
            ("*/5 * * * *", "0 */5 * * * *"),
            ("0 9 * * 1", "0 0 9 * * 1"),
        ] {
            let cron: cron::Schedule = normalized
                .parse()
                .unwrap_or_else(|e| panic!("'{}' (normalized '{}'): {}", unix_expr, normalized, e));
            let next = cron.upcoming(Tz::UTC).next();
            assert!(next.is_some(), "'{}' should have upcoming fires", unix_expr);
        }
    }

    #[test]
    fn test_dst_spring_forward_schedule_has_valid_next_fire() {
        // Spec requirement: a schedule that fires during a DST gap must
        // still produce a valid next fire (chrono-tz resolves the gap).
        // "0 0 * * *" normalized = "0 0 0 * * *" (midnight daily).
        // America/Sao_Paulo springs forward at midnight; chrono-tz resolves.
        let cron: cron::Schedule = "0 0 0 * * *".parse().unwrap();
        let tz = Tz::America__Sao_Paulo;
        let next = cron.upcoming(tz).next();
        assert!(
            next.is_some(),
            "spring-forward schedule must still have a next fire"
        );
    }
}
