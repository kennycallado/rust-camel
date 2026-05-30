use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::{Layer, Service};

use crate::oauth2::TokenProvider;
use camel_api::{CamelError, Exchange};

pub struct BearerTokenLayer {
    provider: Arc<dyn TokenProvider>,
}

impl BearerTokenLayer {
    pub fn new(provider: Arc<dyn TokenProvider>) -> Self {
        Self { provider }
    }
}

impl<S> Layer<S> for BearerTokenLayer {
    type Service = BearerTokenService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        BearerTokenService {
            inner,
            provider: Arc::clone(&self.provider),
        }
    }
}

pub struct BearerTokenService<S> {
    inner: S,
    provider: Arc<dyn TokenProvider>,
}

impl<S: Clone> Clone for BearerTokenService<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            provider: Arc::clone(&self.provider),
        }
    }
}

impl<S> Service<Exchange> for BearerTokenService<S>
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    S::Future: Send,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let provider = Arc::clone(&self.provider);
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let token = provider.get_token().await.map_err(CamelError::from)?;
            exchange
                .input
                .set_header("Authorization", format!("Bearer {token}")); // allow-secret
            inner.call(exchange).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AuthError;
    use async_trait::async_trait;
    use camel_api::{BoxProcessor, BoxProcessorExt, Message};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    #[derive(Debug)]
    struct StaticTokenProvider {
        token: String,
    }

    #[async_trait]
    impl TokenProvider for StaticTokenProvider {
        async fn get_token(&self) -> Result<String, AuthError> {
            Ok(self.token.clone())
        }
    }

    #[derive(Debug)]
    struct FailingTokenProvider;

    #[async_trait]
    impl TokenProvider for FailingTokenProvider {
        async fn get_token(&self) -> Result<String, AuthError> {
            Err(AuthError::ProviderUnavailable("token endpoint down".into()))
        }
    }

    fn make_exchange() -> Exchange {
        Exchange::new(Message::new("test"))
    }

    fn ok_processor() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    #[tokio::test]
    async fn test_injects_authorization_header() {
        let provider = Arc::new(StaticTokenProvider {
            token: "my-token".into(),
        });
        let layer = BearerTokenLayer::new(provider);
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        assert!(result.is_ok());
        let ex = result.unwrap();
        let auth = ex.input.header("Authorization").and_then(|v| v.as_str());
        assert_eq!(auth, Some("Bearer my-token"));
    }

    #[tokio::test]
    async fn test_preserves_existing_headers() {
        let provider = Arc::new(StaticTokenProvider { token: "t".into() });
        let layer = BearerTokenLayer::new(provider);
        let mut svc = layer.layer(ok_processor());
        let mut ex = make_exchange();
        ex.input.set_header("X-Custom", "value");
        let result = svc.ready().await.unwrap().call(ex).await;
        let ex = result.unwrap();
        assert_eq!(
            ex.input.header("X-Custom").and_then(|v| v.as_str()),
            Some("value")
        );
        assert_eq!(
            ex.input.header("Authorization").and_then(|v| v.as_str()),
            Some("Bearer t")
        );
    }

    #[tokio::test]
    async fn test_overwrites_existing_auth_header() {
        let provider = Arc::new(StaticTokenProvider {
            token: "fresh".into(),
        });
        let layer = BearerTokenLayer::new(provider);
        let mut svc = layer.layer(ok_processor());
        let mut ex = make_exchange();
        ex.input.set_header("Authorization", "Bearer stale");
        let result = svc.ready().await.unwrap().call(ex).await;
        let ex = result.unwrap();
        assert_eq!(
            ex.input.header("Authorization").and_then(|v| v.as_str()),
            Some("Bearer fresh")
        );
    }

    #[tokio::test]
    async fn test_provider_error_propagates() {
        let provider = Arc::new(FailingTokenProvider);
        let layer = BearerTokenLayer::new(provider);
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            CamelError::ProcessorError(msg) => {
                assert!(msg.contains("token endpoint down"));
            }
            other => panic!("expected ProcessorError, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_calls_provider_each_time() {
        let count = Arc::new(AtomicUsize::new(0));
        #[derive(Debug)]
        struct CountingProvider {
            count: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl TokenProvider for CountingProvider {
            async fn get_token(&self) -> Result<String, AuthError> {
                let n = self.count.fetch_add(1, Ordering::SeqCst);
                Ok(format!("token-{n}")) // allow-secret
            }
        }
        let provider = Arc::new(CountingProvider {
            count: Arc::clone(&count),
        });
        let layer = BearerTokenLayer::new(provider);
        let mut svc = layer.layer(ok_processor());

        for i in 0..3 {
            let result = svc.ready().await.unwrap().call(make_exchange()).await;
            let ex = result.unwrap();
            let auth = ex
                .input
                .header("Authorization")
                .and_then(|v| v.as_str())
                .unwrap();
            assert!(auth.contains(&format!("token-{i}"))); // allow-secret
        }
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_clone_produces_working_service() {
        let provider = Arc::new(StaticTokenProvider { token: "t".into() });
        let layer = BearerTokenLayer::new(provider);
        let mut svc1 = layer.layer(ok_processor());
        let svc2 = svc1.clone();

        let r1 = svc1.ready().await.unwrap().call(make_exchange()).await;
        let mut svc2 = svc2;
        let r2 = svc2.ready().await.unwrap().call(make_exchange()).await;
        assert!(r1.is_ok());
        assert!(r2.is_ok());
    }
}
