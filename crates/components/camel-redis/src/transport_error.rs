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
//!   equal to "does the inner redis Display match a word" — rule 6's
//!   fallback over the inner `RedisError` Display reproduces that exactly;
//! - plain `ProcessorError` text is never sniffed (rule 7): synthetic and
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
//! 5. A preserved `RedisError` whose server code (`RedisError::code()`) is one
//!    of [`SETTLED_FATAL_SERVER_CODES`] (`NOGROUP`, `BUSYGROUP`, `NOPERM`) →
//!    false, decided by the code alone, before any text matching. This
//!    deliberately diverges from the legacy table when a classifier word
//!    appears inside such a message (redis-failover MODIFIED, exemption
//!    sentence: `NOPERM connection is not allowed …` is fatal, although the
//!    legacy word `connection` said transient).
//! 6. Otherwise that same `RedisError`'s Display is matched against the
//!    legacy word set in [`legacy_substring_matches`] — the only substring
//!    matching in the crate's classification. It preserves
//!    server-controlled messages (`ERR connection lost …`), TLS inner
//!    text, and redis-rs static details.
//! 7. No marker and no `RedisError` in the chain → false.
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

/// Server codes settled as NOT transient by rule 5, decided by the code alone
/// and ahead of any text matching.
///
/// `NOGROUP` (stream group/key gone) and `BUSYGROUP` (consumer group already
/// exists) are business outcomes of `XREADGROUP`/`XGROUP CREATE`; `NOPERM` is
/// an ACL denial. None is resolved by reconnecting, so a classifier word
/// inside their message must not flip the verdict (redis-failover spec,
/// rule 5 plus its exemption sentence).
pub(crate) const SETTLED_FATAL_SERVER_CODES: [&str; 3] = ["NOGROUP", "BUSYGROUP", "NOPERM"];

/// The legacy classifier word set — the rule-6 fallback and the ONLY
/// substring matching in the crate's classification.
///
/// Kept verbatim (lowercased contains) so rule 6 reproduces the legacy
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
    // Rules 3–7: bounded walk of the source chain. At each hop, markers
    // win (rule 3) before the first `redis::RedisError` terminates the
    // walk with its own classification (rules 4–6).
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

/// Returns true if `err` proves the cached connection is dead or unusable,
/// so the caller must discard it before reconnecting.
///
/// # Ruled invalidation set
///
/// Exactly these shapes invalidate (ADR-0013, amended with the connection
/// invalidation rule):
///
/// - the [`TransportTimeout`] marker — a half-open TCP socket surfaces as a
///   timeout, so the timeout wrap is proof enough to drop the connection;
/// - a `redis::RedisError` of kind `Server(ReadOnly)` — a demoted master;
///   this is the ONE server reply that invalidates;
/// - a `redis::RedisError` of kind `Io` whose inner `std::io::Error` kind is
///   `ConnectionRefused`, `ConnectionReset`, `ConnectionAborted`, or
///   `BrokenPipe`, or whose Display carries the driver's
///   `connection dropped` phrase.
///
/// Everything else stays valid: every other server reply (settled-fatal
/// codes and server text that merely matches the transient fallback words
/// included), [`TransientRetryBudgetExhausted`], [`TransientByProse`],
/// `ClusterConnectionNotFound`, io `TimedOut`, config errors, and plain
/// `ProcessorError` text.
///
/// `CamelError::Io(_)` is explicitly NOT invalidating although
/// [`is_transient_redis_error`] classifies it transient: the variant holds
/// flattened text with no structural evidence — no `redis::RedisError` and no
/// typed io kind sit in an inspectable chain — so no connection identity can
/// be inferred from it. Real transport failures at the invalidation call
/// sites preserve the `redis::RedisError` in the source chain instead.
///
/// # Subset obligation
///
/// The invalidation set is a strict subset of the transient set: for every
/// error input, `invalidates_connection(e)` implies
/// `is_transient_redis_error(e)`. Discarding a connection is a strictly
/// stronger reaction than retrying, so any shape that warrants invalidation
/// must already warrant a retry. Pinned by the corpus-driven
/// `subset_property_holds_over_corpus` test.
pub fn invalidates_connection(err: &CamelError) -> bool {
    // Rule 1 (shared with the transient classifier): config-family errors are
    // setup defects, never transport failures.
    if matches!(err, CamelError::Config(_) | CamelError::ConfigValidation(_)) {
        return false;
    }
    // `CamelError::Io` is transient but never invalidating — see the
    // doc comment above.
    if matches!(err, CamelError::Io(_)) {
        return false;
    }
    // Same bounded walk as `is_transient_redis_error`; markers and the first
    // `redis::RedisError` terminate it.
    let mut source = err.source();
    let mut hops = 0;
    while let Some(raw) = source {
        if hops >= MAX_SOURCE_HOPS {
            break;
        }
        let src = unwrap_arc_dyn_error(raw);
        if src.downcast_ref::<TransportTimeout>().is_some() {
            return true;
        }
        // Budget exhaustion and prose markers are transient reasons with no
        // structural connection evidence.
        if src
            .downcast_ref::<TransientRetryBudgetExhausted>()
            .is_some()
            || src.downcast_ref::<TransientByProse>().is_some()
        {
            return false;
        }
        if let Some(redis_err) = src.downcast_ref::<redis::RedisError>() {
            return classify_redis_error_for_invalidation(redis_err);
        }
        source = src.source();
        hops += 1;
    }
    // No marker, no `redis::RedisError` — nothing structural to invalidate on.
    false
}

