use camel_api::{CamelError, Exchange, IdentityProcessor};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::Service;
use tracing::{debug, error, info, trace, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone)]
pub struct LogProcessor {
    inner: IdentityProcessor,
    level: LogLevel,
    message: String,
}

impl LogProcessor {
    pub fn new(level: LogLevel, message: String) -> Self {
        Self {
            inner: IdentityProcessor,
            level,
            message,
        }
    }
}

impl Service<Exchange> for LogProcessor {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let msg = self.message.clone();
        match self.level {
            LogLevel::Trace => trace!("{}", msg),
            LogLevel::Debug => debug!("{}", msg),
            LogLevel::Info => info!("{}", msg),
            LogLevel::Warn => warn!("{}", msg),
            LogLevel::Error => error!("{}", msg),
        }
        self.inner.call(exchange)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::body::Body;

    #[tokio::test]
    async fn test_log_processor_passes_exchange_through() {
        let mut processor = LogProcessor::new(LogLevel::Info, "test message".into());
        let exchange = Exchange::default();
        let result = processor.call(exchange).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_log_processor_preserves_exchange_body() {
        let mut processor = LogProcessor::new(LogLevel::Debug, "debug message".into());
        let mut exchange = Exchange::default();
        exchange.input.body = Body::Text("test body".into());
        let result = processor.call(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("test body"));
    }
}
