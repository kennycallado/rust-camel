//! Structural classification and boundary helpers for Redis transport
//! errors.
//!
//! # Identity contract
//!
//! The legacy classifier OR-matched "classifier words" over the whole
//! `CamelError` Display string. That Display is assembled from three
//! sources:
//!
//! - (a) the variant prefix (`IO error:` → `io error`),
//! - (b) the static prose of the wrap site (`Redis HSET failed: `, …),
//! - (c) the inner `redis::RedisError` Display (general description +
//!   `Kind` Debug, inner io/TLS text, or the server-controlled message).
//!
//! The structural classifier preserves the verdict PER SITE, not per byte:
//!
//! - a wrap site whose static prose contains a classifier word was always
//!   transient regardless of the inner kind — such sites attach a typed
//!   marker (rule 3) instead of relying on their prose;
//! - a site whose prose contains no classifier word had a legacy verdict
//!   equal to "does the inner redis Display match a word" — rule 5's
//!   fallback over the inner `RedisError` Display reproduces that exactly;
//! - plain `ProcessorError` text is never sniffed (rule 6): synthetic and
//!   foreign errors whose message happens to contain a word no longer
//!   classify transient — an intentional flip that only affects inputs
//!   the component's real conversion boundaries cannot produce.
//!
//! Every wrap site in the crate is tracked in the per-site prose audit
//! table in `openspec/changes/rediserr/design.md`; each boundary task must
//! prove `structured verdict == legacy verdict` for its rows or list a
//! documented flip.
//!
//! # Classification rules (in precedence order)
//!
//! 1. `CamelError::Config(_)` / `ConfigValidation(_)` → false
//!    (ADR-0012 error-family early return, rc-ezi0f: a Config error is a
//!    setup defect, never a transport hiccup).
//! 2. `CamelError::Io(_)` → true (legacy source (a): the `IO error:`
//!    Display prefix always matched `io error`).
//! 3. The source chain contains one of the typed markers
//!    ([`TransientRetryBudgetExhausted`], [`TransportTimeout`],
//!    [`TransientByProse`]) → true (legacy source (b): the site's static
//!    prose always matched).
//! 4. The first `redis::RedisError` in the chain has a kind that renders a
//!    classifier word today → true: `Server(ReadOnly)`,
//!    `ClusterConnectionNotFound` (its Debug rendering contains
//!    "connection"), or `Io` whose source is a `std::io::Error` of kind
//!    `ConnectionRefused`, `ConnectionReset`, `ConnectionAborted`,
//!    `BrokenPipe`, or `TimedOut`. Documented flip 1 (accepted,
//!    false→true, design.md): an enumerated transient io kind with custom
//!    text lacking classifier words (e.g.
//!    `io::Error::new(ConnectionRefused, "no route")`) — legacy sniffed
//!    the text and missed it; the kind is ground truth for an OS
//!    connection refusal. Pinned by `io_custom_text_refused_kind_is_transient`.
//! 5. Otherwise that same `RedisError`'s Display is matched against the
//!    legacy word set in [`legacy_substring_matches`] — the only substring
//!    matching in the crate's classification. It preserves
//!    server-controlled messages (`ERR connection lost …`), TLS inner
//!    text, and redis-rs static details.
//! 6. No marker and no `RedisError` in the chain → false.
//!
//! # redis 1.6.0 kind taxonomy
//!
//! `redis::ErrorKind` distinguishes transport failures (`Io`, holding the
//! inner `std::io::Error` in its source), server replies
//! (`Server(redis::ServerErrorKind)`, e.g. `ReadOnly` for a write against
//! a replica), client-side connection bookkeeping
//! (`ClusterConnectionNotFound`), and authentication
//! (`AuthenticationFailed`). The structural rules read the kind and the
//! inner `std::io::Error` directly instead of parsing rendered text, so
//! verdicts survive message rewording inside redis-rs.

use camel_component_api::CamelError;
use std::error::Error as _;
use std::sync::Arc;

/// Bound on how deep [`is_transient_redis_error`] walks the `source()`
/// chain before giving up (guards against pathological/cyclic chains).
const MAX_SOURCE_HOPS: usize = 8;

// ── Typed transient markers ─────────────────────────────────────────────────

