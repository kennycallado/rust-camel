//! In-memory direct component for rust-camel — synchronous point-to-point
//! dispatch between routes sharing the same context. The producer submits
//! each exchange directly to the consumer route's pipeline task via
//! `ConsumerContext::send_and_wait`; there is no per-message channel hop
//! inside camel-direct.
//!
//! Main types: `DirectComponent`, `DirectEndpoint`, `DirectConsumer`, `DirectProducer`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tower::Service;

use camel_component_api::InlineRouteDispatcher;
use camel_component_api::UriConfig;
use camel_component_api::parse_uri;
use camel_component_api::{BoxProcessor, CamelError, Exchange};
use camel_component_api::{
    Component, ComponentMetadata, Consumer, ConsumerContext, ConsumerStartupMode, Endpoint,
    ProducerContext,
};
use tracing::{debug, error, info};

mod inline_guard;

// ---------------------------------------------------------------------------
// Shared state: maps endpoint names to their registered consumer's route
// submission context. The producer dispatches by calling `send_and_wait` on
// the stored context; the `closed` flag replaces the mpsc sender's
// `is_closed()` liveness signal for crashed-consumer detection.
// ---------------------------------------------------------------------------

/// A registered direct consumer: its route submission context plus the
/// liveness flag owned by the consumer's `start()` task. The optional
/// `dispatcher` carries the inline-dispatch capability the core runtime
/// published on the context (absent when the route's concurrency model does
/// not permit inline dispatch).
struct DirectEntry {
    ctx: ConsumerContext,
    closed: Arc<AtomicBool>,
    dispatcher: Option<Arc<dyn InlineRouteDispatcher>>,
}

type DirectRegistry = Arc<Mutex<HashMap<String, DirectEntry>>>;

/// Sets the owning consumer's `closed` flag on drop, mirroring the
/// receiver-drop signal of the previous channel design: any exit path from
/// `DirectConsumer::start` — normal return, panic, or task abort — marks the
/// registry entry stale so a replacement consumer may overwrite it.
struct CloseGuard(Arc<AtomicBool>);

impl Drop for CloseGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Validate the direct endpoint name (the part after `direct:`).
fn validate_name(name: &str) -> Result<(), CamelError> {
    if name.trim().is_empty() {
        return Err(CamelError::InvalidUri(
            "direct: endpoint name must not be empty".to_string(),
        ));
    }
    if name.contains(char::is_whitespace) {
        return Err(CamelError::InvalidUri(
            "direct: endpoint name must not contain whitespace".to_string(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// DirectConfig
// ---------------------------------------------------------------------------

/// Configuration for Direct endpoints parsed from URIs.
///
/// URI format: `direct:name[?timeout_ms=30000]`
///
/// Example: `direct:foo` creates an endpoint named "foo"
#[derive(Debug, Clone, UriConfig)]
#[uri_scheme = "direct"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "direct",
        description = "Synchronous in-memory direct invocation between routes",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub struct DirectConfig {
    /// Endpoint name (path portion).
    pub name: String,
    /// Timeout in milliseconds for producer `call()`. Defaults to 30 000 ms.
    #[uri_param(
        name = "timeout_ms",
        default = "30000",
        desc = "Producer call timeout in milliseconds"
    )]
    pub timeout_ms: Option<u64>,
    /// When false, skip readiness error if no consumer registered.
    #[uri_param(
        name = "failIfNoConsumers",
        default = "true",
        desc = "Fail if no consumer registered for the name"
    )]
    pub fail_if_no_consumers: Option<bool>,
}

impl DirectConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != "direct" {
            return Err(CamelError::InvalidUri(format!(
                "invalid scheme '{}', expected 'direct'",
                parts.scheme
            )));
        }

        let parse_bool = |name: &str, value: &str| -> Result<bool, CamelError> {
            match value.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Ok(true),
                "false" | "0" | "no" => Ok(false),
                _ => Err(CamelError::InvalidUri(format!(
                    "invalid value for {}: invalid boolean value: '{}'",
                    name, value
                ))),
            }
        };

        let timeout_ms = parts
            .params
            .get("timeout_ms")
            .map(|v| {
                v.parse::<u64>().map_err(|e| {
                    CamelError::InvalidUri(format!("invalid value for timeout_ms: {}", e))
                })
            })
            .transpose()?;

        if parts.params.contains_key("block") {
            return Err(CamelError::InvalidUri("block is not supported".into()));
        }

        let fail_if_no_consumers = parts
            .params
            .get("fail_if_no_consumers")
            .or_else(|| parts.params.get("failIfNoConsumers"))
            .map(|v| parse_bool("fail_if_no_consumers", v))
            .transpose()?;

        if parts.params.contains_key("exchange_pattern")
            || parts.params.contains_key("exchangePattern")
        {
            return Err(CamelError::InvalidUri(
                "exchange_pattern is not supported".into(),
            ));
        }

        Ok(Self {
            name: parts.path,
            timeout_ms,
            fail_if_no_consumers,
        })
    }
}

