//! Retry helpers for gRPC RPC calls — classification and execution loop.

use std::future::Future;

use camel_api::CamelError;
use camel_component_api::{NetworkRetryPolicy, retry_async};
use tonic::transport::Channel;
use tonic::{Code, Status};
use tracing::error;

/// Classifies a tonic::Status code as transient (retryable) or permanent.
///
/// Transient: `Unavailable`, `DeadlineExceeded`, `ResourceExhausted`, `Aborted`
/// (all server-side issues that may self-heal).
///
/// Permanent: `InvalidArgument`, `NotFound`, `PermissionDenied`, `Unauthenticated`,
/// `AlreadyExists`, `FailedPrecondition`, `OutOfRange`, `Unimplemented`, `Internal`,
/// `DataLoss`, `Unknown`, `Cancelled`.
///
/// `Cancelled` is treated as permanent because the caller may have intentionally
/// cancelled the call (e.g., deadline expiry).
pub fn is_retryable_tonic_status(status: &tonic::Status) -> bool {
    matches!(
        status.code(),
        Code::Unavailable | Code::DeadlineExceeded | Code::ResourceExhausted | Code::Aborted
    )
}

/// Map a tonic [`Status`] to a [`CamelError`].
///
/// `Unavailable` and `DeadlineExceeded` are tagged `[TRANSIENT]` so the
/// upstream master/retry loops can distinguish them from permanent errors.
pub(crate) fn tonic_to_camel_error(status: Status) -> CamelError {
    let code = status.code();
    let msg = status.message();
    match code {
        Code::NotFound => CamelError::ProcessorError(format!("grpc[NOT_FOUND]: {msg}")),
        Code::Unavailable => {
            CamelError::ProcessorError(format!("grpc[TRANSIENT][UNAVAILABLE]: {msg}"))
        }
        Code::DeadlineExceeded => {
            CamelError::ProcessorError(format!("grpc[TRANSIENT][DEADLINE_EXCEEDED]: {msg}"))
        }
        Code::InvalidArgument => CamelError::Config(format!("grpc invalid argument: {msg}")),
        other => CamelError::ProcessorError(format!("grpc[{other:?}]: {msg}")),
    }
}

// ── Shared retry helper ───────────────────────────────────────────────────