/// Terminal-error marker: the bounded transient-retry loop exhausted its
/// budget (ADR-0012).
///
/// Attached by `retry.rs::retry_budget_exhausted`. Classification is
/// structural: the marker in the source chain makes the error transient,
/// so the consumer's Err-branch fires the `e:redis:message-transient-budget`
/// metric and Route supervision restarts the route (ADR-0007).
#[derive(Debug)]
pub(crate) struct TransientRetryBudgetExhausted {
    /// Stage name of the failed reconnect loop (e.g. "connecting for
    /// PubSub").
    pub stage: String,
    /// The retry policy's max attempts, mirrored for diagnostics.
    pub attempts: u32,
}

impl std::fmt::Display for TransientRetryBudgetExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "transient retry budget exhausted at stage {} after {} attempts",
            self.stage, self.attempts
        )
    }
}

impl std::error::Error for TransientRetryBudgetExhausted {}

/// Marker: a tokio timeout elapsed at a transport stage whose legacy prose
/// contained classifier words (e.g. the executor, queue, and pubsub
/// connect-timeout wraps).
#[derive(Debug)]
pub(crate) struct TransportTimeout {
    /// Timeout-wrapped stage (e.g. "connect").
    pub stage: &'static str,
}

impl std::fmt::Display for TransportTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "transport timeout at stage {}", self.stage)
    }
}

impl std::error::Error for TransportTimeout {}

/// Marker: a wrap site whose static prose contained a classifier word, so
/// its legacy verdict was always-transient regardless of the inner kind.
///
/// The site name records WHERE the always-transient decision was audited.
#[derive(Debug)]
pub(crate) struct TransientByProse {
    /// Audit-table site identifier of the always-transient wrap.
    pub site: &'static str,
}

impl std::fmt::Display for TransientByProse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "transient by prose audit at site {}", self.site)
    }
}

impl std::error::Error for TransientByProse {}

// ── Conversion boundaries ───────────────────────────────────────────────────

/// Wraps a `redis::RedisError` with the standard op prose, preserving the
/// error in the source chain for structural classification.
///
/// Message text is byte-identical to the legacy
/// `ProcessorError(format!("Redis {op} failed: {err}"))` wrap.
pub(crate) fn redis_error_to_camel(op: &str, err: redis::RedisError) -> CamelError {
    CamelError::ProcessorErrorWithSource(format!("Redis {op} failed: {err}"), Arc::new(err))
}

/// Wraps a `redis::RedisError` with its bare Display, for passthrough
/// sites whose legacy text was `{e}` (the queue blpop error path).
pub(crate) fn redis_error_raw(err: redis::RedisError) -> CamelError {
    CamelError::ProcessorErrorWithSource(err.to_string(), Arc::new(err))
}

/// Attaches a typed transient marker to an error whose text is already
/// final (byte-identical to its legacy rendering).
pub(crate) fn marker_camel(
    text: String,
    marker: impl std::error::Error + Send + Sync + 'static,
) -> CamelError {
    CamelError::ProcessorErrorWithSource(text, Arc::new(marker))
}

// ── Classification ──────────────────────────────────────────────────────────

/// The legacy classifier word set — the rule-5 fallback and the ONLY
/// substring matching in the crate's classification.
///
/// Kept verbatim (lowercased contains) so rule 5 reproduces the legacy
/// verdict for `RedisError` Displays that match no structural shape:
/// server-controlled messages, TLS inner text, and redis-rs static
/// details.
pub(crate) fn legacy_substring_matches(msg: &str) -> bool {
    let msg = msg.to_lowercase();
    msg.contains("connection")
        || msg.contains("io error")
        || msg.contains("timed out")
        || msg.contains("broken pipe")
        || msg.contains("connection reset")
        || msg.contains("eof")
        || msg.contains("refused")
        || msg.contains("readonly")
        || msg.contains("read only")
}

/// Unwraps the `Arc<dyn Error + Send + Sync>` wrapper hop this toolchain's
/// std inserts into source chains.
///
/// std (rustc 1.98+) implements `Error for Arc<T: Error + ?Sized>`
/// (alloc/src/sync.rs), and thiserror 2 resolves `#[source]
/// Arc<dyn Error + Send + Sync>` fields through it, so a `source()` hop
/// built from such an Arc surfaces as a wrapper object that delegates
/// Display/Debug/source but fails `downcast_ref::<T>()` for the wrapped
/// `T`. Both `CamelError::ProcessorErrorWithSource` and redis-rs's
/// internal error repr store their sources as exactly this Arc shape, so
/// the classification walk unwraps the wrapper before probing. The
/// wrapper's own `source()` delegates to the pointee's `source()`, so
/// unwrapping never skips a chain node.
fn unwrap_arc_dyn_error<'a>(
    src: &'a (dyn std::error::Error + 'static),
) -> &'a (dyn std::error::Error + 'static) {
    match src.downcast_ref::<Arc<dyn std::error::Error + Send + Sync>>() {
        Some(arc) => &**arc,
        None => src,
    }
}

