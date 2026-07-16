//! `RouteController` trait implementation for `DefaultRouteController`.
//!
//! Extracted from `route_controller.rs` to reduce file size. All lifecycle methods
//! (start, stop, suspend, resume, etc.) live here.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower::Service;
use tracing::{error, info, warn};

use camel_api::{CamelError, NoOpMetrics};
use camel_component_api::{ConcurrencyModel, ConsumerContext, consumer::ExchangeEnvelope};

use crate::lifecycle::adapters::consumer_management;
use crate::lifecycle::adapters::controller_component_context::ControllerComponentContext;
use crate::lifecycle::adapters::route_compiler::CANCEL_TOKEN;
use crate::lifecycle::adapters::route_controller::DefaultRouteController;
#[cfg(test)]
use crate::lifecycle::adapters::route_helpers::emit_start_route_event;
use crate::lifecycle::adapters::route_helpers::{
    handle_is_running, inferred_lifecycle_label, ready_with_backoff,
};
use crate::lifecycle::adapters::route_registry::DEFAULT_SHUTDOWN_TIMEOUT;

#[async_trait::async_trait]
impl camel_api::RouteController for DefaultRouteController {
    async fn start_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        // Check if route exists and can be started.
        {
            let managed = self
                .routes
                .get_mut(route_id)
                .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

            let consumer_running = handle_is_running(&managed.consumer_handle);
            let pipeline_running = handle_is_running(&managed.pipeline_handle);
            if consumer_running && pipeline_running {
                return Ok(());
            }
            if !consumer_running && pipeline_running {
                return Err(CamelError::RouteError(format!(
                    "Route '{}' is suspended; use resume_route() to resume, or stop_route() then start_route() for full restart",
                    route_id
                )));
            }
            if consumer_running && !pipeline_running {
                return Err(CamelError::RouteError(format!(
                    "Route '{}' has inconsistent execution state; stop_route() then retry start_route()",
                    route_id
                )));
            }
        }

        info!(route_id = %route_id, "Starting route");

        // Get the resolved route info
        let (from_uri, pipeline, concurrency) = {
            let managed = self
                .routes
                .get(route_id)
                .expect("invariant: route must exist after prior existence check"); // allow-unwrap
            (
                managed.from_uri.clone(),
                Arc::clone(&managed.pipeline),
                managed.concurrency.clone(),
            )
        };

        // Clone crash notifier for consumer task
        let crash_notifier = self.crash_notifier.clone();
        let runtime_for_consumer = self.runtime.clone();

