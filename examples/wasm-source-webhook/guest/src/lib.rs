use bindings::camel::plugin::types::{WasmBody, WasmError, WasmExchange, WasmMessage, WasmPattern};
use bindings::camel::plugin::source_host::{
    self, HttpListener, HttpRequest, HttpListenerSpec, CapabilityRequest, ConcurrencyModel,
    SourcePlan, SubmitOutcome,
};

mod bindings {
    wit_bindgen::generate!({
        world: "source",
        path: "../wit",
    });
}

struct WebhookSource;

/// Guest-side crash toggle. When `configure()` is called with `crash=run`,
/// `run()` deliberately traps (via `unreachable`) on its first iteration so
/// the host's crash/lifecycle handling can be exercised end-to-end. The value
/// survives between `configure()` and `run()` because the component instance is
/// long-lived for the source world.
static mut CRASH_IN_RUN: bool = false;

impl bindings::Guest for WebhookSource {
    fn configure(config: Vec<(String, String)>) -> Result<SourcePlan, WasmError> {
        let mut bind = String::from("0.0.0.0:8080");
        let mut path: Option<String> = None;

        for (key, value) in &config {
            match key.as_str() {
                "bind" => bind = value.clone(),
                "path" => path = Some(value.clone()),
                "crash" if value == "run" => {
                    // SAFETY: the source component is single-threaded; configure()
                    // and run() never execute concurrently.
                    unsafe { CRASH_IN_RUN = true };
                }
                _ => {}
            }
        }

        Ok(SourcePlan {
            capabilities: vec![CapabilityRequest::HttpListener(HttpListenerSpec {
                bind,
                path,
            })],
            concurrency: ConcurrencyModel::Sequential,
        })
    }

    fn run(listener: &HttpListener) -> Result<(), WasmError> {
        // SAFETY: single-threaded component; see CRASH_IN_RUN docs.
        if unsafe { CRASH_IN_RUN } {
            // Intentional trap to exercise host crash handling (Q3 of the spike).
            unreachable!("crash-guest: deliberate trap in run()");
        }
        loop {
            if source_host::is_cancelled() {
                return Ok(());
            }

            let req = source_host::accept_http(listener)?;
            let Some(req) = req else {
                // Cancelled — no more requests
                return Ok(());
            };

            let exchange = http_request_to_exchange(&req);

            match source_host::submit_exchange(&exchange)? {
                SubmitOutcome::Accepted => {}
                SubmitOutcome::Stopped => return Ok(()),
            }
        }
    }
}

fn http_request_to_exchange(req: &HttpRequest) -> WasmExchange {
    let body = if req.body.is_empty() {
        WasmBody::Empty
    } else {
        WasmBody::Bytes(req.body.clone())
    };

    WasmExchange {
        input: WasmMessage {
            headers: req.headers.clone(),
            body,
        },
        output: None,
        properties: vec![
            ("camel.http.method".to_string(), req.method.clone()),
            ("camel.http.path".to_string(), req.path.clone()),
        ],
        pattern: WasmPattern::InOnly,
        correlation_id: String::new(),
        route_id: None,
        message_id: None,
    }
}

bindings::export!(WebhookSource with_types_in bindings);