/// Retry loop for gRPC RPC calls with exponential backoff, built on the generic
/// [`retry_async`].
///
/// The closure receives a fresh [`tonic::client::Grpc`] handle for each attempt
/// (so it can be `FnMut` without borrow-checker issues). It must call
/// `.ready()` and then the actual RPC method, returning `Ok(response)` or
/// `Err(tonic::Status)`.
pub(crate) async fn retry_rpc<T, F, Fut>(
    channel: Channel,
    retry: &NetworkRetryPolicy,
    kind: &str,
    mut rpc_call: F,
) -> Result<T, CamelError>
where
    F: FnMut(tonic::client::Grpc<Channel>) -> Fut,
    Fut: Future<Output = Result<T, tonic::Status>>,
{
    let result = retry_async::<T, _, _, _, tonic::Status>(
        retry,
        || {
            let grpc = tonic::client::Grpc::new(channel.clone());
            rpc_call(grpc)
        },
        is_retryable_tonic_status,
    )
    .await;
    match result {
        Ok(response) => Ok(response),
        Err(status) => {
            error!(
                code = %status.code(),
                "grpc {} call failed (retries exhausted or non-retryable)",
                kind
            );
            Err(tonic_to_camel_error(status))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use super::{is_retryable_tonic_status, retry_rpc, tonic_to_camel_error};
    use camel_api::CamelError;
    use camel_component_api::NetworkRetryPolicy;

    // ── is_retryable_tonic_status tests ────────────────────────────────────

    #[test]
    fn is_retryable_tonic_status_transient_codes() {
        // Transient: server-side issues that may self-heal
        assert!(is_retryable_tonic_status(&tonic::Status::unavailable(
            "down"
        )));
        assert!(is_retryable_tonic_status(
            &tonic::Status::deadline_exceeded("timeout")
        ));
        assert!(is_retryable_tonic_status(
            &tonic::Status::resource_exhausted("rate limit")
        ));
        assert!(is_retryable_tonic_status(&tonic::Status::aborted(
            "conflict"
        )));
    }

    #[test]
    fn is_retryable_tonic_status_permanent_codes() {
        // Permanent: client-side errors, logical errors, or unsafe to retry
        assert!(!is_retryable_tonic_status(
            &tonic::Status::invalid_argument("bad")
        ));
        assert!(!is_retryable_tonic_status(&tonic::Status::not_found(
            "missing"
        )));
        assert!(!is_retryable_tonic_status(
            &tonic::Status::permission_denied("no")
        ));
        assert!(!is_retryable_tonic_status(&tonic::Status::unauthenticated(
            "who"
        )));
        assert!(!is_retryable_tonic_status(&tonic::Status::already_exists(
            "dup"
        )));
        assert!(!is_retryable_tonic_status(
            &tonic::Status::failed_precondition("state")
        ));
        assert!(!is_retryable_tonic_status(&tonic::Status::out_of_range(
            "oob"
        )));
        assert!(!is_retryable_tonic_status(&tonic::Status::unimplemented(
            "noimpl"
        )));
        assert!(!is_retryable_tonic_status(&tonic::Status::internal("oops")));
        assert!(!is_retryable_tonic_status(&tonic::Status::data_loss(
            "lost"
        )));
        assert!(!is_retryable_tonic_status(&tonic::Status::unknown("?")));
        // Cancelled is permanent (caller may have intentionally cancelled)
        assert!(!is_retryable_tonic_status(&tonic::Status::cancelled(
            "stopped"
        )));
    }

    // ── retry attempt-counting regression test ─────────────────────────

    /// Validates that the canonical attempt-counting pattern is used in
    /// `retry_rpc`: with `max_attempts=3`, the RPC closure is invoked
    /// exactly 3 times before giving up (not 4).
    #[tokio::test]
    async fn retry_attempts_count_matches_policy_max_attempts() {
        let retry = NetworkRetryPolicy {
            enabled: true,
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };
        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = Arc::clone(&calls);

        // Use a valid but unreachable endpoint so Grpc::new() succeeds
        // but the closure never actually uses the channel.
        let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy();

        let result = retry_rpc::<(), _, _>(channel, &retry, "test", |_grpc| {
            let c = Arc::clone(&calls_clone);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(tonic::Status::unavailable("simulated transient failure"))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    /// Validates that retry_rpc returns CamelError::ProcessorError (via
    /// tonic_to_camel_error) when retries are exhausted on a transient
    /// (UNAVAILABLE) status — operators see a clear error, not a silent
    /// drop. Also serves as a functional complement to the
    /// retry_attempts_count_matches_policy_max_attempts test.
    #[tokio::test]
    async fn retry_rpc_returns_processor_error_on_exhausted_transient() {
        let retry = NetworkRetryPolicy {
            enabled: true,
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..NetworkRetryPolicy::default()
        };

        let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy();

        let result = retry_rpc::<(), _, _>(channel, &retry, "test", |_grpc| async move {
            Err(tonic::Status::unavailable("simulated transient failure"))
        })
        .await;

        let err = result.unwrap_err();
        assert!(
            matches!(err, CamelError::ProcessorError(_)),
            "expected ProcessorError, got {:?}",
            err
        );
        assert!(
            err.to_string().contains("grpc[TRANSIENT][UNAVAILABLE]"),
            "expected error to contain transient marker, got: {}",
            err
        );
    }

    // ── tonic_to_camel_error tests ─────────────────────────────────────

    #[test]
    fn test_tonic_to_camel_error_unavailable() {
        let status = tonic::Status::unavailable("service down");
        let err = tonic_to_camel_error(status);
        assert!(matches!(err, CamelError::ProcessorError(_)));
        assert!(err.to_string().contains("grpc[TRANSIENT][UNAVAILABLE]"));
        assert!(err.to_string().contains("service down"));
    }

    #[test]
    fn test_tonic_to_camel_error_not_found() {
        let status = tonic::Status::not_found("method not found");
        let err = tonic_to_camel_error(status);
        assert!(matches!(err, CamelError::ProcessorError(_)));
        assert!(err.to_string().contains("grpc[NOT_FOUND]"));
        assert!(err.to_string().contains("method not found"));
    }

    #[test]
    fn test_tonic_to_camel_error_deadline_exceeded() {
        let status = tonic::Status::deadline_exceeded("timeout");
        let err = tonic_to_camel_error(status);
        assert!(matches!(err, CamelError::ProcessorError(_)));
        assert!(
            err.to_string()
                .contains("grpc[TRANSIENT][DEADLINE_EXCEEDED]")
        );
    }

    #[test]
    fn test_tonic_to_camel_error_invalid_argument_maps_to_config() {
        let status = tonic::Status::invalid_argument("bad arg");
        let err = tonic_to_camel_error(status);
        assert!(matches!(err, CamelError::Config(_)));
        assert!(err.to_string().contains("grpc invalid argument"));
    }

    #[test]
    fn test_tonic_to_camel_error_various_codes() {
        let codes = [
            tonic::Status::permission_denied("no access"),
            tonic::Status::resource_exhausted("too many"),
            tonic::Status::failed_precondition("bad state"),
            tonic::Status::aborted("conflict"),
            tonic::Status::out_of_range("oob"),
            tonic::Status::unimplemented("no impl"),
            tonic::Status::internal("oops"),
            tonic::Status::data_loss("lost"),
            tonic::Status::unauthenticated("who are you"),
        ];
        for status in codes {
            let err = tonic_to_camel_error(status);
            assert!(matches!(err, CamelError::ProcessorError(_)));
            assert!(err.to_string().contains("grpc["));
        }
    }
}
