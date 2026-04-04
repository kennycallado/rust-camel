use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use camel_api::{Body, CamelError, Exchange, Value};
use tonic::transport::Channel;
use tower::Service;
use tracing::debug;

use crate::config::JmsEndpointConfig;
use crate::headers::extract_send_headers;
use crate::proto::{SendRequest, bridge_service_client::BridgeServiceClient};

#[derive(Clone)]
pub struct JmsProducer {
    channel: Channel,
    endpoint_config: JmsEndpointConfig,
}

impl JmsProducer {
    pub fn new(channel: Channel, endpoint_config: JmsEndpointConfig) -> Self {
        Self {
            channel,
            endpoint_config,
        }
    }

    fn body_to_bytes(body: &Body) -> Result<Vec<u8>, CamelError> {
        match body {
            Body::Text(s) => Ok(s.as_bytes().to_vec()),
            Body::Xml(s) => Ok(s.as_bytes().to_vec()),
            Body::Bytes(b) => Ok(b.to_vec()),
            Body::Json(v) => serde_json::to_vec(v)
                .map_err(|e| CamelError::ProcessorError(format!("JSON error: {e}"))),
            Body::Empty => Ok(vec![]),
            Body::Stream(_) => Err(CamelError::ProcessorError(
                "Body::Stream must be materialized before sending to JMS".to_string(),
            )),
        }
    }

    fn content_type(exchange: &Exchange) -> String {
        // Explicit header always wins
        if let Some(ct) = exchange
            .input
            .header("Content-Type")
            .and_then(|v| v.as_str().map(str::to_string))
        {
            return ct;
        }
        // Infer from body type so Java creates the right message type
        match &exchange.input.body {
            Body::Text(_) => "text/plain".to_string(),
            Body::Xml(_) => "text/xml".to_string(),
            Body::Json(_) => "application/json".to_string(),
            _ => String::new(),
        }
    }
}

impl Service<Exchange> for JmsProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let channel = self.channel.clone();
        let destination = self.endpoint_config.destination();

        Box::pin(async move {
            let body = Self::body_to_bytes(&exchange.input.body)?;
            let headers = extract_send_headers(&exchange);
            let content_type = Self::content_type(&exchange);

            let mut client = BridgeServiceClient::new(channel);
            let request = SendRequest {
                destination,
                body,
                headers,
                content_type,
            };

            let response = client
                .send(request)
                .await
                .map_err(|s| CamelError::ProcessorError(format!("JMS gRPC send error: {s}")))?
                .into_inner();

            debug!(message_id = %response.message_id, "JMS message sent");
            exchange
                .input
                .set_header("JMSMessageID", Value::String(response.message_id));
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::StreamBody;
    use futures::stream;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn body_text_to_bytes() {
        let b = JmsProducer::body_to_bytes(&Body::Text("hello".to_string())).unwrap();
        assert_eq!(b, b"hello");
    }

    #[test]
    fn body_empty_to_bytes() {
        let b = JmsProducer::body_to_bytes(&Body::Empty).unwrap();
        assert!(b.is_empty());
    }

    #[test]
    fn body_stream_returns_error() {
        let stream = stream::empty::<Result<bytes::Bytes, camel_api::CamelError>>();
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: Default::default(),
        });
        let err = JmsProducer::body_to_bytes(&body).unwrap_err();
        assert!(err.to_string().contains("Stream"));
    }

    #[test]
    fn content_type_inferred_from_text_body() {
        // When body is Body::Text and no Content-Type header is set,
        // the content type should be inferred as "text/plain" so Java
        // creates a TextMessage instead of a BytesMessage.
        let mut ex = Exchange::default();
        ex.input.body = Body::Text("hello".to_string());
        assert_eq!(JmsProducer::content_type(&ex), "text/plain");
    }

    #[test]
    fn content_type_header_overrides_inferred() {
        // Explicit Content-Type header should always take precedence
        let mut ex = Exchange::default();
        ex.input.body = Body::Text("hello".to_string());
        ex.input.set_header("Content-Type", Value::String("text/xml".to_string()));
        assert_eq!(JmsProducer::content_type(&ex), "text/xml");
    }

    #[test]
    fn content_type_inferred_from_xml_body() {
        let mut ex = Exchange::default();
        ex.input.body = Body::Xml("<root/>".to_string());
        assert_eq!(JmsProducer::content_type(&ex), "text/xml");
    }

    #[test]
    fn content_type_inferred_from_json_body() {
        let mut ex = Exchange::default();
        ex.input.body = Body::Json(serde_json::json!({"key": "val"}));
        assert_eq!(JmsProducer::content_type(&ex), "application/json");
    }

    #[test]
    fn content_type_empty_for_bytes_without_header() {
        // Body::Bytes without a header → no inference, stays empty
        let mut ex = Exchange::default();
        ex.input.body = Body::from(b"raw".to_vec());
        assert_eq!(JmsProducer::content_type(&ex), "");
    }
}
