use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::{Request, Response, Status};

use camel_component_cxf::proto::{
    ConsumerRequest, ConsumerResponse, HealthRequest, HealthResponse, SoapRequest, SoapResponse,
    cxf_bridge_server::{CxfBridge, CxfBridgeServer},
};

#[derive(Clone, Default)]
#[allow(clippy::type_complexity)]
pub struct MockState {
    pub last_invoke: Arc<Mutex<Option<SoapRequest>>>,
    pub invoke_fault: Arc<Mutex<Option<(String, String)>>>,
    pub invoke_delay: Arc<Mutex<Option<Duration>>>,
    pub invoke_response_payload: Arc<Mutex<Option<Vec<u8>>>>,
    pub healthy: Arc<Mutex<bool>>,
    pub consumer_requests_tx: Arc<Mutex<Option<mpsc::Sender<Result<ConsumerRequest, Status>>>>>,
    pub consumer_responses_received: Arc<Mutex<Vec<ConsumerResponse>>>,
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
        {
            let mut last = self.state.last_invoke.lock().await;
            *last = Some(req.clone());
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
                        state.consumer_responses_received.lock().await.push(resp);
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
