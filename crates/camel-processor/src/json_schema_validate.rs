use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::body::Body;
use camel_api::{CamelError, Exchange};

/// Tower Service that validates the exchange body against a compiled JSON
/// Schema. Returns `CamelError::ValidationError` if the body does not match.
/// Validation runs BEFORE the inner service is called.
///
/// Non-JSON bodies (Body::Text, Body::Bytes, Body::Xml, Body::Empty,
/// Body::Stream) are passed through without validation — the validator only
/// runs against Body::Json. Callers that want to validate raw text/bytes
/// must first run an Unmarshal step that produces Body::Json.
pub struct JsonSchemaValidateService<P> {
    inner: P,
    validator: jsonschema::Validator,
}

impl<P> std::fmt::Debug for JsonSchemaValidateService<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonSchemaValidateService")
            .finish_non_exhaustive()
    }
}

impl<P> Clone for JsonSchemaValidateService<P>
where
    P: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            validator: self.validator.clone(),
        }
    }
}

impl<P> JsonSchemaValidateService<P> {
    /// Compile a JSON Schema and wrap the given inner service. Returns
    /// `CamelError::RouteError` if the schema is not a valid JSON Schema.
    pub fn new(inner: P, schema: &serde_json::Value) -> Result<Self, CamelError> {
        let validator = jsonschema::validator_for(schema)
            .map_err(|e| CamelError::RouteError(format!("invalid JSON schema: {e}")))?;
        Ok(Self { inner, validator })
    }
}

impl<P> Service<Exchange> for JsonSchemaValidateService<P>
where
    P: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    P::Future: Send,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        // Only Body::Json is validated. Anything else passes through.
        let result = match &exchange.input.body {
            Body::Json(v) => {
                let errors: Vec<String> = self
                    .validator
                    .iter_errors(v)
                    .map(|e| format!("{e} at {}", e.instance_path()))
                    .collect();
                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors.join("; "))
                }
            }
            _ => Ok(()),
        };
        match result {
            Ok(()) => {
                let fut = self.inner.call(exchange);
                Box::pin(fut)
            }
            Err(msg) => Box::pin(async move {
                Err(CamelError::ValidationError(format!(
                    "JSON schema validation failed: {msg}"
                )))
            }),
        }
    }
}