// ---------------------------------------------------------------------------
// DirectComponent
// ---------------------------------------------------------------------------

/// The Direct component provides in-memory synchronous communication between
/// routes.
///
/// URI format: `direct:name`
///
/// A producer sending to `direct:foo` will block until the consumer on
/// `direct:foo` has finished processing the exchange.
pub struct DirectComponent {
    registry: DirectRegistry,
}

impl DirectComponent {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for DirectComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for DirectComponent {
    fn scheme(&self) -> &str {
        "direct"
    }

    fn metadata(&self) -> ComponentMetadata {
        DirectConfig::metadata()
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = DirectConfig::from_uri(uri)?;
        validate_name(&config.name)?;
        let name = config.name.clone();
        debug!(endpoint_name = %name, "direct endpoint created");
        Ok(Box::new(DirectEndpoint {
            uri: uri.to_string(),
            config,
            registry: Arc::clone(&self.registry),
        }))
    }
}

// ---------------------------------------------------------------------------
// DirectEndpoint
// ---------------------------------------------------------------------------

struct DirectEndpoint {
    uri: String,
    config: DirectConfig,
    registry: DirectRegistry,
}

impl Endpoint for DirectEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(DirectConsumer::new(
            self.config.name.clone(),
            Arc::clone(&self.registry),
        )))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(DirectProducer {
            name: self.config.name.clone(),
            registry: Arc::clone(&self.registry),
            config: self.config.clone(),
            semaphore: Arc::new(Semaphore::new(1)),
            fail_if_no_consumers: self.config.fail_if_no_consumers,
            runtime: rt,
        }))
    }
}

// ---------------------------------------------------------------------------
// DirectConsumer
// ---------------------------------------------------------------------------

/// The Direct consumer registers its route submission context in the shared
/// registry so producers can dispatch exchanges into its pipeline directly.
struct DirectConsumer {
    name: String,
    registry: DirectRegistry,
    cancel: Option<CancellationToken>,
}

impl DirectConsumer {
    fn new(name: String, registry: DirectRegistry) -> Self {
        Self {
            name,
            registry,
            cancel: None,
        }
    }
}

#[async_trait]
impl Consumer for DirectConsumer {
    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        // Liveness flag: set by the guard on every exit path from this task.
        let closed = Arc::new(AtomicBool::new(false));
        let _close_guard = CloseGuard(Arc::clone(&closed));

        // Capture the inline-dispatch capability once: the core runtime sets
        // it before start(), and the registry entry must carry the same
        // snapshot for the whole consumer lifetime.
        let dispatcher = context.inline_dispatcher();

        // Register our submission context so producers can dispatch to us.
        {
            let mut reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = reg.get(&self.name)
                && !existing.closed.load(Ordering::Acquire)
            {
                return Err(CamelError::EndpointCreationFailed(format!(
                    "direct endpoint '{}' already has a registered consumer",
                    self.name
                )));
            }
            reg.insert(
                self.name.clone(),
                DirectEntry {
                    ctx: context.clone(),
                    closed: Arc::clone(&closed),
                    dispatcher,
                },
            );
        }

