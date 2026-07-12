//! CronService SPI — trait and types for cron-triggered scheduling backends.
//!
//! The default implementation is `TokioCronService` in `camel-cron`.
//! Future backends (e.g. a persistent Quartz-style scheduler) implement
//! this trait from `component-api` without depending on `camel-cron`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use tokio_util::sync::CancellationToken;

use crate::CamelError;

/// A scheduled cron trigger handed to a [`CronService`].
#[derive(Debug, Clone)]
pub struct CronSchedule {
    /// Normalized 6-field cron expression in `cron` crate format
    /// (`sec min hour dom month dow`). The `CronConsumer` normalizes
    /// the 5-field Unix input from the URI by prepending `0` for seconds.
    pub expression: String,
    /// IANA timezone for evaluating the expression.
    pub time_zone: Tz,
    /// Stable trigger identity — the `name` from the URI path.
    pub trigger_id: String,
    /// Route owning this schedule, if known at Endpoint creation.
    pub route_id: Option<String>,
}

/// Metadata about a single cron fire, passed to [`CronCallback`].
#[derive(Debug, Clone)]
pub struct CronFire {
    /// When the schedule computed this fire should occur.
    pub scheduled_at: DateTime<Utc>,
    /// When the service actually invoked the callback.
    pub fired_at: DateTime<Utc>,
    /// 1-based fire counter.
    pub counter: u64,
}

/// Async, fallible callback invoked on each cron fire.
///
/// The `CronConsumer` builds this from its `ConsumerContext` to submit an
/// Exchange to the pipeline. Returning `Err` propagates the failure through
/// `CronService::run` -> `Consumer::start` to Route supervision (ADR-0007).
pub type CronCallback = Arc<
    dyn Fn(CronFire) -> Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>> + Send + Sync,
>;

/// SPI for cron-triggered scheduling backends.
///
/// The default implementation is `TokioCronService` (in `camel-cron`).
#[async_trait::async_trait]
pub trait CronService: Send + Sync {
    /// Run the scheduling loop until `cancel` is triggered or `callback` fails.
    ///
    /// - Clean cancellation -> return `Ok(())`.
    /// - Callback failure -> return `Err(...)` for ADR-0007 supervision.
    async fn run(
        &self,
        schedule: &CronSchedule,
        callback: CronCallback,
        cancel: CancellationToken,
    ) -> Result<(), CamelError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_schedule_construction() {
        let schedule = CronSchedule {
            // Normalized 6-field format (sec min hour dom month dow)
            expression: "0 0 2 * * *".to_string(),
            time_zone: Tz::UTC,
            trigger_id: "nightly".to_string(),
            route_id: Some("route-1".to_string()),
        };
        assert_eq!(schedule.expression, "0 0 2 * * *");
        assert_eq!(schedule.trigger_id, "nightly");
        assert_eq!(schedule.route_id.as_deref(), Some("route-1"));
    }

    #[test]
    fn cron_fire_construction() {
        let now = Utc::now();
        let fire = CronFire {
            scheduled_at: now,
            fired_at: now,
            counter: 1,
        };
        assert_eq!(fire.counter, 1);
    }
}
