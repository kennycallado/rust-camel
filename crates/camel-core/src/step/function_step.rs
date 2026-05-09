use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use camel_api::{
    CamelError, Exchange, ExchangePatch, FunctionDefinition, FunctionId, FunctionInvocationError,
    FunctionInvoker, PatchBody,
};
use tower::Service;

#[derive(Clone)]
pub struct FunctionStep {
    definition: FunctionDefinition,
    invoker: Arc<dyn FunctionInvoker>,
}

impl FunctionStep {
    pub fn new(invoker: Arc<dyn FunctionInvoker>, definition: FunctionDefinition) -> Self {
        Self { definition, invoker }
    }
}

impl Service<Exchange> for FunctionStep {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut ex: Exchange) -> Self::Future {
        let invoker = Arc::clone(&self.invoker);
        let id = self.definition.id.clone();
        let timeout_ms = self.definition.timeout_ms;
        Box::pin(async move {
            let result = tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                invoker.invoke(&id, &ex),
            )
            .await
            .map_err(|_| {
                CamelError::ProcessorError(format!(
                    "function:timeout: {} timed out after {}ms",
                    id.0, timeout_ms
                ))
            })?;
            let patch = result.map_err(|e| map_invocation_error(e, &id))?;
            apply_patch(&mut ex, patch);
            Ok(ex)
        })
    }
}

fn map_invocation_error(err: FunctionInvocationError, id: &FunctionId) -> CamelError {
    match err {
        FunctionInvocationError::UserError { message, .. } => {
            CamelError::ProcessorError(format!("function:user_error: {}: {}", id.0, message))
        }
        FunctionInvocationError::Timeout { timeout_ms, .. } => CamelError::ProcessorError(format!(
            "function:timeout: {} timed out after {}ms",
            id.0, timeout_ms
        )),
        other => CamelError::ProcessorError(format!("function:invoke_error: {}: {other}", id.0)),
    }
}

fn apply_patch(ex: &mut Exchange, patch: ExchangePatch) {
    if let Some(body) = patch.body {
        ex.input.body = match body {
            PatchBody::Text(s) => s.into(),
            PatchBody::Json(v) => v.into(),
            PatchBody::Empty => camel_api::Body::Empty,
        };
    }
    for (k, v) in patch.headers_set {
        ex.input.headers.insert(k, v);
    }
    for k in patch.headers_removed {
        ex.input.headers.remove(&k);
    }
    for (k, v) in patch.properties_set {
        ex.properties.insert(k, v);
    }
}
