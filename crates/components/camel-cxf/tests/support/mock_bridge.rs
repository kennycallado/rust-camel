use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::{Request, Response, Status};

use camel_component_cxf::proto::{
    ConsumerRequest, ConsumerResponse, HealthRequest, HealthResponse, SoapRequest, SoapResponse,
    cxf_bridge_server::{CxfBridge, CxfBridgeServer},
};

/// One scripted outcome for the next `invoke`, consumed in FIFO order.
///
/// `Error` models a gRPC/transport status (`Unavailable` or a
/// transport-class `Internal`). The mock counts the receipt before returning
/// it, so an `Error` outcome can represent a possibly transmitted SOAP send.
/// Only the producer unit test scripts outcomes; other integration test
/// binaries compile this shared support module without using it.
#[allow(dead_code)]
pub enum ScriptedInvokeOutcome {
    /// Respond with a successful `SoapResponse` carrying this payload.
    Success(Vec<u8>),
    /// Fail the call with this gRPC status.
    Error(Status),
}

#[derive(Clone, Default)]
#[allow(clippy::type_complexity)]
pub struct MockState {
    pub last_invoke: Arc<Mutex<Option<SoapRequest>>>,
    pub invoke_fault: Arc<Mutex<Option<(String, String)>>>,
    pub invoke_delay: Arc<Mutex<Option<Duration>>>,
    pub invoke_response_payload: Arc<Mutex<Option<Vec<u8>>>>,
    pub healthy: Arc<Mutex<bool>>,
    /// Counts every `invoke` receipt, incremented before any response is
    /// produced so a transport error still reflects a received send.
    pub invocation_count: Arc<Mutex<u64>>,
    /// FIFO of scripted outcomes; when empty the existing default successful
    /// behavior applies.
    pub scripted_outcomes: Arc<Mutex<VecDeque<ScriptedInvokeOutcome>>>,
    pub consumer_requests_tx: Arc<Mutex<Option<mpsc::Sender<Result<ConsumerRequest, Status>>>>>,
    pub consumer_responses_received: Arc<Mutex<Vec<ConsumerResponse>>>,
    /// Explicit test-side completion boundary. The recorder resolves the
    /// `oneshot` registered for a request id once that request's response
    /// has been written into the gRPC sink. A test can therefore await
    /// request-processing completion instead of inferring it from poll
    /// stability (observe() in the consumer precedes the send, so a stale
    /// count is not a completion proof).
    #[allow(dead_code)]
    pub response_completion: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
    /// Fires on every recorded response. `notify_one` stores a permit, so a
    /// write that races a waiter is never lost; used to drain any late
    /// duplicate response within a bounded window.
    #[allow(dead_code)]
    pub response_recorded: Arc<Notify>,
}

impl MockState {
    /// Register an explicit completion boundary for `request_id` and return
    /// a future that resolves when the recorder writes that request's
    /// response. Call before dispatch: the registry is otherwise
    /// uncontended, so a synchronous `try_lock` cannot fail.
    #[allow(dead_code)]
    pub fn expect_response_completion(&self, request_id: &str) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.response_completion
            .try_lock()
            .expect("response-completion registry must be uncontended before dispatch")
            .insert(request_id.to_string(), tx);
        rx
    }
}

#[derive(Clone)]
pub struct MockCxfBridge {
    state: MockState,
}

impl MockCxfBridge {
    pub fn new(state: MockState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl CxfBridge for MockCxfBridge {
    async fn invoke(
        &self,
        request: Request<SoapRequest>,
    ) -> Result<Response<SoapResponse>, Status> {
        let req = request.into_inner();
        // Count receipt BEFORE responding so a transport error can represent a
        // possibly transmitted SOAP send.
        {
            let mut count = self.state.invocation_count.lock().await;
            *count += 1;
        }
        {
            let mut last = self.state.last_invoke.lock().await;
            *last = Some(req.clone());
        }

        // Scripted outcomes take precedence; each is consumed once in FIFO
        // order. When the queue is empty the existing default behavior runs.
        let scripted = self.state.scripted_outcomes.lock().await.pop_front();
        if let Some(outcome) = scripted {
            return match outcome {
                ScriptedInvokeOutcome::Success(payload) => Ok(Response::new(SoapResponse {
                    payload,
                    headers: Default::default(),
                    fault: false,
                    fault_code: String::new(),
                    fault_string: String::new(),
                })),
                ScriptedInvokeOutcome::Error(status) => Err(status),
            };
        }

        if let Some(delay) = *self.state.invoke_delay.lock().await {
            tokio::time::sleep(delay).await;
        }

        if let Some((fault_code, fault_string)) = self.state.invoke_fault.lock().await.clone() {
            return Ok(Response::new(SoapResponse {
                payload: Vec::new(),
                headers: Default::default(),
                fault: true,
                fault_code,
                fault_string,
            }));
        }

        let payload = self
            .state
            .invoke_response_payload
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| req.payload.clone());

        Ok(Response::new(SoapResponse {
            payload,
            headers: Default::default(),
            fault: false,
            fault_code: String::new(),
            fault_string: String::new(),
        }))
    }

    type OpenConsumerStreamStream = ReceiverStream<Result<ConsumerRequest, Status>>;

    async fn open_consumer_stream(
        &self,
        request: Request<tonic::Streaming<ConsumerResponse>>,
    ) -> Result<Response<Self::OpenConsumerStreamStream>, Status> {
        let mut inbound = request.into_inner();
        let state = self.state.clone();

        tokio::spawn(async move {
            while let Some(next) = inbound.next().await {
                match next {
                    Ok(resp) => {
                        let request_id = resp.request_id.clone();
                        state.consumer_responses_received.lock().await.push(resp);
                        // Explicit completion boundary for this request id,
                        // then a permit-storing wakeup for the drain.
                        if let Some(tx) = state.response_completion.lock().await.remove(&request_id)
                        {
                            let _ = tx.send(());
                        }
                        state.response_recorded.notify_one();
                    }
                    Err(_) => break,
                }
            }
        });

        let (tx, rx) = mpsc::channel::<Result<ConsumerRequest, Status>>(32);
        {
            let mut guard = self.state.consumer_requests_tx.lock().await;
            *guard = Some(tx);
        }

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let healthy = *self.state.healthy.lock().await;
        Ok(Response::new(HealthResponse {
            healthy,
            message: if healthy {
                "ok".to_string()
            } else {
                "unhealthy".to_string()
            },
        }))
    }
}

pub async fn spawn_mock_bridge()
-> Result<(u16, MockState), Box<dyn std::error::Error + Send + Sync>> {
    let state = MockState {
        healthy: Arc::new(Mutex::new(true)),
        ..Default::default()
    };

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let incoming = TcpListenerStream::new(listener);

    let server = MockCxfBridge::new(state.clone());
    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(CxfBridgeServer::new(server))
            .serve_with_incoming(incoming)
            .await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok((port, state))
}