        context.mark_ready();

        let name = self.name.clone();
        let registry = Arc::clone(&self.registry);
        let cancel = context.cancel_token();
        let cancel_clone = cancel.clone();

        info!(endpoint_name = %self.name, "direct consumer started");

        self.cancel = Some(cancel);

        // No receive loop: producers submit directly through the registered
        // context. Park until shutdown.
        cancel_clone.cancelled().await;

        // Cleanup: remove from registry on exit (the guard sets `closed`).
        {
            let mut reg = registry.lock().unwrap_or_else(|e| e.into_inner());
            reg.remove(&name);
        }

        debug!(endpoint_name = %name, "direct consumer stopped");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // Cancel the consumer loop if we have a cancellation token.
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }

        let mut reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        reg.remove(&self.name);

        debug!(endpoint_name = %self.name, "direct consumer stopped");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DirectProducer
// ---------------------------------------------------------------------------

/// The Direct producer sends an exchange to the named direct endpoint and
/// waits for the reply (synchronous in-memory call): it submits the exchange
/// into the consumer route's pipeline through the registered
/// `ConsumerContext::send_and_wait`.
struct DirectProducer {
    name: String,
    registry: DirectRegistry,
    config: DirectConfig,
    semaphore: Arc<Semaphore>,
    fail_if_no_consumers: Option<bool>,
    runtime: Arc<dyn camel_component_api::RuntimeObservability>,
}

impl Clone for DirectProducer {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            registry: self.registry.clone(),
            config: self.config.clone(),
            semaphore: self.semaphore.clone(),
            fail_if_no_consumers: self.fail_if_no_consumers,
            runtime: Arc::clone(&self.runtime),
        }
    }
}

/// Effective dispatch timeout shared by the channel and inline paths —
/// single construction site so the 30s default cannot drift between paths
/// (inline timeout parity).
fn effective_dispatch_timeout(timeout_ms: Option<u64>) -> Duration {
    Duration::from_millis(timeout_ms.unwrap_or(30_000))
}

/// Timeout error shared by the channel and inline paths — single
/// construction site so the error text cannot drift between paths
/// (inline timeout parity).
fn dispatch_timeout_error(name: &str) -> CamelError {
    CamelError::ProcessorError(format!("direct:{name} call timed out"))
}