/// Returns the server code (`RedisError::code()`) of the first
/// `redis::RedisError` in the chain starting at `root`, if any.
///
/// `root` is the first chain node to probe, not a wrapper: call sites that
/// hold a raw error pass it directly (`Some(&*arc)`), call sites that hold a
/// `CamelError` use [`redis_error_code`].
///
/// Uses the same bounded walk and Arc unwrapping as
/// [`is_transient_redis_error`], so call sites that must match a reply code
/// (for example `BUSYGROUP` after `XGROUP CREATE`) inspect the preserved
/// server error instead of the rendered message text.
pub(crate) fn redis_error_code_in_source<'a>(
    root: Option<&'a (dyn std::error::Error + 'static)>,
) -> Option<&'a str> {
    let mut source = root;
    let mut hops = 0;
    while let Some(raw) = source {
        if hops >= MAX_SOURCE_HOPS {
            break;
        }
        let src = unwrap_arc_dyn_error(raw);
        if let Some(redis_err) = src.downcast_ref::<redis::RedisError>() {
            return redis_err.code();
        }
        source = src.source();
        hops += 1;
    }
    None
}

/// Returns the server code (`RedisError::code()`) of the first
/// `redis::RedisError` below `err` in its source chain, if any.
///
/// Thin wrapper over [`redis_error_code_in_source`] for call sites that
/// already hold a [`CamelError`].
pub(crate) fn redis_error_code(err: &CamelError) -> Option<&str> {
    redis_error_code_in_source(err.source())
}

/// Returns true when `err` carries the retry-budget-exhaustion marker.
///
/// Budget exhaustion is classified transient for the consumer's metric, but
/// the stream session must treat it as terminal (supervision), so this probes
/// only the [`TransientRetryBudgetExhausted`] marker instead of reusing the
/// transient classifier. It uses the same bounded source-chain walk and Arc
/// unwrapping as [`is_transient_redis_error`].
pub(crate) fn is_retry_budget_exhausted(err: &CamelError) -> bool {
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
        {
            return true;
        }
        source = src.source();
        hops += 1;
    }
    false
}

/// Rules 4–6 applied to the first `redis::RedisError` in a source chain.
fn classify_redis_error(err: &redis::RedisError) -> bool {
    // Rule 4: structural kinds that always rendered a classifier word.
    match err.kind() {
        redis::ErrorKind::Server(redis::ServerErrorKind::ReadOnly)
        | redis::ErrorKind::ClusterConnectionNotFound => return true,
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
            return enumerated_io_kind || legacy_substring_matches(&err.to_string());
        }
        _ => {}
    }
    // Rule 5: settled server codes are decided by the code alone, before any
    // text matching. These three deliberately diverge from the legacy table:
    // when the server message embeds a classifier word (e.g.
    // `NOPERM connection is not allowed for this user`), the legacy substring
    // rule said transient — the code is ground truth that reconnecting cannot
    // help (redis-failover MODIFIED, exemption sentence).
    if err
        .code()
        .is_some_and(|code| SETTLED_FATAL_SERVER_CODES.contains(&code))
    {
        return false;
    }
    // Rule 6: the explicit legacy substring fallback on this RedisError's own
    // Display.
    legacy_substring_matches(&err.to_string())
}

