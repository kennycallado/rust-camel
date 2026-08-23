//! Source-world guest whose declared `listener_spec.bind` is INDEPENDENT of
//! the operator `bind` config entry (wasm-source-auth-kernel, Task 1.4).
//!
//! The guest NEVER reads the `bind` key: the declared bind derives only
//! from the distinct `conflict_port` config key, so the host-side operator
//! `bind` entry can point at a different address, exercising the
//! bind-conflict refusal in `WasmSourceConsumer::start()`.
//!
//! The run loop mirrors the webhook guest's accept/submit shape, but the
//! exchange is a minimal empty-body InOnly — this fixture exists for host
//! bind governance, not payload handling.

use std::future::Future;

use bindings::camel::plugin::source_host::{
    self, CapabilityRequest, ConcurrencyModel, HttpListener, HttpListenerSpec, HttpRequest,
    SourcePlan, SubmitOutcome,
};
use bindings::camel::plugin::types::{WasmBody, WasmError, WasmExchange, WasmMessage, WasmPattern};

mod bindings {
    wit_bindgen::generate!({
        world: "source",
        // Shares the webhook example's WIT — single source of truth for the
        // source world; this fixture lives under tests/fixtures/.
        path: "../../../../../../examples/wasm-source-webhook/wit",
    });
}

struct ConflictingBindSource;

impl bindings::Guest for ConflictingBindSource {
    fn configure(config: Vec<(String, String)>) -> Result<SourcePlan, WasmError> {
        // Deliberately independent of the `bind` key: the declared bind
        // comes ONLY from `conflict_port` (ephemeral-port allocation by the
        // test harness). The host resolves its operator `bind` entry
        // separately, so the two can be made to conflict.
        let port = config
            .iter()
            .find(|(key, _)| key == "conflict_port")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| "0".to_string());
        let bind = format!("0.0.0.0:{port}");

        Ok(SourcePlan {
            capabilities: vec![CapabilityRequest::HttpListener(HttpListenerSpec {
                bind,
                path: None,
            })],
            concurrency: ConcurrencyModel::Sequential,
        })
    }

    fn run(listener: &HttpListener) -> impl Future<Output = Result<(), WasmError>> {
        async move {
            loop {
                if source_host::is_cancelled() {
                    return Ok(());
                }

                let req = source_host::accept_http(listener).await?;
                let Some(req) = req else {
                    // Cancelled — no more requests
                    return Ok(());
                };

                let exchange = request_to_exchange(req);
                match source_host::submit_exchange(exchange).await? {
                    SubmitOutcome::Accepted => {}
                    SubmitOutcome::Stopped => return Ok(()),
                }
            }
        }
    }
}

/// Minimal exchange: empty body, InOnly.
fn request_to_exchange(req: HttpRequest) -> WasmExchange {
    WasmExchange {
        input: WasmMessage {
            headers: req.headers,
            body: WasmBody::Empty,
        },
        output: None,
        properties: vec![
            ("camel.http.method".to_string(), req.method),
            ("camel.http.path".to_string(), req.path),
        ],
        pattern: WasmPattern::InOnly,
        correlation_id: String::new(),
        route_id: None,
        message_id: None,
    }
}

bindings::export!(ConflictingBindSource with_types_in bindings);