        let consumer_component_ctx = Arc::new(ControllerComponentContext::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.languages),
            self.tracer_metrics
                .clone()
                .unwrap_or_else(|| Arc::new(NoOpMetrics)),
            Arc::clone(&self.platform_service),
            self.health_registry(),
            Some(route_id.to_string()),
        ));
        let consumer_rt: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::clone(&consumer_component_ctx) as Arc<_>;
        let (mut consumer, consumer_concurrency) = consumer_management::create_route_consumer(
            consumer_rt,
            &self.registry,
            &from_uri,
            consumer_component_ctx.as_ref(),
        )?;

        // Resolve effective concurrency: route override > consumer default
        let effective_concurrency = concurrency.unwrap_or(consumer_concurrency);

        // Get the managed route for mutation
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap

        // Wire security context before spawning consumer
        if let (Some(sp_config), Some(authenticator)) = (
            managed.compiled.security_policy.as_ref(),
            managed.compiled.security_authenticator.as_ref(),
        ) {
            use camel_component_api::SecurityContext;
            let sec_ctx =
                SecurityContext::from_arc(Arc::clone(&sp_config.policy), Arc::clone(authenticator));
            consumer.set_security_context(sec_ctx);
        }

        // Create channel for consumer to send exchanges
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(256);
        // Create child tokens for independent lifecycle control
        let consumer_cancel = managed.consumer_cancel_token.child_token();
        let pipeline_cancel = managed.pipeline_cancel_token.child_token();
        // Clone sender for storage (to reuse on resume)
        let tx_for_storage = tx.clone();
        let consumer_ctx = ConsumerContext::new(tx, consumer_cancel.clone(), route_id.to_string());

        // --- Aggregator v2: check for aggregate route with timeout ---
        let split_clone = managed.aggregate_split.clone();
        if let Some(split) = split_clone {
            return self
                .start_aggregate_route(
                    route_id,
                    split,
                    consumer,
                    consumer_ctx,
                    rx,
                    crash_notifier,
                    runtime_for_consumer,
                    tx_for_storage,
                    pipeline_cancel,
                )
                .await;
        }
        // --- End aggregator v2 branch ---

        // Spawn pipeline task with its own cancellation token
        let pipeline_handle = match effective_concurrency {
            ConcurrencyModel::Sequential => {
                tokio::spawn(async move {
                    loop {
                        // Use select! to exit promptly on cancellation even when idle
                        let envelope = tokio::select! {
                            envelope = rx.recv() => match envelope {
                                Some(e) => e,
                                None => return, // Channel closed
                            },
                            _ = pipeline_cancel.cancelled() => {
                                // Cancellation requested - exit gracefully
                                return;
                            }
                        };
                        let ExchangeEnvelope { exchange, reply_tx } = envelope;

                        // Load current pipeline from ArcSwap (picks up hot-reloaded pipelines)
                        let mut pipeline = pipeline.load().processor.clone_inner();

                        if let Err(e) = ready_with_backoff(&mut pipeline, &pipeline_cancel).await {
                            if let Some(tx) = reply_tx {
                                let _ = tx.send(Err(e));
                            }
                            return;
                        }

                        // B1: scope CANCEL_TOKEN so run_steps can check cancellation
                        // between steps. Per-start task-local — child token expires
                        // when this pipeline task exits; the next start re-scopes a
                        // fresh one (avoids the lifecycle bug where a compiled-in
                        // child token stays cancelled after stop→restart).
                        let cancel = pipeline_cancel.clone();
                        let result = CANCEL_TOKEN
                            .scope(cancel, async move { pipeline.call(exchange).await })
                            .await;
                        if let Some(tx) = reply_tx {
                            let _ = tx.send(result);
                        } else if let Err(ref e) = result {
                            // log-policy: system-broken
                            error!("Pipeline error: {e}");
                        }
                    }
                })
            }
            ConcurrencyModel::Concurrent { max } => {
                let sem = max.map(|n| Arc::new(tokio::sync::Semaphore::new(n)));
                tokio::spawn(async move {
                    loop {
                        // B2 (ADR-0044): acquire permit BEFORE dequeue.
                        // Cancel-aware: route stop is not blocked waiting for a permit.
                        let permit = match &sem {
                            Some(s) => {
                                let acquired = tokio::select! {
                                    p = Arc::clone(s).acquire_owned() => p.expect("semaphore closed"), // allow-unwrap
                                    _ = pipeline_cancel.cancelled() => return,
                                };
                                Some(acquired)
                            }
                            None => None,
                        };

                        let envelope = tokio::select! {
                            envelope = rx.recv() => match envelope {
                                Some(e) => e,
                                None => return,
                            },
                            _ = pipeline_cancel.cancelled() => return,
                        };
                        let ExchangeEnvelope { exchange, reply_tx } = envelope;
                        let pipe_ref = Arc::clone(&pipeline);
                        let cancel = pipeline_cancel.clone();
                        tokio::spawn(async move {
                            // Permit owned by this task — released on completion (RAII).
                            let _permit = permit;

                            // Load current pipeline from ArcSwap
                            let mut pipe = pipe_ref.load().processor.clone_inner();

                            // Wait for service ready with circuit breaker backoff
                            if let Err(e) = ready_with_backoff(&mut pipe, &cancel).await {
                                if let Some(tx) = reply_tx {
                                    let _ = tx.send(Err(e));
                                }
                                return;
                            }

                            // B1: scope CANCEL_TOKEN so run_steps can check
                            // cancellation between steps.
                            let result = CANCEL_TOKEN
                                .scope(cancel, async move { pipe.call(exchange).await })
                                .await;
                            if let Some(tx) = reply_tx {
                                let _ = tx.send(result);
                            } else if let Err(ref e) = result {
                                // log-policy: system-broken
                                error!("Pipeline error: {e}");
                            }
                        });
                    }
                })
            }
        };
        #[cfg(test)]
        emit_start_route_event("pipeline_spawned");

        // Start consumer after pipeline task is spawned to minimize the chance of
        // fire-and-forget events being produced before the pipeline loop is active.
        let consumer_handle = consumer_management::spawn_consumer_task(
            route_id.to_string(),
            consumer,
            consumer_ctx,
            crash_notifier,
            runtime_for_consumer,
            false,
        );
        #[cfg(test)]
        emit_start_route_event("consumer_spawned");

        // Store handles and update status
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        managed.consumer_handle = Some(consumer_handle);
        managed.pipeline_handle = Some(pipeline_handle);
        managed.channel_sender = Some(tx_for_storage);

        info!(route_id = %route_id, "Route started");
        self.health_registry().mark_route_started(route_id);
        Ok(())
    }

    async fn stop_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route_internal(route_id).await?;
        self.health_registry().mark_route_stopped(route_id);
        Ok(())
    }

    async fn restart_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        self.stop_route(route_id).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.start_route(route_id).await
    }

    async fn suspend_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        // Check route exists and state.
        let managed = self
            .routes
            .get_mut(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        let consumer_running = handle_is_running(&managed.consumer_handle);
        let pipeline_running = handle_is_running(&managed.pipeline_handle);

        // Can only suspend from active started state.
        if !consumer_running || !pipeline_running {
            return Err(CamelError::RouteError(format!(
                "Cannot suspend route '{}' with execution lifecycle {}",
                route_id,
                inferred_lifecycle_label(managed)
            )));
        }

        info!(route_id = %route_id, "Suspending route (consumer only, keeping pipeline)");

        // Cancel consumer token only (keep pipeline running)
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        managed.consumer_cancel_token.cancel();

        // Take and join consumer handle
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        let consumer_handle = managed.consumer_handle.take();

        // Wait for consumer task to complete with timeout
        let timeout_result = tokio::time::timeout(DEFAULT_SHUTDOWN_TIMEOUT, async {
            if let Some(handle) = consumer_handle {
                let _ = handle.await;
            }
        })
        .await;

        if timeout_result.is_err() {
            warn!(route_id = %route_id, "Consumer shutdown timed out during suspend");
        }

        // Get the managed route again (can't hold across await)
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap

        // Create fresh cancellation token for consumer (for resume)
        managed.consumer_cancel_token = CancellationToken::new();

        info!(route_id = %route_id, "Route suspended (pipeline still running)");
        self.health_registry().mark_route_stopped(route_id);
        Ok(())
    }

    async fn resume_route(&mut self, route_id: &str) -> Result<(), CamelError> {
        // Check route exists and is Suspended-equivalent execution state.
        let managed = self
            .routes
            .get(route_id)
            .ok_or_else(|| CamelError::RouteError(format!("Route '{}' not found", route_id)))?;

        let consumer_running = handle_is_running(&managed.consumer_handle);
        let pipeline_running = handle_is_running(&managed.pipeline_handle);
        if consumer_running || !pipeline_running {
            return Err(CamelError::RouteError(format!(
                "Cannot resume route '{}' with execution lifecycle {} (expected Suspended)",
                route_id,
                inferred_lifecycle_label(managed)
            )));
        }

        // Get the stored channel sender (must exist for a suspended route)
        let sender = managed.channel_sender.clone().ok_or_else(|| {
            CamelError::RouteError("Suspended route has no channel sender".into())
        })?;

        // Get from_uri and concurrency for creating new consumer
        let from_uri = managed.from_uri.clone();

        info!(route_id = %route_id, "Resuming route (spawning consumer only)");

        let consumer_component_ctx = Arc::new(ControllerComponentContext::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.languages),
            self.tracer_metrics
                .clone()
                .unwrap_or_else(|| Arc::new(NoOpMetrics)),
            Arc::clone(&self.platform_service),
            self.health_registry(),
            Some(route_id.to_string()),
        ));
        let consumer_rt: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::clone(&consumer_component_ctx) as Arc<_>;
        let (mut consumer, _) = consumer_management::create_route_consumer(
            consumer_rt,
            &self.registry,
            &from_uri,
            consumer_component_ctx.as_ref(),
        )?;

        // Wire security context before spawning consumer
        let managed = self
            .routes
            .get(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        if let (Some(sp_config), Some(authenticator)) = (
            managed.compiled.security_policy.as_ref(),
            managed.compiled.security_authenticator.as_ref(),
        ) {
            use camel_component_api::SecurityContext;
            let sec_ctx =
                SecurityContext::from_arc(Arc::clone(&sp_config.policy), Arc::clone(authenticator));
            consumer.set_security_context(sec_ctx);
        }

        // Get the managed route for mutation
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap

        // Create child token for consumer lifecycle
        let consumer_cancel = managed.consumer_cancel_token.child_token();

        let crash_notifier = self.crash_notifier.clone();
        let runtime_for_consumer = self.runtime.clone();

        // Create ConsumerContext with the stored sender
        let consumer_ctx =
            ConsumerContext::new(sender, consumer_cancel.clone(), route_id.to_string());

        // Spawn consumer task
        let consumer_handle = consumer_management::spawn_consumer_task(
            route_id.to_string(),
            consumer,
            consumer_ctx,
            crash_notifier,
            runtime_for_consumer,
            true,
        );

        // Store consumer handle and update status
        let managed = self
            .routes
            .get_mut(route_id)
            .expect("invariant: route must exist after prior existence check"); // allow-unwrap
        managed.consumer_handle = Some(consumer_handle);

        info!(route_id = %route_id, "Route resumed");
        self.health_registry().mark_route_started(route_id);
        Ok(())
    }

    async fn start_all_routes(&mut self) -> Result<(), CamelError> {
        // Only start routes where auto_startup() == true
        // Sort by startup_order() ascending before starting
        let route_ids: Vec<String> = {
            let pairs = self.routes.auto_startup_sorted();
            pairs.into_iter().map(|(id, _)| id).collect()
        };

        info!("Starting {} auto-startup routes", route_ids.len());

        // Collect errors but continue starting remaining routes
        let mut errors: Vec<String> = Vec::new();
        for route_id in route_ids {
            if let Err(e) = self.start_route(&route_id).await {
                errors.push(format!("Route '{}': {}", route_id, e));
            }
        }

        if !errors.is_empty() {
            return Err(CamelError::RouteError(format!(
                "Failed to start routes: {}",
                errors.join(", ")
            )));
        }

        info!("All auto-startup routes started");
        Ok(())
    }

    async fn stop_all_routes(&mut self) -> Result<(), CamelError> {
        // Sort by startup_order descending (reverse order)
        let route_ids: Vec<String> = {
            let pairs = self.routes.shutdown_sorted();
            pairs.into_iter().map(|(id, _)| id).collect()
        };

        info!("Stopping {} routes", route_ids.len());

        for route_id in route_ids {
            let _ = self.stop_route(&route_id).await;
        }

        info!("All routes stopped");
        Ok(())
    }
}