/// Returns true if the error is a transient transport failure that may be
/// resolved by reconnecting.
///
/// Structural classification per the module docs (rules 1–6). Business
/// errors (WRONGTYPE, NOSCRIPT, auth failures) and config errors are not
/// transient; plain `ProcessorError` text is never sniffed.
pub fn is_transient_redis_error(err: &CamelError) -> bool {
    // Rule 1: config-family errors are setup defects, never transport
    // hiccups (rc-ezi0f) — early-return before any structure is inspected,
    // so transient-looking substrings inside Config messages (e.g. a CA
    // file path containing "readonly") cannot flip the verdict.
    if matches!(err, CamelError::Config(_) | CamelError::ConfigValidation(_)) {
        return false;
    }
    // Rule 2: the Io variant's Display prefix (`IO error:`) always matched
    // the legacy `io error` word.
    if matches!(err, CamelError::Io(_)) {
        return true;
    }
    // Rules 3–6: bounded walk of the source chain. At each hop, markers
    // win (rule 3) before the first `redis::RedisError` terminates the
    // walk with its own classification (rules 4–5).
    let mut source = err.source();
    let mut hops = 0;
    while let Some(raw) = source {
        if hops >= MAX_SOURCE_HOPS {
            break;
        }
        let src = unwrap_arc_dyn_error(raw);
        if src
            .downcast_ref::<TransientRetryBudgetExhausted>()
            .is_some()
            || src.downcast_ref::<TransportTimeout>().is_some()
            || src.downcast_ref::<TransientByProse>().is_some()
        {
            return true;
        }
        if let Some(redis_err) = src.downcast_ref::<redis::RedisError>() {
            return classify_redis_error(redis_err);
        }
        source = src.source();
        hops += 1;
    }
    // Rule 6: no marker, no redis error, not Io — plain ProcessorError
    // text is never sniffed.
    false
}

/// Rules 4–5 applied to the first `redis::RedisError` in a source chain.
fn classify_redis_error(err: &redis::RedisError) -> bool {
    match err.kind() {
        redis::ErrorKind::Server(redis::ServerErrorKind::ReadOnly)
        | redis::ErrorKind::ClusterConnectionNotFound => true,
        redis::ErrorKind::Io => {
            let enumerated_io_kind = err
                .source()
                .map(unwrap_arc_dyn_error)
                .and_then(|src| src.downcast_ref::<std::io::Error>())
                .is_some_and(|io_err| {
                    matches!(
                        io_err.kind(),
                        std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::TimedOut
                    )
                });
            enumerated_io_kind || legacy_substring_matches(&err.to_string())
        }
        _ => legacy_substring_matches(&err.to_string()),
    }
}

// ── Shared test fixtures ────────────────────────────────────────────────────
//
// Canonical transient fixtures reused by this crate's test modules
// (retry, queue, pubsub) so every suite classifies the same shapes the
// production boundaries can produce.

/// `redis::RedisError` wrapping an OS connection-refused io error
/// (rule-4 transient).
#[cfg(test)]
pub(crate) fn io_refused_error() -> redis::RedisError {
    redis::RedisError::from(std::io::Error::from(std::io::ErrorKind::ConnectionRefused))
}