/// The [`invalidates_connection`] verdict for the first `redis::RedisError` in
/// a source chain.
///
/// Only `Server(ReadOnly)` invalidates among server replies; only the four
/// enumerated connection-death io kinds (or the driver's own
/// `connection dropped` phrase) invalidate among io failures. Every other
/// kind — TLS/static text, `ClusterConnectionNotFound`, authentication,
/// business replies, io `TimedOut` — leaves the cached connection in place.
fn classify_redis_error_for_invalidation(err: &redis::RedisError) -> bool {
    match err.kind() {
        // The one server reply that proves a demoted master.
        redis::ErrorKind::Server(redis::ServerErrorKind::ReadOnly) => true,
        // Every other server reply is a business outcome; reconnecting does
        // not help and the socket is fine.
        redis::ErrorKind::Server(_) => false,
        // Transient bookkeeping, deliberately outside the ruled set.
        redis::ErrorKind::ClusterConnectionNotFound => false,
        redis::ErrorKind::Io => {
            let connection_death_io_kind = err
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
                    )
                });
            // redis-rs renders its own dead-socket detection as this phrase on
            // io errors with no typed inner kind.
            connection_death_io_kind
                || err
                    .to_string()
                    .to_lowercase()
                    .contains("connection dropped")
        }
        _ => false,
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
    use camel_api::ConfigValidationError;

    fn io_redis_error(kind: std::io::ErrorKind) -> redis::RedisError {
        redis::RedisError::from(std::io::Error::from(kind))
    }

    /// Builds a real `redis::RedisError` carrying the server code of a RESP
    /// error reply.
    ///
    /// redis-rs 1.8.x has no `Value::Error` constructor, so the reply bytes
    /// are parsed (`redis::parse_redis_value`) into a `Value::ServerError`
    /// and converted with `Value::extract_error()` — the same path a live
    /// connection takes, which is what makes `RedisError::code()` non-`None`.
    fn parsed_server_error(reply: &[u8]) -> redis::RedisError {
        let value = redis::parse_redis_value(reply)
            .unwrap_or_else(|e| panic!("valid RESP error reply {reply:?}: {e}"));
        match value.extract_error() {
            Err(err) => err,
            Ok(other) => panic!("expected a server error reply {reply:?}, got {other:?}"),
        }
    }

    #[test]
    fn stream_server_codes_classify_not_transient() {
        for (reply, expected_code) in [
            (
                &b"-NOGROUP No such key 'stream:x' or consumer group 'g' in XREADGROUP with GROUP option\r\n"[..],
                "NOGROUP",
            ),
            (
                b"-BUSYGROUP Consumer Group name already exists\r\n",
                "BUSYGROUP",
            ),
            (
                b"-NOPERM this user has no permissions to run the 'xreadgroup' command\r\n",
                "NOPERM",
            ),
        ] {
            let err = parsed_server_error(reply);
            // Construction contract: the parsed error really carries the
            // server code the new rule keys on.
            assert_eq!(
                err.code(),
                Some(expected_code),
                "parsed RESP reply must carry code {expected_code}: {reply:?}"
            );
            let wrapped = redis_error_raw(err);
            assert!(
                !is_transient_redis_error(&wrapped),
                "{expected_code} must be not transient via rule 5 (code alone): {wrapped}"
            );
        }
    }

    #[test]
    fn noperm_message_with_classifier_word_stays_fatal() {
        // The deliberate divergence: the legacy substring table matched
        // "connection" inside this server message and said transient.
        let err = parsed_server_error(b"-NOPERM connection is not allowed for this user\r\n");
        assert_eq!(
            err.code(),
            Some("NOPERM"),
            "parsed NOPERM reply must carry its server code"
        );
        let wrapped = redis_error_raw(err);
        assert!(
            !is_transient_redis_error(&wrapped),
            "NOPERM decides by code alone; the word 'connection' must not flip it: {wrapped}"
        );
    }

    #[test]
    fn wrongtype_stays_not_transient() {
        // Existing behavior pinned: WRONGTYPE is a business reply. Its
        // `code()` is populated but sits outside the settled set, so the
        // verdict comes from rule 6, not the new code rule.
        let err = parsed_server_error(
            b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n",
        );
        assert_eq!(
            err.code(),
            Some("WRONGTYPE"),
            "parsed WRONGTYPE reply must carry its server code"
        );
        assert!(
            !SETTLED_FATAL_SERVER_CODES.contains(&"WRONGTYPE"),
            "WRONGTYPE must stay outside the settled code set"
        );
        let wrapped = redis_error_raw(err);
        assert!(
            !is_transient_redis_error(&wrapped),
            "business WRONGTYPE reply must not be transient: {wrapped}"
        );
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
            "server message containing a classifier word must stay transient via rule 6: {err}"
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
            "'connection dropped - Io' must stay transient via rule 6: {dropped}"
        );
    }

    #[test]
    fn tls_close_notify_text_falls_back_transient() {
        // io kind not enumerated by rule 4 — the rule-6 fallback on the
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
            "TLS close-notify text must stay transient via rule 6: {err}"
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

    // ── invalidates_connection ──────────────────────────────────────────

    #[test]
    fn invalidation_shapes_classify_true() {
        let cases: [(&str, CamelError); 4] = [
            (
                "TransportTimeout marker",
                marker_camel(
                    "Redis connection to 'redis://x' timed out after 10s".into(),
                    TransportTimeout { stage: "connect" },
                ),
            ),
            (
                "Server(ReadOnly) reply",
                redis_error_to_camel(
                    "SET",
                    redis::RedisError::from((
                        redis::ErrorKind::Server(redis::ServerErrorKind::ReadOnly),
                        "READONLY You can't write against a read only replica.",
                    )),
                ),
            ),
            (
                "Io-kind RedisError with ConnectionReset source",
                redis_error_raw(io_redis_error(std::io::ErrorKind::ConnectionReset)),
            ),
            (
                "Io-kind RedisError with driver connection-dropped phrase",
                redis_error_raw(redis::RedisError::from((
                    redis::ErrorKind::Io,
                    "connection dropped",
                ))),
            ),
        ];
        for (shape, err) in cases {
            assert!(
                invalidates_connection(&err),
                "{shape} must invalidate the cached connection: {err}"
            );
        }
    }

    #[test]
    fn other_server_replies_never_invalidate() {
        for (reply, code) in [
            (
                &b"-NOGROUP No such key 'stream:x' or consumer group 'g' in XREADGROUP with GROUP option\r\n"[..],
                "NOGROUP",
            ),
            (
                b"-BUSYGROUP Consumer Group name already exists\r\n",
                "BUSYGROUP",
            ),
            (
                b"-NOPERM this user has no permissions to run the 'xreadgroup' command\r\n",
                "NOPERM",
            ),
            (
                b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n",
                "WRONGTYPE",
            ),
        ] {
            let err = redis_error_raw(parsed_server_error(reply));
            assert!(
                !invalidates_connection(&err),
                "{code} is a server reply other than ReadOnly and must not invalidate: {err}"
            );
        }
        // Server text that matches the transient fallback word set: transient
        // yes (rule 6), invalidating no — only `Server(ReadOnly)` invalidates
        // among server replies.
        let fallback = redis_error_to_camel(
            "GET",
            redis::RedisError::from((
                redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
                "ERR connection lost while processing",
            )),
        );
        assert!(
            !invalidates_connection(&fallback),
            "a server reply carrying a transient fallback word must not invalidate: {fallback}"
        );
        assert!(
            is_transient_redis_error(&fallback),
            "the fallback-word reply must stay transient (invalidating is the subset): {fallback}"
        );
    }

    #[test]
    fn io_variant_transient_but_not_invalidating() {
        let err = CamelError::Io("x".into());
        assert!(
            is_transient_redis_error(&err),
            "CamelError::Io stays transient (IO error: prefix): {err}"
        );
        assert!(
            !invalidates_connection(&err),
            "CamelError::Io carries no structural evidence and must not invalidate: {err}"
        );
    }

    #[test]
    fn io_timeout_is_transient_but_not_invalidating() {
        let err = redis_error_raw(io_redis_error(std::io::ErrorKind::TimedOut));
        assert!(
            is_transient_redis_error(&err),
            "Io-kind RedisError with TimedOut source is transient (rule 4): {err}"
        );
        assert!(
            !invalidates_connection(&err),
            "io TimedOut is not in the ruled invalidation set: {err}"
        );
    }

    #[test]
    fn subset_property_holds_over_corpus() {
        let corpus: Vec<(&str, CamelError)> = vec![
            // Shared transient fixtures.
            ("io_refused_error", redis_error_raw(io_refused_error())),
            ("io_reset_error", redis_error_raw(io_reset_error())),
            (
                "io ConnectionRefused via wrap site",
                redis_error_to_camel("GET", io_redis_error(std::io::ErrorKind::ConnectionRefused)),
            ),
            (
                "io BrokenPipe via wrap site",
                redis_error_to_camel("GET", io_redis_error(std::io::ErrorKind::BrokenPipe)),
            ),
            (
                "io ConnectionAborted via wrap site",
                redis_error_to_camel("GET", io_redis_error(std::io::ErrorKind::ConnectionAborted)),
            ),
            (
                "io PermissionDenied via wrap site",
                redis_error_to_camel("GET", io_redis_error(std::io::ErrorKind::PermissionDenied)),
            ),
            (
                "io TimedOut via raw wrap",
                redis_error_raw(io_redis_error(std::io::ErrorKind::TimedOut)),
            ),
            (
                "io custom text with refused kind",
                redis_error_raw(redis::RedisError::from(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "no route",
                ))),
            ),
            // Server replies.
            (
                "Server(ReadOnly)",
                redis_error_to_camel(
                    "SET",
                    redis::RedisError::from((
                        redis::ErrorKind::Server(redis::ServerErrorKind::ReadOnly),
                        "READONLY You can't write against a read only replica.",
                    )),
                ),
            ),
            (
                "Server(ResponseError) with classifier word",
                redis_error_to_camel(
                    "GET",
                    redis::RedisError::from((
                        redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
                        "ERR connection lost while processing",
                    )),
                ),
            ),
            (
                "NOGROUP",
                redis_error_raw(parsed_server_error(
                    b"-NOGROUP No such key 'stream:x' or consumer group 'g' in XREADGROUP with GROUP option\r\n",
                )),
            ),
            (
                "BUSYGROUP",
                redis_error_raw(parsed_server_error(
                    b"-BUSYGROUP Consumer Group name already exists\r\n",
                )),
            ),
            (
                "NOPERM with classifier word",
                redis_error_raw(parsed_server_error(
                    b"-NOPERM connection is not allowed for this user\r\n",
                )),
            ),
            (
                "WRONGTYPE",
                redis_error_raw(parsed_server_error(
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n",
                )),
            ),
            (
                "AuthenticationFailed",
                redis_error_to_camel(
                    "AUTH",
                    redis::RedisError::from((
                        redis::ErrorKind::AuthenticationFailed,
                        "WRONGPASS invalid username-password pair",
                    )),
                ),
            ),
            // Client-side connection bookkeeping and generic io details.
            (
                "ClusterConnectionNotFound",
                redis_error_to_camel(
                    "GET",
                    redis::RedisError::from((
                        redis::ErrorKind::ClusterConnectionNotFound,
                        "cluster connection not found",
                    )),
                ),
            ),
            (
                "generic Io static detail (SSL handshake)",
                redis_error_to_camel(
                    "GET",
                    redis::RedisError::from((redis::ErrorKind::Io, "SSL Handshake error")),
                ),
            ),
            (
                "generic Io driver connection-dropped phrase",
                redis_error_raw(redis::RedisError::from((
                    redis::ErrorKind::Io,
                    "connection dropped",
                ))),
            ),
            (
                "TLS close-notify text",
                redis_error_to_camel(
                    "GET",
                    redis::RedisError::from(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "peer closed connection without sending TLS close_notify",
                    )),
                ),
            ),
            // Markers.
            (
                "TransportTimeout marker",
                marker_camel(
                    "Redis connection to 'redis://x' timed out after 10s".into(),
                    TransportTimeout { stage: "connect" },
                ),
            ),
            (
                "TransientRetryBudgetExhausted marker",
                marker_camel(
                    "connection lost while connecting (retry budget exhausted after 3 attempts): x"
                        .into(),
                    TransientRetryBudgetExhausted {
                        stage: "connecting".into(),
                        attempts: 3,
                    },
                ),
            ),
            (
                "TransientByProse marker",
                marker_camel(
                    "failed to build Redis connection info: can't connect with TLS".into(),
                    TransientByProse {
                        site: "topology connection info",
                    },
                ),
            ),
            // Non-transport and boundary variants.
            (
                "Config error",
                CamelError::Config("CA file at /etc/readonly.pem unreadable".into()),
            ),
            (
                "ConfigValidation error",
                CamelError::ConfigValidation(ConfigValidationError::ThrottlerMaxRequestsZero),
            ),
            (
                "CamelError::Io variant",
                CamelError::Io("cache serialization: bogus".into()),
            ),
            (
                "plain ProcessorError",
                CamelError::ProcessorError("connection refused".into()),
            ),
            // Chained wraps inside and beyond the hop bound.
            ("chained marker depth 1", chained_marker_chain(1)),
            ("chained marker depth 9", chained_marker_chain(9)),
        ];
        let mut invalidating = 0usize;
        for (shape, err) in &corpus {
            let invalidates = invalidates_connection(err);
            if invalidates {
                invalidating += 1;
            }
            assert!(
                !invalidates || is_transient_redis_error(err),
                "subset violated by {shape}: invalidates_connection is true but is_transient_redis_error is false: {err}"
            );
        }
        // Guard against a vacuous property: the corpus must contain the
        // invalidation-positive shapes the ruled set accepts (TransportTimeout
        // marker, Server(ReadOnly), Io ConnectionReset, driver connection-dropped
        // phrase).
        assert!(
            invalidating >= 4,
            "corpus must exercise the invalidation-positive shapes, saw {invalidating}"
        );
    }
}