impl Service<Exchange> for DirectProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Check that the endpoint is registered. Permits are NOT acquired
        // here: a permit reserved in poll_ready is held across the
        // poll_ready/call boundary, wedging the semaphore when a wrapping
        // Service re-readies a clone (tower contract: call() may only
        // reserve resources for its own future).
        let reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        match reg.get(&self.name) {
            None => {
                if self.fail_if_no_consumers != Some(false) {
                    return Poll::Ready(Err(CamelError::EndpointCreationFailed(format!(
                        "direct endpoint '{}' not registered",
                        self.name
                    ))));
                }
                Poll::Ready(Ok(()))
            }
            Some(entry) if entry.closed.load(Ordering::Acquire) => {
                Poll::Ready(Err(CamelError::EndpointCreationFailed(format!(
                    "direct endpoint '{}' consumer closed",
                    self.name
                ))))
            }
            Some(_) => Poll::Ready(Ok(())),
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let name = self.name.clone();
        let registry = Arc::clone(&self.registry);
        let semaphore = Arc::clone(&self.semaphore);
        let runtime = Arc::clone(&self.runtime);
        let timeout = effective_dispatch_timeout(self.config.timeout_ms);
        let exchange_id = exchange.correlation_id.clone();

        debug!(
            endpoint_name = %name,
            exchange_id = %exchange_id,
            "direct producer call entry"
        );

        Box::pin(async move {
            // One timed section covers BOTH paths: the boundary spans the
            // registry lookup, the per-path serialization wait (channel
            // permit or dispatcher admission), and the dispatch itself,
            // so neither path's timeout can drift (inline timeout parity).
            let timed = tokio::time::timeout(timeout, async {
                // Registry lookup: a missing entry is an unhandled dispatch
                // failure like any other — it flows through the same
                // emission site below instead of `?`-exiting before it
                // (b′ visibility for the no-consumer case).
                let looked_up = {
                    let reg = registry.lock().unwrap_or_else(|e| e.into_inner());
                    reg.get(&name)
                        .map(|entry| (entry.ctx.clone(), entry.dispatcher.clone()))
                };

                let (result, entry_ctx) = match looked_up {
                    None => {
                        let err = CamelError::EndpointCreationFailed(format!(
                            "no consumer registered for direct:{name}"
                        ));
                        // No warn here: the shared emission site below logs
                        // at error! with the b′ increment for this failure
                        // (single log per failed dispatch — review finding).
                        (Err(err), None)
                    }
                    Some((ctx, dispatcher)) => {
                        let result = match dispatcher {
                            // Inline fast path: run the consumer pipeline on this
                            // task. The endpoint semaphore is skipped — the
                            // dispatcher's admission mutex is the single
                            // serializer for inline dispatches. Cycle/depth guard
                            // rejection maps straight out; there is no channel
                            // fallback on guard rejection.
                            Some(d) => {
                                let dispatch_future = d.dispatch(exchange);
                                let guard_name = name.clone();
                                inline_guard::with_inline_stack(async move {
                                    // Per-dispatch guard: drops after the dispatch
                                    // completes, unwinding before the reply.
                                    let _guard = inline_guard::enter(&guard_name)?;
                                    dispatch_future.await
                                })
                                .await
                            }
                            // Channel path (Phase 1 semantics): submit through the
                            // consumer context under the endpoint's sole permit.
                            // Covers Concurrent consumers and
                            // capability-unavailable entries.
                            None => {
                                let _permit = semaphore
                                    .acquire_owned()
                                    .await
                                    .map_err(|_| CamelError::ChannelClosed)?;
                                ctx.send_and_wait(exchange).await
                            }
                        };
                        (result, Some(ctx))
                    }
                };

                if let Err(ref err) = result
                    && !matches!(err, CamelError::ConsumerStopping)
                {
                    // (category b′: the dispatch invocation returned Err for a
                    // normal-data send — lookup failure, admission failure, or
                    // an in-pipeline error the route handler did NOT absorb —
                    // see ADR-0012 "b-bridged discriminator". This emitter is
                    // the only ERROR signal for the unhandled failure; must
                    // stay loud. ConsumerStopping is a stop-time surrender,
                    // not an operator-visible failure, and does not emit.
                    // Attribution: entry-present failures record under the
                    // consumer entry's route id; the no-entry case has no
                    // entry context and records under the endpoint-derived id
                    // `direct:<name>`, distinguishing the component signal
                    // from the producing route's traced wrapper.)
                    let attribution = match entry_ctx.as_ref() {
                        Some(ctx) => ctx.route_id().to_string(),
                        None => format!("direct:{name}"),
                    };
                    runtime
                        .metrics()
                        .increment_errors(&attribution, "b-prime:direct:send-and-wait");
                    // log-policy: outside-contract
                    error!(
                        endpoint_name = %name,
                        error = %err,
                        "direct consumer pipeline error"
                    );
                }

                debug!(endpoint_name = %name, "direct message sent");
                result
            })
            .await;

            match timed {
                Ok(result) => result,
                Err(_) => {
                    // Timeout branch: tokio dropped the inner future on
                    // expiry, so the emission site above never ran — an
                    // expired dispatch emitted nothing pre-fix. Emit here
                    // through the same context-threaded handle (the freshly
                    // constructed timeout error is never ConsumerStopping).
                    let err = dispatch_timeout_error(&name);
                    runtime.metrics().increment_errors(
                        &format!("direct:{name}"),
                        "b-prime:direct:send-and-wait",
                    );
                    // log-policy: outside-contract
                    error!(
                        endpoint_name = %name,
                        error = %err,
                        "direct dispatch timed out"
                    );
                    Err(err)
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "direct_tests.rs"]
mod tests;