/// `redis::RedisError` wrapping an OS connection-reset io error
/// (rule-4 transient).
#[cfg(test)]
pub(crate) fn io_reset_error() -> redis::RedisError {
    redis::RedisError::from(std::io::Error::from(std::io::ErrorKind::ConnectionReset))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io_redis_error(kind: std::io::ErrorKind) -> redis::RedisError {
        redis::RedisError::from(std::io::Error::from(kind))
    }

    #[test]
    fn io_error_source_connection_refused_is_transient() {
        let err =
            redis_error_to_camel("GET", io_redis_error(std::io::ErrorKind::ConnectionRefused));
        assert!(
            is_transient_redis_error(&err),
            "Io-kind RedisError with ConnectionRefused io source must be transient: {err}"
        );
    }

    #[test]
    fn io_custom_text_refused_kind_is_transient() {
        // Flip 1 (accepted, false→true): custom io text with no classifier
        // word — legacy sniffed the Display and missed it; rule 4
        // classifies on the io kind, which is ground truth for an OS
        // connection refusal.
        let err = redis_error_raw(redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "no route",
        )));
        assert!(
            is_transient_redis_error(&err),
            "ConnectionRefused io kind with classifier-less custom text must be transient (flip 1): {err}"
        );
    }

    #[test]
    fn io_error_source_broken_pipe_is_transient() {
        let err = redis_error_to_camel("GET", io_redis_error(std::io::ErrorKind::BrokenPipe));
        assert!(
            is_transient_redis_error(&err),
            "Io-kind RedisError with BrokenPipe io source must be transient: {err}"
        );
    }

    #[test]
    fn io_error_source_permission_denied_is_not_transient() {
        let err = redis_error_to_camel("GET", io_redis_error(std::io::ErrorKind::PermissionDenied));
        assert!(
            !is_transient_redis_error(&err),
            "PermissionDenied io source was never a legacy word match: {err}"
        );
    }

    #[test]
    fn server_readonly_is_transient() {
        let err = redis_error_to_camel(
            "SET",
            redis::RedisError::from((
                redis::ErrorKind::Server(redis::ServerErrorKind::ReadOnly),
                "READONLY You can't write against a read only replica.",
            )),
        );
        assert!(
            is_transient_redis_error(&err),
            "Server(ReadOnly) must be transient (write against a replica; failover in progress): {err}"
        );
    }

    #[test]
    fn server_wrongtype_is_not_transient() {
        let err = redis_error_to_camel(
            "SET",
            redis::RedisError::from((
                redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
                "WRONGTYPE Operation against a key holding the wrong kind of value",
            )),
        );
        assert!(
            !is_transient_redis_error(&err),
            "business WRONGTYPE reply must not be transient: {err}"
        );
    }

    #[test]
    fn server_message_with_classifier_word_falls_back_transient() {
        // Server-controlled message text: no structural shape, but the
        // legacy word match on the RedisError Display must hold.
        let err = redis_error_to_camel(
            "GET",
            redis::RedisError::from((
                redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
                "ERR connection lost while processing",
            )),
        );
        assert!(
            is_transient_redis_error(&err),
            "server message containing a classifier word must stay transient via rule 5: {err}"
        );
    }

    #[test]
    fn cluster_connection_not_found_is_transient() {
        let err = redis_error_to_camel(
            "GET",
            redis::RedisError::from((
                redis::ErrorKind::ClusterConnectionNotFound,
                "cluster connection not found",
            )),
        );
        assert!(
            is_transient_redis_error(&err),
            "ClusterConnectionNotFound must be transient (its Debug rendering contained 'connection'): {err}"
        );
    }

    #[test]
    fn io_general_static_detail_falls_back() {
        // General Io error with a static detail that matches no word.
        let ssl = redis_error_to_camel(
            "GET",
            redis::RedisError::from((redis::ErrorKind::Io, "SSL Handshake error")),
        );
        assert!(
            !is_transient_redis_error(&ssl),
            "'SSL Handshake error - Io' matched no legacy word: {ssl}"
        );
        // Twin: same shape, detail containing a classifier word.
        let dropped = redis_error_to_camel(
            "GET",
            redis::RedisError::from((redis::ErrorKind::Io, "connection dropped")),
        );
        assert!(
            is_transient_redis_error(&dropped),
            "'connection dropped - Io' must stay transient via rule 5: {dropped}"
        );
    }

    #[test]
    fn tls_close_notify_text_falls_back_transient() {
        // io kind not enumerated by rule 4 — the rule-5 fallback on the
        // RedisError Display (the inner io text) decides, and it contains
        // "connection".
        let err = redis_error_to_camel(
            "GET",
            redis::RedisError::from(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "peer closed connection without sending TLS close_notify",
            )),
        );
        assert!(
            is_transient_redis_error(&err),
            "TLS close-notify text must stay transient via rule 5: {err}"
        );
    }

    #[test]
    fn camel_error_io_variant_is_transient() {
        let err = CamelError::Io("cache serialization: bogus".into());
        assert!(
            is_transient_redis_error(&err),
            "CamelError::Io was always transient (IO error: prefix): {err}"
        );
    }

    #[test]
    fn plain_processor_error_text_is_not_sniffed() {
        // The false-positive removal: a bare ProcessorError whose text
        // happens to contain a classifier word is NOT transient.
        let err = CamelError::ProcessorError("connection refused".into());
        assert!(
            !is_transient_redis_error(&err),
            "plain ProcessorError text must not be sniffed: {err}"
        );
    }

    #[test]
    fn budget_marker_is_transient() {
        let err = marker_camel(
            "connection lost while connecting (retry budget exhausted after 3 attempts): x".into(),
            TransientRetryBudgetExhausted {
                stage: "connecting".into(),
                attempts: 3,
            },
        );
        assert!(
            is_transient_redis_error(&err),
            "TransientRetryBudgetExhausted marker must be transient: {err}"
        );
    }

    #[test]
    fn timeout_marker_is_transient() {
        let err = marker_camel(
            "Redis connection to 'redis://x' timed out after 10s".into(),
            TransportTimeout { stage: "connect" },
        );
        assert!(
            is_transient_redis_error(&err),
            "TransportTimeout marker must be transient: {err}"
        );
    }

    #[test]
    fn prose_marker_is_transient() {
        let err = marker_camel(
            "failed to build Redis connection info: can't connect with TLS".into(),
            TransientByProse {
                site: "topology connection info",
            },
        );
        assert!(
            is_transient_redis_error(&err),
            "TransientByProse marker must be transient: {err}"
        );
    }

    #[test]
    fn config_with_transient_substring_is_not_transient() {
        let err = CamelError::Config("CA file at /etc/readonly.pem unreadable".into());
        assert!(
            !is_transient_redis_error(&err),
            "Config errors are never transient, regardless of embedded substrings: {err}"
        );
    }

    #[test]
    fn authentication_failed_is_not_transient() {
        let err = redis_error_to_camel(
            "AUTH",
            redis::RedisError::from((
                redis::ErrorKind::AuthenticationFailed,
                "WRONGPASS invalid username-password pair",
            )),
        );
        assert!(
            !is_transient_redis_error(&err),
            "AuthenticationFailed matched no legacy word and must stay non-transient: {err}"
        );
    }

    /// Local chain node for the hop-bound test: each node's `source()` is
    /// the next node, so a chain of `depth` nodes ends in the terminal
    /// marker.
    #[derive(Debug)]
    struct ChainedError {
        depth: u32,
        next: Option<Box<dyn std::error::Error + Send + Sync>>,
    }

    impl std::fmt::Display for ChainedError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "chained error at depth {}", self.depth)
        }
    }

    impl std::error::Error for ChainedError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.next
                .as_deref()
                .map(|e| e as &(dyn std::error::Error + 'static))
        }
    }

    /// Builds a `CamelError` whose source chain is `depth` `ChainedError`
    /// nodes ending in a `TransientByProse` marker (the marker sits at hop
    /// `depth` from the `CamelError`).
    fn chained_marker_chain(depth: u32) -> CamelError {
        let marker: Box<dyn std::error::Error + Send + Sync> =
            Box::new(TransientByProse { site: "deep chain" });
        let mut tail: Box<dyn std::error::Error + Send + Sync> = marker;
        for d in (1..depth).rev() {
            tail = Box::new(ChainedError {
                depth: d,
                next: Some(tail),
            });
        }
        let root = ChainedError {
            depth,
            next: Some(tail),
        };
        CamelError::ProcessorErrorWithSource("chained".into(), Arc::new(root))
    }

    #[test]
    fn source_chain_beyond_hop_bound_is_not_sniffed() {
        // The walk inspects at most MAX_SOURCE_HOPS (8) source hops. A
        // marker at hop 9 (root + 8 ChainedError intermediates + marker)
        // is beyond the bound and must not flip the verdict.
        let deep = chained_marker_chain(9);
        assert!(
            !is_transient_redis_error(&deep),
            "marker beyond the source-walk hop bound must not be found: {deep}"
        );
        // Twin: the same chain mechanics with the marker at hop 1 must be
        // found — proves the ChainedError source delegation works and the
        // deep verdict is due to the bound, not a broken chain.
        let shallow = chained_marker_chain(1);
        assert!(
            is_transient_redis_error(&shallow),
            "marker within the source-walk hop bound must be found: {shallow}"
        );
    }
}
