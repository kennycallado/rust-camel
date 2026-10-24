use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use camel_component_api::NetworkRetryPolicy;
use sha2::{Digest, Sha256};

use crate::BrokerType;

pub fn default_bridge_cache_dir() -> PathBuf {
    camel_bridge::download::default_cache_dir()
}

// ── Attempt-journal directory and broker key ────────────────────────────────
//
// Rust is the single source of the default resolution of the durable
// attempt-journal directory (jms spec, task 3.6). The resolved directory and
// the derived broker key travel to the Java bridge through the named env vars
// `JMS_ATTEMPT_JOURNAL_DIR` and `JMS_BRIDGE_BROKER_KEY`; Java never re-derives
// the default.

/// A usable state/home root: non-empty and absolute. An empty or relative
/// value counts as unset so a bare `$XDG_STATE_HOME=state` never resolves
/// against an arbitrary current directory.
fn absolute_non_empty(value: &OsString) -> Option<&Path> {
    let path = Path::new(value);
    if value.is_empty() || !path.is_absolute() {
        return None;
    }
    Some(path)
}

/// Pure resolver for the default attempt-journal directory.
///
/// Order: `$XDG_STATE_HOME/rust-camel/jms-bridge` when `XDG_STATE_HOME` is
/// set to a usable value, else `$HOME/.local/state/rust-camel/jms-bridge`.
/// An empty or relative `XDG_STATE_HOME` counts as unset. Errors when neither
/// variable resolves, which is a startup error — never a fallback to the
/// current directory.
///
/// Takes the two env values as arguments so tests stay free of
/// `std::env::set_var` (unsafe in edition 2024).
pub fn resolve_attempt_journal_dir(
    xdg_state_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, camel_component_api::CamelError> {
    if let Some(root) = xdg_state_home.as_ref().and_then(absolute_non_empty) {
        return Ok(root.join("rust-camel").join("jms-bridge"));
    }
    if let Some(root) = home.as_ref().and_then(absolute_non_empty) {
        return Ok(root
            .join(".local")
            .join("state")
            .join("rust-camel")
            .join("jms-bridge"));
    }
    Err(camel_component_api::CamelError::Config(
        "cannot resolve the attempt journal directory: neither XDG_STATE_HOME nor HOME \
         is set to an absolute path"
            .to_string(),
    ))
}

/// Thin environment wrapper over [`resolve_attempt_journal_dir`].
pub fn default_attempt_journal_dir() -> Result<PathBuf, camel_component_api::CamelError> {
    resolve_attempt_journal_dir(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
}

/// Create `dir` and any missing ancestors, mode `0o700` on POSIX. An
/// existing directory is never touched, so its mode is never changed. Rust
/// only creates the path; the Java bridge performs the owner-private check
/// and fails closed when it does not hold (the permanent-failure classifier
/// lands in phase 4). Non-POSIX platforms create the path with default
/// permissions; the bridge skips the privacy-bit check there.
pub fn ensure_attempt_journal_dir(dir: &Path) -> Result<(), camel_component_api::CamelError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(dir) {
        Ok(()) => Ok(()),
        // The directory already exists: leave its ownership and mode as-is.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(camel_component_api::CamelError::Config(format!(
            "cannot create attempt journal directory {}: {e}",
            dir.display()
        ))),
    }
}

/// Stable 32-lowercase-hex broker key: the first 16 bytes of SHA-256 over the
/// configured broker name, the resolved broker type, and the *untruncated*
/// redacted broker URL. The redaction drops credentials but keeps transport
/// policy; it must not be the 256-byte diagnostic truncation, which would
/// conflate two long URLs that share a prefix but differ in the address after
/// the cap.
pub fn attempt_journal_broker_key(
    broker_name: &str,
    broker_type: &BrokerType,
    broker_url: &str,
) -> String {
    let redacted = camel_api::redact::redact_url_with_query_allowlist_untruncated(
        broker_url,
        BROKER_SENSITIVE_KEY_SUBSTRINGS,
    );
    let mut hasher = Sha256::new();
    hasher.update(broker_name.as_bytes());
    hasher.update([0u8]);
    hasher.update(broker_type.as_env_str().as_bytes());
    hasher.update([0u8]);
    hasher.update(redacted.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

fn default_max_bridges() -> usize {
    8
}

fn default_bridge_start_timeout_ms() -> u64 {
    30_000
}

fn default_pool_idle_timeout_ms() -> u64 {
    30_000
}

fn default_pool_expiration_check_ms() -> u64 {
    30_000
}

pub fn default_broker_reconnect_interval_ms() -> u64 {
    5_000
}

/// Per-component reconnect default: unlimited retries (max_attempts=0),
/// preserving the previous infinite-reconnect behavior via BackoffState.
/// Operators can opt into bounded retry via TOML `[reconnect]`.
pub(crate) fn jms_reconnect_default() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 0, // unlimited
        initial_delay: Duration::from_millis(default_broker_reconnect_interval_ms()),
        multiplier: 2.0,
        max_delay: Duration::from_secs(30),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    }
}

fn default_health_check_interval_ms() -> u64 {
    5_000
}

fn default_health_timeout_ms() -> u64 {
    3_000
}

fn default_health_failure_threshold() -> u32 {
    3
}

#[derive(Debug, Clone, PartialEq)]
pub enum DestinationType {
    Queue,
    Topic,
}

// ── JMS-009: Acknowledgement mode ────────────────────────────────────────────

/// JMS session acknowledgement mode.
///
/// Controls how the JMS provider acknowledges consumed messages.
///
/// - `Auto`: The session automatically acknowledges a message after it is
///   delivered to the consumer. This is the simplest mode but may lose
///   messages if the consumer fails before processing.
/// - `Client`: The consumer must explicitly acknowledge each message.
///   Provides full control over when acknowledgement occurs.
/// - `DupsOk`: The session lazily acknowledges messages, which may result
///   in duplicate deliveries. Optimises throughput at the cost of
///   potential duplicates.
/// - `Transacted`: Messages are acknowledged as part of a transaction.
///   See [`JmsTransactionMode`] for transaction configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AcknowledgementMode {
    #[default]
    Auto,
    Client,
    DupsOk,
    Transacted,
}

impl fmt::Display for AcknowledgementMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => write!(f, "Auto"),
            Self::Client => write!(f, "Client"),
            Self::DupsOk => write!(f, "DupsOk"),
            Self::Transacted => write!(f, "Transacted"),
        }
    }
}

impl FromStr for AcknowledgementMode {
    type Err = camel_component_api::CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Auto" | "auto" => Ok(Self::Auto),
            "Client" | "client" => Ok(Self::Client),
            "DupsOk" | "dupsOk" | "dups_ok" => Ok(Self::DupsOk),
            "Transacted" | "transacted" => Ok(Self::Transacted),
            _ => Err(camel_component_api::CamelError::ProcessorError(format!(
                "invalid acknowledgement mode '{}': expected Auto, Client, DupsOk, or Transacted",
                s
            ))),
        }
    }
}

// ── JMS-012: Transaction mode ────────────────────────────────────────────────

/// JMS transaction mode for sessions.
///
/// - `None`: No transaction boundaries are applied. Each operation is
///   auto-acknowledged (subject to the acknowledgement mode).
/// - `Session`: All send/receive operations within a session are batched
///   into a single transaction that must be explicitly committed or rolled
///   back. **Not yet implemented** — using this value emits a warning and
///   falls back to `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JmsTransactionMode {
    #[default]
    None,
    Session,
}

impl fmt::Display for JmsTransactionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Session => write!(f, "Session"),
        }
    }
}

impl FromStr for JmsTransactionMode {
    type Err = camel_component_api::CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "None" | "none" => Ok(Self::None),
            "Session" | "session" => Ok(Self::Session),
            _ => Err(camel_component_api::CamelError::ProcessorError(format!(
                "invalid transaction mode '{}': expected None or Session",
                s
            ))),
        }
    }
}

// ── JMS-005: Exchange pattern ────────────────────────────────────────────────

/// Message exchange pattern for JMS endpoints.
///
/// - `InOnly`: Fire-and-forget (default). The producer sends a message and
///   does not wait for a reply.
/// - `InOut`: Request-reply. The producer sends a message and waits for a
///   correlated reply on a temporary or dedicated reply destination.
///   **Not yet implemented** — using this value emits a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExchangePattern {
    #[default]
    InOnly,
    InOut,
}

impl fmt::Display for ExchangePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InOnly => write!(f, "InOnly"),
            Self::InOut => write!(f, "InOut"),
        }
    }
}

impl FromStr for ExchangePattern {
    type Err = camel_component_api::CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "InOnly" | "inOnly" | "in_only" => Ok(Self::InOnly),
            "InOut" | "inOut" | "in_out" => Ok(Self::InOut),
            _ => Err(camel_component_api::CamelError::ProcessorError(format!(
                "invalid exchange pattern '{}': expected InOnly or InOut",
                s
            ))),
        }
    }
}

// ── Endpoint config ──────────────────────────────────────────────────────────

fn default_concurrent_consumers() -> u32 {
    1
}

#[derive(Debug, Clone)]
pub struct JmsEndpointConfig {
    pub destination_type: DestinationType,
    pub destination_name: String,
    pub broker_name: Option<String>,

    // JMS-009: Acknowledgement mode for the JMS session (default: Auto).
    pub acknowledgement_mode: AcknowledgementMode,

    // JMS-010: JMS SQL-92 selector expression used to filter incoming messages.
    // Only messages matching the selector are delivered to the consumer.
    pub message_selector: Option<String>,

    // JMS-011: Number of concurrent consumer tasks to spawn for this endpoint.
    // Each consumer polls messages independently. Default: 1.
    pub concurrent_consumers: u32,

    // JMS-012: Transaction mode for the JMS session (default: None).
    pub transaction_mode: JmsTransactionMode,

    // JMS-013: QoS options for outbound messages.
    /// Message time-to-live in milliseconds. `None` means no expiration.
    pub time_to_live: Option<u64>,
    /// JMS message priority (0-9, where 9 is highest). `None` uses broker default.
    pub priority: Option<u8>,
    /// Whether to use persistent delivery mode (default: true). Persistent
    /// messages survive broker restarts; non-persistent messages may be lost.
    pub persistent_delivery: bool,

    // JMS-018: When `true` (default), JMS message properties are mapped to
    // Camel exchange headers on receive and Camel exchange headers are mapped
    // to JMS message properties on send.
    pub map_jms_headers: bool,

    // JMS-005: Message exchange pattern (default: InOnly).
    pub exchange_pattern: ExchangePattern,

    // Credential-safe producer header filtering (default: empty). Only exchange
    // headers whose normalized name appears here reach the bridge; an empty
    // list forwards no user headers.
    pub jms_header_allowlist: Vec<String>,
}

impl JmsEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, camel_component_api::CamelError> {
        let (scheme, rest) = if let Some(r) = uri.strip_prefix("jms:") {
            ("jms", r)
        } else if let Some(r) = uri.strip_prefix("activemq:") {
            ("activemq", r)
        } else if let Some(r) = uri.strip_prefix("artemis:") {
            ("artemis", r)
        } else {
            return Err(camel_component_api::CamelError::ProcessorError(
                "expected scheme 'jms', 'activemq', or 'artemis'".to_string(),
            ));
        };

        let (path, query) = match rest.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (rest, None),
        };

        let (destination_type, destination_name) =
            match path.splitn(2, ':').collect::<Vec<_>>().as_slice() {
                // Shorthand (no prefix): only allowed for activemq/artemis, NOT jms
                [name] if !name.is_empty() && scheme != "jms" => {
                    (DestinationType::Queue, name.to_string())
                }
                // Explicit queue: or topic:
                [prefix, name]
                    if (*prefix == "queue" || *prefix == "topic") && !name.is_empty() =>
                {
                    let dt = if *prefix == "queue" {
                        DestinationType::Queue
                    } else {
                        DestinationType::Topic
                    };
                    (dt, name.to_string())
                }
                // jms: shorthand (rejected)
                [name] if !name.is_empty() && scheme == "jms" => {
                    return Err(camel_component_api::CamelError::ProcessorError(format!(
                        "URI 'jms:{}' is ambiguous — use 'jms:queue:{}' or 'jms:topic:{}'",
                        name, name, name
                    )));
                }
                _ => {
                    return Err(camel_component_api::CamelError::ProcessorError(
                        "destination must be 'queue:<name>' or 'topic:<name>'".to_string(),
                    ));
                }
            };

        // Parse query parameters
        let mut broker_name: Option<String> = None;
        let mut acknowledgement_mode = AcknowledgementMode::default();
        let mut message_selector: Option<String> = None;
        let mut concurrent_consumers = default_concurrent_consumers();
        let mut transaction_mode = JmsTransactionMode::default();
        let mut time_to_live: Option<u64> = None;
        let mut priority: Option<u8> = None;
        let mut persistent_delivery = true;
        let mut map_jms_headers = true;
        let mut exchange_pattern = ExchangePattern::default();
        let mut jms_header_allowlist: Vec<String> = Vec::new();

        if let Some(q) = query {
            for kv in q.split('&') {
                let Some((k, v)) = kv.split_once('=') else {
                    continue;
                };
                match k {
                    "broker" if !v.is_empty() => {
                        broker_name = Some(v.to_string());
                    }
                    "acknowledgementMode" | "acknowledgement_mode" => {
                        acknowledgement_mode = AcknowledgementMode::from_str(v)?;
                    }
                    "messageSelector" | "message_selector" if !v.is_empty() => {
                        message_selector = Some(v.to_string());
                    }
                    "concurrentConsumers" | "concurrent_consumers" => {
                        concurrent_consumers = v.parse::<u32>().map_err(|_| {
                            camel_component_api::CamelError::ProcessorError(format!(
                                "invalid concurrent_consumers '{}': expected positive integer",
                                v
                            ))
                        })?;
                        if concurrent_consumers == 0 {
                            return Err(camel_component_api::CamelError::ProcessorError(
                                "concurrent_consumers must be >= 1".to_string(),
                            ));
                        }
                    }
                    "transactionMode" | "transaction_mode" => {
                        transaction_mode = JmsTransactionMode::from_str(v)?;
                    }
                    "timeToLive" | "time_to_live" => {
                        time_to_live = Some(v.parse::<u64>().map_err(|_| {
                            camel_component_api::CamelError::ProcessorError(format!(
                                "invalid time_to_live '{}': expected non-negative integer (ms)",
                                v
                            ))
                        })?);
                    }
                    "priority" => {
                        let p = v.parse::<u8>().map_err(|_| {
                            camel_component_api::CamelError::ProcessorError(format!(
                                "invalid priority '{}': expected integer 0-9",
                                v
                            ))
                        })?;
                        if p > 9 {
                            return Err(camel_component_api::CamelError::ProcessorError(format!(
                                "invalid priority '{}': must be 0-9",
                                p
                            )));
                        }
                        priority = Some(p);
                    }
                    "persistentDelivery" | "persistent_delivery" => {
                        persistent_delivery = v.parse::<bool>().map_err(|_| {
                            camel_component_api::CamelError::ProcessorError(format!(
                                "invalid persistent_delivery '{}': expected true or false",
                                v
                            ))
                        })?;
                    }
                    "mapJmsHeaders" | "map_jms_headers" => {
                        map_jms_headers = v.parse::<bool>().map_err(|_| {
                            camel_component_api::CamelError::ProcessorError(format!(
                                "invalid map_jms_headers '{}': expected true or false",
                                v
                            ))
                        })?;
                    }
                    "exchangePattern" | "exchange_pattern" => {
                        exchange_pattern = ExchangePattern::from_str(v)?;
                    }
                    "jmsHeaderAllowlist" | "jms_header_allowlist" => {
                        jms_header_allowlist = v
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect();
                    }
                    _ => {} // ignore unknown params
                }
            }
        }

        Ok(JmsEndpointConfig {
            destination_type,
            destination_name,
            broker_name,
            acknowledgement_mode,
            message_selector,
            concurrent_consumers,
            transaction_mode,
            time_to_live,
            priority,
            persistent_delivery,
            map_jms_headers,
            exchange_pattern,
            jms_header_allowlist,
        })
    }
}

/// Normalize a JMS header name for allowlist/denylist comparison: casefold,
/// then remove `-`, `_`, and `.` separators.
pub(crate) fn normalize_header_name(key: &str) -> String {
    key.to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.'))
        .collect()
}

#[derive(Clone, PartialEq, serde::Deserialize)]
pub struct BrokerConfig {
    pub broker_url: String,
    pub broker_type: BrokerType,
    pub username: Option<String>,
    pub password: Option<String>,
    pub tls: Option<BrokerTlsConfig>,
}

/// Per-broker TLS material table (provider-neutral). Paths are validated
/// fail-closed at startup by `validate_broker_tls`; file CONTENTS are
/// never read in Rust — the table only ever holds path strings and the
/// SNI name, so no output can leak key material.
#[derive(Clone, PartialEq, serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct BrokerTlsConfig {
    pub ca_cert_path: Option<String>,
    pub client_cert_path: Option<String>,
    pub client_key_path: Option<String>,
    pub server_name: Option<String>,
}

/// Sensitive query-key substrings for broker URL redaction (7 entries,
/// same order as the landed `redact_broker_url` list): a pair whose
/// lowercased key contains any of these renders as `k=<redacted>`.
pub(crate) const BROKER_SENSITIVE_KEY_SUBSTRINGS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "credential",
    "token",
    "username",
    "user",
];

/// Redact credentials embedded in a JMS broker URL (audit 2026-08-31,
/// F5-3; windowed alignment bd rc-eh49). ActiveMQ-style URLs routinely
/// carry credentials:
/// `failover:(tcp://host:61616)?jms.userName=admin&jms.password=secret` or
/// `tcp://user:pass@host`. Userinfo is masked in place as `***@` through
/// the LAST `@` of every `//`-window (a window starts after the `//` plus
/// any run of extra slashes and ends at the next `/`, `?`, or `#`) — the
/// old whole-string `split_once('@')` misfired when an `@` rode the query
/// or fragment. BrokerConfig Debug retains per-key query redaction (not
/// whole-query drop) because ActiveMQ failover URIs encode non-secret
/// transport policy in query params that are the sole diagnostic value of
/// logging the broker URL; secret keys are redacted by the substring
/// allowlist. Fragments are never echoed: everything from the first `#`
/// is dropped and replaced with the `#[redacted]` sentinel. The result is
/// capped at 256 bytes on a UTF-8 char boundary. String-based (no url
/// crate dep in this component). Delegates to the canonical
/// [`camel_api::redact::redact_url_with_query_allowlist`] with the
/// component's 7-entry key list.
pub(crate) fn redact_broker_url(raw: &str) -> String {
    camel_api::redact::redact_url_with_query_allowlist(raw, BROKER_SENSITIVE_KEY_SUBSTRINGS)
}

/// Whether a query key matches the sensitive-key-substring policy. Runs the
/// canonical redactor over a synthetic single-pair URL so the predicate
/// (percent-decoded matching and credential-shape suppression) stays
/// identical to [`redact_broker_url`].
fn is_sensitive_query_key(key: &str) -> bool {
    redact_broker_url(&format!("tcp://h?{key}=probe")).contains("<redacted>")
}

/// Render a query key for an error string. A sensitive key is masked whole
/// (`<redacted>`) rather than as `key=<redacted>`: the key bytes themselves
/// can embed the secret (`password-SECRET`), so echoing the key position
/// would still leak it. Benign keys are returned unchanged.
fn redact_query_key_for_error(key: &str) -> String {
    if is_sensitive_query_key(key) {
        "<redacted>".to_string()
    } else {
        key.to_string()
    }
}

/// Redact a broker URL for a validation error. Same contract as
/// [`redact_broker_url`] except that a query key matching the
/// sensitive-substring policy is masked whole (`<redacted>`): the error also
/// names the offending key, and the redactor's usual `key=<redacted>` form
/// would still echo key bytes such as `password-SECRET`.
fn redact_broker_url_for_error(raw: &str) -> String {
    let Some((base, query)) = raw.split_once('?') else {
        return redact_broker_url(raw);
    };
    let pairs: Vec<String> = query
        .split('&')
        .map(|pair| {
            let key = pair.split('=').next().unwrap_or(pair);
            if is_sensitive_query_key(key) {
                "<redacted>".to_string()
            } else {
                pair.to_string()
            }
        })
        .collect();
    redact_broker_url(&format!("{base}?{}", pairs.join("&")))
}

/// Broker URL schemes accepted by the top-level syntax check (JMS-016).
/// Member-level transport policy is enforced separately by
/// `validate_broker_tls` (which keeps `nio`/`nio+ssl` rejected and can
/// name the unsupported transport).
const KNOWN_URL_SCHEMES: &[&str] = &["tcp", "ssl", "failover", "ws", "wss"];

/// Transport scheme of a broker URL or composite member: the text before
/// the first `://`. The parenthesized failover form `failover:(...)` has
/// no `://`, so `(` terminates the scheme; without this it would yield
/// `failover:(ssl` and fail the known-scheme check. Returns `None` for
/// schemeless members.
fn url_scheme(url: &str) -> Option<&str> {
    let url = url.trim();
    if url.starts_with("failover:(") {
        return Some("failover");
    }
    let (scheme, _) = url.split_once("://")?;
    let scheme = scheme.trim();
    if scheme.is_empty() {
        None
    } else {
        Some(scheme)
    }
}

/// Host component of a broker URL: the text after the first `://` up to the
/// first `:`, `/`, `?`, `#`, `)`, or `,` (a port, path, query, fragment, or
/// composite separator). Bracketed IPv6 literals keep their brackets. Used to
/// compute the expected TLS identity when no `tls.server_name` is configured
/// (task 8.1). Falls back to the whole input when the URL has no `://`.
pub(crate) fn url_host(url: &str) -> &str {
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    if let Some(stripped) = rest.strip_prefix('[')
        && let Some(end) = stripped.find(']')
    {
        return &rest[..end + 2];
    }
    let end = rest
        .find([':', '/', '?', '#', ')', ','])
        .unwrap_or(rest.len());
    &rest[..end]
}

/// Parsed shape of a broker URL: composite members plus, for the
/// `failover:(m1,m2)?outer` paren form, the outer (failover-transport)
/// query. The `failover://member` form keeps its query on the member
/// (prefix strip only — members are never rewritten).
struct CompositeUrl<'a> {
    members: Vec<&'a str>,
    outer_query: Option<&'a str>,
}

fn parse_composite(broker_url: &str) -> CompositeUrl<'_> {
    let url = broker_url.trim();
    let Some(after) = url.strip_prefix("failover:") else {
        return CompositeUrl {
            members: if url.is_empty() {
                Vec::new()
            } else {
                vec![url]
            },
            outer_query: None,
        };
    };
    let after = after.strip_prefix("//").unwrap_or(after);
    let after = after.trim_start();
    let Some(inner) = after.strip_prefix('(') else {
        let tail = after.trim();
        return CompositeUrl {
            members: if tail.is_empty() {
                Vec::new()
            } else {
                vec![tail]
            },
            outer_query: None,
        };
    };
    // Depth scan for the matching ')'; unbalanced input is malformed and
    // yields an empty member list (rejected by the caller).
    let mut depth = 1usize;
    let mut close = None;
    for (i, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return CompositeUrl {
            members: Vec::new(),
            outer_query: None,
        };
    };
    let list = &inner[..close];
    // Depth-0 comma split keeps nested parens intact.
    let mut members = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (i, c) in list.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                members.push(list[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    members.push(list[start..].trim());
    members.retain(|m| !m.is_empty());
    CompositeUrl {
        members,
        outer_query: inner[close + 1..].strip_prefix('?'),
    }
}

/// Members of a broker URL, verbatim (never rewritten):
/// `failover://member?opts` yields the prefix-stripped member,
/// `failover:(m1,m2)?opts` yields the parenthesized list, and a plain
/// `scheme://…` URL yields itself. An empty or malformed member list
/// yields an empty `Vec` (a validation error at the call site).
fn composite_members(broker_url: &str) -> Vec<String> {
    parse_composite(broker_url)
        .members
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Query keys of a URI query string (`a=1&b=2` -> `["a", "b"]`), fragment
/// excluded. Keys without `=` are kept verbatim.
fn query_keys(query: &str) -> Vec<&str> {
    let query = query.split('#').next().unwrap_or(query);
    query
        .split('&')
        .filter_map(|pair| {
            let key = pair.split_once('=').map(|(k, _)| k).unwrap_or(pair);
            if key.is_empty() { None } else { Some(key) }
        })
        .collect()
}

/// Full single-pass `%HH` decode of a query option name (any two hex digits
/// after `%`, case-insensitive; invalid sequences copied verbatim),
/// lowercased — the same key-matching decode the canonical redactor uses,
/// because a denylist matches literal text (`pass%77ord` must hit
/// `password`): percent-encoded evasions of the TLS-store rule cannot slip
/// through. Invalid UTF-8 from a decoded byte becomes `U+FFFD`, which
/// cannot match an ASCII needle.
fn decoded_query_key(key: &str) -> String {
    let bytes = key.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(hi) = (bytes[i + 1] as char).to_digit(16)
            && let Some(lo) = (bytes[i + 2] as char).to_digit(16)
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_lowercase()
}

/// Whether a query option is a `jms.*` TLS-store setting: an option in the
/// `jms.` namespace whose decoded name contains `truststore` or `keystore`
/// (`jms.trustStore`, `jms.keyStore`, `jms.trustStorePassword`,
/// `jms.keyStoreType`, `jms.TRUSTSTORE`, `jms.trust%53tore`, …), matched
/// case-insensitively. ActiveMQ's bean-setting applies these to the
/// connection factory superclass, whose `createTransport()` then rebuilds
/// the TLS managers from those stores — overriding the tls-table PEM
/// pinning — so they are never legal in a broker URL.
fn is_tls_store_query_key(key: &str) -> bool {
    let decoded = decoded_query_key(key);
    decoded.starts_with("jms.") && (decoded.contains("truststore") || decoded.contains("keystore"))
}

/// DNS hostname shape for `tls.server_name`: dot-separated labels of 1-63
/// chars matching `[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?`, total length
/// <= 253.
fn is_valid_dns_hostname(name: &str) -> bool {
    if name.is_empty() || name.len() > 253 {
        return false;
    }
    name.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

/// Fail-closed broker TLS/transport validation. Enforced in order:
/// (a) artemis + tls is a rejected follow-up; nested `failover` members are
/// rejected (classic failover does not nest); (b) composite members must
/// share one transport security class (mixed `tcp`/`ssl` would silently
/// downgrade during failover) — independent of `tls` presence; (c) with a
/// `tls` table every member must be `ssl`; (d) client cert and key go
/// together; (e) `server_name` requires an `ssl` member and must be a DNS
/// hostname (not empty, not an IP literal); (f) query options on `ssl`
/// members are `jms.*` only and never a TLS-store setting (`truststore`
/// or `keystore` names — TLS material comes only from the tls table),
/// composite outer options are `jms.*`, `maxReconnectAttempts`,
/// `startupMaxReconnectAttempts` (the `nested.` prefix is rejected);
/// (g) `jms.clientID` is endpoint-scoped and never
/// legal in a broker URL; (h) activemq members allow `tcp`, `ssl`,
/// `failover` only; (i) configured tls paths must exist, be readable, and
/// never carry the `placeholder-` marker. Every error embeds the redacted
/// broker URL; file contents are never read.
fn validate_broker_tls(broker: &BrokerConfig) -> Result<(), String> {
    let red = redact_broker_url_for_error(&broker.broker_url);

    // (a) Artemis TLS is a tracked follow-up, not wired in this change.
    if broker.broker_type == BrokerType::Artemis && broker.tls.is_some() {
        return Err(format!(
            "broker URL '{red}' configures a tls table with the artemis broker type: TLS for Artemis is a follow-up and is not wired here — drop the tls table or use broker_type = \"activemq\""
        ));
    }

    let members = composite_members(&broker.broker_url);
    let outer_query = parse_composite(&broker.broker_url).outer_query;
    if members.is_empty() {
        return Err(format!("broker URL '{red}' is a composite with no members"));
    }

    // (a.1) Classic failover does not nest: a member that is itself a
    // `failover:(...)` composite hides its children from transport
    // validation, so reject it fail-closed (redacted, contents never read).
    for m in &members {
        if url_scheme(m) == Some("failover") {
            return Err(format!(
                "broker URL '{red}' has nested failover member '{}': failover cannot be nested — list the transport members directly in the outer failover list",
                redact_broker_url_for_error(m)
            ));
        }
    }

    // (b) Transport-security uniformity, independent of tls presence.
    let tls_members = members
        .iter()
        .filter(|m| matches!(url_scheme(m), Some("ssl" | "wss" | "nio+ssl")))
        .count();
    if tls_members != 0 && tls_members != members.len() {
        let named = members
            .iter()
            .map(|m| redact_broker_url_for_error(m))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "broker URL '{red}' has mixed transport security across members ({named}): all members must share one transport security class — a mixed list would silently downgrade to plaintext when failover picks a plaintext member"
        ));
    }

    // (c)+(d)+(e) tls table must agree with the URL and be complete.
    if let Some(tls) = &broker.tls {
        for m in &members {
            if url_scheme(m) != Some("ssl") {
                return Err(format!(
                    "broker URL '{red}' has member '{}' with scheme '{}' but a tls table is configured: every member must parse to the ssl scheme when tls is present",
                    redact_broker_url_for_error(m),
                    url_scheme(m).unwrap_or("(none)")
                ));
            }
        }
        let has_cert = tls.client_cert_path.is_some();
        let has_key = tls.client_key_path.is_some();
        if has_cert != has_key {
            return Err(format!(
                "broker URL '{red}': tls.client_cert_path and tls.client_key_path form an incomplete pair — configure both or neither (client_cert_path is {}, client_key_path is {})",
                if has_cert { "set" } else { "missing" },
                if has_key { "set" } else { "missing" }
            ));
        }
        if let Some(server_name) = &tls.server_name {
            if !members.iter().any(|m| url_scheme(m) == Some("ssl")) {
                return Err(format!(
                    "broker URL '{red}' sets tls.server_name but has no ssl member: server_name requires an ssl transport"
                ));
            }
            if server_name.is_empty() {
                return Err(format!(
                    "broker URL '{red}': tls.server_name is empty: it must be a non-empty DNS hostname"
                ));
            }
            if server_name.parse::<std::net::Ipv4Addr>().is_ok()
                || server_name.parse::<std::net::Ipv6Addr>().is_ok()
                || server_name.contains(':')
            {
                return Err(format!(
                    "broker URL '{red}': tls.server_name '{server_name}' is an IP literal: the DNS hostname rule forbids IP literals"
                ));
            }
            if !is_valid_dns_hostname(server_name) {
                return Err(format!(
                    "broker URL '{red}': tls.server_name '{server_name}' is not a syntactically valid DNS hostname (labels of 1-63 [A-Za-z0-9-] chars, dot-separated, total length <= 253)"
                ));
            }
        }
    }

    // (f) ssl members accept jms.* query options only — and never a
    // TLS-store setting (a `truststore`/`keystore` name in the jms.*
    // namespace: ActiveMQ bean-sets it onto the connection factory and
    // createTransport() rebuilds the TLS managers from that store,
    // overriding the tls-table PEM pinning — TLS material comes only
    // from the tls table). Composite outer options are jms.*,
    // maxReconnectAttempts, startupMaxReconnectAttempts (the nested.
    // prefix is rejected).
    for m in &members {
        if url_scheme(m) != Some("ssl") {
            continue;
        }
        if let Some((_, query)) = m.split_once('?') {
            for key in query_keys(query) {
                if !key.starts_with("jms.") {
                    return Err(format!(
                        "broker URL '{red}': ssl member '{}' has query option '{}': only jms.* query options are allowed on ssl members",
                        redact_broker_url_for_error(m),
                        redact_query_key_for_error(key)
                    ));
                }
                if is_tls_store_query_key(key) {
                    return Err(format!(
                        "broker URL '{red}': ssl member '{}' has query option '{}': jms TLS-store settings are rejected — TLS material comes only from the tls table",
                        redact_broker_url_for_error(m),
                        redact_query_key_for_error(key)
                    ));
                }
            }
        }
    }
    if let Some(query) = outer_query {
        for key in query_keys(query) {
            let allowed = key.starts_with("jms.")
                || key == "maxReconnectAttempts"
                || key == "startupMaxReconnectAttempts";
            if !allowed {
                return Err(format!(
                    "broker URL '{red}': composite outer query option '{}' is not allowed (allowed: jms.*, maxReconnectAttempts, startupMaxReconnectAttempts — the nested. prefix is rejected)",
                    redact_query_key_for_error(key)
                ));
            }
        }
    }

    // (g) Client identity is endpoint-scoped, never broker-URL-scoped.
    let mut all_queries: Vec<&str> = Vec::new();
    for m in &members {
        if let Some((_, query)) = m.split_once('?') {
            all_queries.push(query);
        }
    }
    if let Some(query) = outer_query {
        all_queries.push(query);
    }
    for query in all_queries {
        for key in query_keys(query) {
            if key.eq_ignore_ascii_case("jms.clientID") {
                return Err(format!(
                    "broker URL '{red}' carries query key '{}': client identity is endpoint-scoped — set it on the jms endpoint, not in the broker URL",
                    redact_query_key_for_error(key)
                ));
            }
        }
    }

    // (h) activemq member transport allowlist (keeps ws/wss/nio rejected).
    if broker.broker_type == BrokerType::ActiveMq {
        for m in &members {
            match url_scheme(m) {
                Some("tcp" | "ssl" | "failover") => {}
                other => {
                    return Err(format!(
                        "broker URL '{red}': member '{}' uses unsupported transport scheme '{}': supported schemes are tcp, ssl, failover",
                        redact_broker_url_for_error(m),
                        other.unwrap_or("(none)")
                    ));
                }
            }
        }
    }

    // (i) Configured material must exist and be readable, and must never
    // be placeholder material. Only opened — contents are never read.
    if let Some(tls) = &broker.tls {
        let paths = [
            tls.ca_cert_path.as_ref(),
            tls.client_cert_path.as_ref(),
            tls.client_key_path.as_ref(),
        ];
        for path in paths.into_iter().flatten() {
            if path.contains("placeholder-") {
                return Err(format!(
                    "broker URL '{red}': tls material path '{path}' contains the forbidden 'placeholder-' marker: placeholder material must never be configured"
                ));
            }
            if std::fs::File::open(path).is_err() {
                return Err(format!(
                    "broker URL '{red}': tls material path '{path}' is missing or unreadable: configured tls material must exist and be readable"
                ));
            }
        }
    }

    Ok(())
}

impl std::fmt::Debug for BrokerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokerConfig")
            .field("broker_url", &redact_broker_url(&self.broker_url))
            .field("broker_type", &self.broker_type)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Broker probe strategy for JMS health checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthBrokerProbeMode {
    Reuse,
    Skip,
}

// Kept explicit per the config contract; the derived form would hide the
// operator-visible default choice.
#[allow(clippy::derivable_impls)]
impl Default for HealthBrokerProbeMode {
    fn default() -> Self {
        Self::Reuse
    }
}

impl HealthBrokerProbeMode {
    pub(crate) fn as_env_str(self) -> &'static str {
        match self {
            Self::Reuse => "reuse",
            Self::Skip => "skip",
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct JmsPoolConfig {
    #[serde(default)]
    pub brokers: HashMap<String, BrokerConfig>,
    #[serde(default = "default_max_bridges")]
    pub max_bridges: usize,
    #[serde(default = "default_bridge_start_timeout_ms")]
    pub bridge_start_timeout_ms: u64,
    #[serde(default = "default_pool_idle_timeout_ms")]
    pub pool_idle_timeout_ms: u64,
    #[serde(default = "default_pool_expiration_check_ms")]
    pub pool_expiration_check_ms: u64,
    #[serde(default = "default_broker_reconnect_interval_ms")]
    pub broker_reconnect_interval_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
    #[serde(default = "default_health_timeout_ms")]
    pub health_timeout_ms: u64,
    #[serde(default = "default_health_failure_threshold")]
    pub health_failure_threshold: u32,
    #[serde(default)]
    pub health_broker_probe_mode: HealthBrokerProbeMode,
    #[serde(default = "default_bridge_cache_dir")]
    pub bridge_cache_dir: PathBuf,
    #[serde(default = "jms_reconnect_default")]
    pub reconnect: NetworkRetryPolicy,
}

impl Default for JmsPoolConfig {
    fn default() -> Self {
        Self {
            brokers: HashMap::new(),
            max_bridges: default_max_bridges(),
            bridge_start_timeout_ms: default_bridge_start_timeout_ms(),
            pool_idle_timeout_ms: default_pool_idle_timeout_ms(),
            pool_expiration_check_ms: default_pool_expiration_check_ms(),
            broker_reconnect_interval_ms: default_broker_reconnect_interval_ms(),
            health_check_interval_ms: default_health_check_interval_ms(),
            health_timeout_ms: default_health_timeout_ms(),
            health_failure_threshold: default_health_failure_threshold(),
            health_broker_probe_mode: HealthBrokerProbeMode::default(),
            bridge_cache_dir: default_bridge_cache_dir(),
            reconnect: jms_reconnect_default(),
        }
    }
}

impl JmsPoolConfig {
    /// Convenience constructor for single-broker scenarios (tests, simple examples).
    /// Creates a pool with one broker named "default".
    pub fn single_broker(broker_url: impl Into<String>, broker_type: BrokerType) -> Self {
        let url = broker_url.into();
        let mut brokers = HashMap::new();
        brokers.insert(
            "default".to_string(),
            BrokerConfig {
                broker_url: url,
                broker_type,
                username: None,
                password: None,
                tls: None,
            },
        );
        Self {
            brokers,
            max_bridges: 1,
            ..Self::default()
        }
    }

    /// Validates the config: all brokers must have non-empty URLs with a known
    /// scheme, `max_bridges` must be >= 1, and all timing fields must be strictly
    /// positive to prevent busy-loops.
    pub fn validate(&self) -> Result<(), camel_component_api::CamelError> {
        use camel_component_api::CamelError;

        if self.max_bridges < 1 {
            return Err(CamelError::Config("max_bridges must be >= 1".to_string()));
        }

        for (name, bc) in &self.brokers {
            if bc.broker_url.is_empty() {
                return Err(CamelError::ProcessorError(format!(
                    "broker '{}' has an empty broker_url",
                    name
                )));
            }

            let scheme = url_scheme(&bc.broker_url);
            let has_known_scheme = scheme.is_some_and(|s| KNOWN_URL_SCHEMES.contains(&s));
            if !has_known_scheme {
                // Re-review of F5-2: validation errors are persisted to the
                // redb journal — the broker URL must be redacted here too,
                // not only in Debug.
                return Err(CamelError::ProcessorError(format!(
                    "broker '{}' has an invalid broker_url '{}': must start with one of {:?}",
                    name,
                    redact_broker_url(&bc.broker_url),
                    KNOWN_URL_SCHEMES
                        .iter()
                        .map(|s| format!("{s}://"))
                        .collect::<Vec<_>>()
                )));
            }

            if bc.broker_url.starts_with("failover://") && bc.broker_type == BrokerType::Artemis {
                return Err(CamelError::ProcessorError(format!(
                    "broker '{}' uses failover:// with the artemis broker type (URL '{}'): the Artemis sidecar does not unwrap failover wrappers — use a single primary broker URL or multiple broker entries",
                    name,
                    redact_broker_url(&bc.broker_url)
                )));
            }

            if let Err(msg) = validate_broker_tls(bc) {
                return Err(CamelError::ProcessorError(format!(
                    "broker '{name}': {msg}"
                )));
            }
        }

        if self.bridge_start_timeout_ms == 0 {
            return Err(CamelError::Config(
                "bridge_start_timeout_ms must be > 0".to_string(),
            ));
        }
        if !(1_000..=86_400_000).contains(&self.pool_idle_timeout_ms) {
            return Err(CamelError::Config(format!(
                "pool_idle_timeout_ms must be in 1000..=86400000 ms (got {}): shorter holds fewer idle connections on the broker but reopens more often on slow cadences",
                self.pool_idle_timeout_ms
            )));
        }
        if !(1_000..=86_400_000).contains(&self.pool_expiration_check_ms) {
            return Err(CamelError::Config(format!(
                "pool_expiration_check_ms must be in 1000..=86400000 ms (got {}): this is how long expired connections linger before the sweep removes them",
                self.pool_expiration_check_ms
            )));
        }
        if self.health_check_interval_ms == 0 {
            return Err(CamelError::Config(
                "health_check_interval_ms must be > 0".to_string(),
            ));
        }
        if self.health_timeout_ms == 0 {
            return Err(CamelError::Config(
                "health_timeout_ms must be > 0".to_string(),
            ));
        }
        if self.health_failure_threshold == 0 {
            return Err(CamelError::Config(
                "health_failure_threshold must be > 0".to_string(),
            ));
        }
        if self.broker_reconnect_interval_ms == 0 {
            return Err(CamelError::Config(
                "broker_reconnect_interval_ms must be > 0".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrokerType;

    /// Re-review of F5-2: validation errors persist to the redb journal —
    /// the emitted error string must carry the redacted URL, not the raw one.
    #[test]
    fn validate_error_redacts_broker_url_credentials() {
        let mut cfg = super::JmsPoolConfig::default();
        cfg.brokers.insert(
            "primary".to_string(),
            crate::BrokerConfig {
                broker_url: "http://admin:secretpass@evil.example.com:61616".to_string(),
                broker_type: BrokerType::Artemis,
                username: None,
                password: None,
                tls: None,
            },
        );
        let err = cfg.validate().unwrap_err().to_string();
        assert!(
            !err.contains("secretpass"),
            "error must not carry credentials: {err}"
        );
        assert!(
            err.contains("***@evil.example.com"),
            "redacted host kept for diagnosability: {err}"
        );
    }

    /// Audit 2026-08-31, F5-3: JMS broker URLs must not leak credentials
    /// through Debug output.
    #[test]
    fn redact_broker_url_masks_userinfo_and_sensitive_query() {
        // userinfo form
        let redacted = redact_broker_url("tcp://admin:secretpass@broker.example.com:61616");
        assert!(
            !redacted.contains("secretpass"),
            "password masked: {redacted}"
        );
        assert!(
            redacted.contains("broker.example.com"),
            "host visible: {redacted}"
        );

        // failover + query-param form (ActiveMQ style)
        let redacted = redact_broker_url(
            "failover:(tcp://host:61616)?jms.userName=admin&jms.password=secret&keepAlive=true",
        );
        assert!(
            !redacted.contains("secret"),
            "password param masked: {redacted}"
        );
        assert!(
            !redacted.contains("=admin"),
            "username param masked: {redacted}"
        );
        assert!(
            redacted.contains("keepAlive=true"),
            "benign param kept: {redacted}"
        );

        // clean URL untouched
        assert_eq!(redact_broker_url("tcp://host:61616"), "tcp://host:61616");
    }

    /// Task 8.1: the expected TLS identity falls back to the URL host, so
    /// `url_host` must strip port, path, query, and composite separators.
    #[test]
    fn url_host_extracts_host_from_broker_urls() {
        assert_eq!(url_host("ssl://10.0.0.5:61617"), "10.0.0.5");
        assert_eq!(url_host("tcp://mq.internal:61616"), "mq.internal");
        assert_eq!(
            url_host("ssl://mq.internal:61617/path?x=1#frag"),
            "mq.internal"
        );
        assert_eq!(url_host("failover:(ssl://a:1,tcp://b:2)"), "a");
        assert_eq!(url_host("ssl://[::1]:61617"), "[::1]");
    }

    /// Delegation pin: `redact_broker_url` is a thin wrapper over the
    /// canonical `camel_api::redact::redact_url_with_query_allowlist` —
    /// benign params stay visible (ActiveMQ diagnosability).
    #[test]
    fn redact_broker_url_delegation_pin() {
        assert_eq!(
            redact_broker_url("tcp://h:61616?keepAlive=true"),
            "tcp://h:61616?keepAlive=true"
        );
    }

    /// bd rc-tfugr, jms spec scenario "credential-shaped key never
    /// echoes": a denylist-matched key whose single-pass minimal decode
    /// carries `@` AND (`:` OR `//`) renders as a bare `<redacted>` —
    /// the key bytes may themselves embed the credential (ADR-0076
    /// appendix, key-position credential-shape symmetry).
    #[test]
    fn redact_broker_url_suppresses_credential_shaped_key() {
        for fixture in [
            "tcp://h:61616?user%3Asecret%40host=1",
            "tcp://h:61616?user%3asecret%40host=1",
            "tcp://h:61616?user:pass@host=1",
        ] {
            let redacted = redact_broker_url(fixture);
            assert_eq!(redacted, "tcp://h:61616?<redacted>");
            assert!(
                !redacted.contains("secret")
                    && !redacted.contains("user%3Asecret%40")
                    && !redacted.contains("user:pass"),
                "credential must not echo in any position: {redacted}"
            );
        }
    }

    /// bd rc-tfugr, jms spec scenario "non-shaped sensitive keys keep
    /// their key names": a denylist-matched key that does not decode to
    /// the credential shape renders as `{raw_key}=<redacted>` — the
    /// authored key bytes survive for transport-policy diagnosis.
    #[test]
    fn redact_broker_url_keeps_non_shaped_sensitive_keys() {
        assert_eq!(
            redact_broker_url("tcp://h:61616?password=p&jms.userName=admin&user=u&keepAlive=true"),
            "tcp://h:61616?password=<redacted>&jms.userName=<redacted>&user=<redacted>&keepAlive=true"
        );
        assert_eq!(
            redact_broker_url("tcp://h:61616?user@host=1"),
            "tcp://h:61616?user@host=<redacted>"
        );
        assert_eq!(
            redact_broker_url("tcp://h:61616?pass%77ord=p"),
            "tcp://h:61616?pass%77ord=<redacted>"
        );
    }

    /// Exact-output pin: userinfo is fully masked by `***`, never partially
    /// truncated, in scheme://...@ authority position.
    #[test]
    fn redact_exact_userinfo_mask() {
        assert_eq!(
            redact_broker_url("tcp://admin:secretpass@broker.example.com:61616"),
            "tcp://***@broker.example.com:61616"
        );
    }

    /// Exact-output pin: sensitive query params redact to `<redacted>`,
    /// benign params survive byte-for-byte, `&` separators preserved.
    #[test]
    fn redact_exact_query_join() {
        assert_eq!(
            redact_broker_url("tcp://host:61616?password=p&user=u&keepAlive=true"),
            "tcp://host:61616?password=<redacted>&user=<redacted>&keepAlive=true"
        );
    }

    /// Exact-output pin: a bare `user@host` (no scheme) is NOT an authority
    /// position — passthrough untouched.
    #[test]
    fn redact_exact_bare_at_passthrough() {
        assert_eq!(redact_broker_url("admin@host"), "admin@host");
    }

    /// Exact-output pin: delimiter-exact query redaction on the ActiveMQ
    /// failover form — the whole query part after '?' must match verbatim,
    /// no dropped params, no mangled separators.
    #[test]
    fn redact_exact_failover_param_boundaries() {
        let redacted = redact_broker_url(
            "failover:(tcp://host:61616)?jms.userName=admin&jms.password=secret&keepAlive=true",
        );
        let (_, query) = redacted.split_once('?').expect("query segment after '?'");
        assert_eq!(
            query,
            "jms.userName=<redacted>&jms.password=<redacted>&keepAlive=true"
        );
    }

    /// bd rc-eh49 exact pin: an `@` riding the query is NOT userinfo — the
    /// URL must pass through byte-for-byte. The old whole-string
    /// `split_once('@')` mangled this to `failover:(tcp://***@b`.
    #[test]
    fn redact_broker_url_query_at_no_misfire() {
        assert_eq!(
            redact_broker_url("failover:(tcp://h:61616)?x=a@b"),
            "failover:(tcp://h:61616)?x=a@b"
        );
    }

    /// bd rc-eh49 exact pin: a slash run after `//` is preserved
    /// byte-for-byte and the windowed mask composes with the per-key query
    /// allowlist. The old whole-string scan rewrote from the scheme's own
    /// `://` and swallowed the extra slashes.
    #[test]
    fn redact_exact_slash_run_window_composition() {
        assert_eq!(
            redact_broker_url("tcp:////user:pass@h:61616?keepAlive=true"),
            "tcp:////***@h:61616?keepAlive=true"
        );
    }

    /// bd rc-eh49 exact pin: the window mask consumes through the LAST `@`
    /// of the window, so an `@` embedded in the userinfo cannot keep a
    /// spoofable prefix alive (the old first-`@` scan left `a@` visible).
    #[test]
    fn redact_exact_last_at_in_window() {
        assert_eq!(
            redact_broker_url("tcp://u:p@a@h:61616"),
            "tcp://***@h:61616"
        );
    }

    /// bd rc-eh49: the 256-byte cap cuts on a char boundary — a multibyte
    /// character straddling byte 256 is dropped whole, never split
    /// mid-encode.
    #[test]
    fn redact_broker_url_truncate_multibyte_boundary() {
        let mut url = String::from("tcp://broker:61616/");
        url.push_str(&"x".repeat(236)); // 255 ASCII bytes before the multibyte char
        url.push('日'); // 3 bytes straddling the 256-byte cap
        url.push_str(&"y".repeat(50)); // push the total past 300 bytes
        assert!(url.len() > 300);
        let redacted = redact_broker_url(&url);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(std::str::from_utf8(redacted.as_bytes()).is_ok());
        assert!(
            !redacted.contains('日'),
            "straddling char dropped whole: {redacted}"
        );
    }

    /// bd rc-eh49: a `//` window after the first is scanned too; the
    /// benign query stays visible per the broker allowlist exception.
    #[test]
    fn redact_broker_url_later_window_masked() {
        assert_eq!(
            redact_broker_url("tcp://h//user:pass@x/?keepAlive=true"),
            "tcp://h//***@x/?keepAlive=true"
        );
    }

    /// bd rc-eh49: fragments are never echoed — everything from the first
    /// `#` is dropped and the `#[redacted]` sentinel appended; processed
    /// query params stay visible.
    #[test]
    fn redact_broker_url_drops_fragment() {
        assert_eq!(
            redact_broker_url("tcp://h:61616?keepAlive=true#tok=x"),
            "tcp://h:61616?keepAlive=true#[redacted]"
        );
    }

    /// bd rc-eh49: broker URLs are capped at 256 bytes on a UTF-8 char
    /// boundary.
    #[test]
    fn redact_broker_url_truncates() {
        let mut url = format!("tcp://broker:61616/{}", "x".repeat(300));
        url.push('日');
        url.push_str(&"tail".repeat(20));
        assert!(url.len() > 300);
        let redacted = redact_broker_url(&url);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(redacted.starts_with("tcp://broker:61616/"));
    }

    #[test]
    fn broker_config_debug_redacts_url_credentials() {
        let cfg = BrokerConfig {
            broker_url: "failover:(tcp://h:61616)?jms.password=topsecret".to_string(),
            broker_type: BrokerType::ActiveMq,
            username: Some("admin".to_string()),
            password: Some("topsecret".to_string()),
            tls: None,
        };
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("topsecret"), "Debug must not leak: {dbg}");
    }

    /// bd rc-r7v8s (redact2 jms spec scenario): a percent-encoded
    /// credential riding a benign-looking redirect param must not survive
    /// Debug output — neither the decoded secret nor the encoded userinfo
    /// bytes may appear, and the bare `<redacted>` marker must.
    #[test]
    fn broker_debug_masks_encoded_credentials() {
        let cfg = BrokerConfig {
            broker_url: "tcp://h:61616?redirect=http%3A%2F%2Fuser%3Asecret%40host".to_string(),
            broker_type: BrokerType::ActiveMq,
            username: None,
            password: None,
            tls: None,
        };
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("secret"), "encoded credential leaked: {dbg}");
        assert!(
            !dbg.contains("user%3Asecret%40"),
            "encoded userinfo leaked: {dbg}"
        );
        assert!(dbg.contains("<redacted>"), "bare marker expected: {dbg}");
    }

    #[test]
    fn parse_jms_queue_explicit() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
        assert_eq!(cfg.broker_name, None);
    }

    #[test]
    fn parse_jms_topic_explicit() {
        let cfg = JmsEndpointConfig::from_uri("jms:topic:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Topic);
        assert_eq!(cfg.destination_name, "events");
    }

    #[test]
    fn parse_jms_with_broker_param() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?broker=primary").unwrap();
        assert_eq!(cfg.broker_name, Some("primary".to_string()));
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn jms_shorthand_rejected() {
        let err = JmsEndpointConfig::from_uri("jms:orders").unwrap_err();
        assert!(err.to_string().contains("ambiguous"), "got: {}", err);
    }

    #[test]
    fn invalid_destination_type_returns_error() {
        let err = JmsEndpointConfig::from_uri("jms:inbox:orders").unwrap_err();
        assert!(
            err.to_string().contains("'queue' or 'topic'")
                || err.to_string().contains("queue:<name>"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_activemq_queue_explicit() {
        let cfg = JmsEndpointConfig::from_uri("activemq:queue:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn parse_activemq_shorthand() {
        let cfg = JmsEndpointConfig::from_uri("activemq:orders").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "orders");
    }

    #[test]
    fn parse_artemis_shorthand_defaults_to_queue() {
        let cfg = JmsEndpointConfig::from_uri("artemis:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Queue);
        assert_eq!(cfg.destination_name, "events");
    }

    #[test]
    fn parse_jms_with_empty_broker_param_treated_as_none() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?broker=").unwrap();
        assert_eq!(cfg.broker_name, None);
    }

    #[test]
    fn parse_activemq_topic() {
        let cfg = JmsEndpointConfig::from_uri("activemq:topic:events").unwrap();
        assert_eq!(cfg.destination_type, DestinationType::Topic);
        assert_eq!(cfg.destination_name, "events");
    }

    #[test]
    fn single_broker_convenience() {
        let cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        assert_eq!(cfg.max_bridges, 1);
        assert!(cfg.brokers.contains_key("default"));
        let bc = &cfg.brokers["default"];
        assert_eq!(bc.broker_url, "tcp://localhost:61616");
        assert_eq!(bc.broker_type, BrokerType::ActiveMq);
    }

    #[test]
    fn default_pool_config() {
        let cfg = JmsPoolConfig::default();
        assert_eq!(cfg.max_bridges, 8);
        assert!(cfg.brokers.is_empty());
        assert_eq!(cfg.bridge_start_timeout_ms, 30_000);
        assert_eq!(cfg.broker_reconnect_interval_ms, 5_000);
        assert_eq!(cfg.health_check_interval_ms, 5_000);
    }

    #[test]
    fn validate_empty_broker_url() {
        let cfg = JmsPoolConfig::single_broker("", BrokerType::ActiveMq);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("empty broker_url"), "got: {}", err);
    }

    #[test]
    fn broker_config_debug_redacts_password() {
        let bc = BrokerConfig {
            broker_url: "tcp://localhost:61616".to_string(),
            broker_type: BrokerType::ActiveMq,
            username: Some("admin".to_string()),
            password: Some("secret".to_string()),
            tls: None,
        };
        let s = format!("{bc:?}");
        assert!(s.contains("<redacted>"), "got: {s}");
        assert!(!s.contains("secret"), "got: {s}");
    }

    #[test]
    fn validate_ok() {
        let cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.validate().unwrap();
    }

    #[test]
    fn validate_rejects_zero_bridge_start_timeout() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.bridge_start_timeout_ms = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("bridge_start_timeout_ms"),
            "got: {}",
            err
        );
    }

    fn assert_pool_knob_rejected(field: &str, value: u64) {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        match field {
            "pool_idle_timeout_ms" => cfg.pool_idle_timeout_ms = value,
            "pool_expiration_check_ms" => cfg.pool_expiration_check_ms = value,
            other => panic!("unknown field {other}"),
        }
        let err = cfg.validate().unwrap_err();
        assert!(
            matches!(err, camel_component_api::CamelError::Config(_)),
            "expected CamelError::Config, got: {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains(field), "missing field name in: {msg}");
        assert!(msg.contains("1000"), "missing range lower bound in: {msg}");
        assert!(
            msg.contains("86400000"),
            "missing range upper bound in: {msg}"
        );
    }

    #[test]
    fn validate_rejects_zero_pool_idle_timeout() {
        assert_pool_knob_rejected("pool_idle_timeout_ms", 0);
    }

    #[test]
    fn validate_rejects_sub_second_pool_idle_timeout() {
        assert_pool_knob_rejected("pool_idle_timeout_ms", 999);
    }

    #[test]
    fn validate_rejects_above_max_pool_idle_timeout() {
        assert_pool_knob_rejected("pool_idle_timeout_ms", 86_400_001);
    }

    #[test]
    fn validate_rejects_zero_pool_expiration_check() {
        assert_pool_knob_rejected("pool_expiration_check_ms", 0);
    }

    #[test]
    fn validate_rejects_sub_second_pool_expiration_check() {
        assert_pool_knob_rejected("pool_expiration_check_ms", 999);
    }

    #[test]
    fn validate_rejects_above_max_pool_expiration_check() {
        assert_pool_knob_rejected("pool_expiration_check_ms", 86_400_001);
    }

    #[test]
    fn validate_accepts_pool_knob_endpoints() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.pool_idle_timeout_ms = 1_000;
        cfg.pool_expiration_check_ms = 1_000;
        cfg.validate().unwrap();
        cfg.pool_idle_timeout_ms = 86_400_000;
        cfg.pool_expiration_check_ms = 86_400_000;
        cfg.validate().unwrap();
    }

    #[test]
    fn pool_knob_serde_defaults() {
        let cfg = JmsPoolConfig::default();
        assert_eq!(cfg.pool_idle_timeout_ms, 30_000);
        assert_eq!(cfg.pool_expiration_check_ms, 30_000);

        let toml = r#"
[brokers.default]
broker_url = "tcp://localhost:61616"
broker_type = "activemq"
"#;
        let cfg: JmsPoolConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.pool_idle_timeout_ms, 30_000);
        assert_eq!(cfg.pool_expiration_check_ms, 30_000);
    }

    #[test]
    fn validate_rejects_zero_health_check_interval() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.health_check_interval_ms = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("health_check_interval_ms"),
            "got: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_zero_reconnect_interval() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.broker_reconnect_interval_ms = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("broker_reconnect_interval_ms"),
            "got: {}",
            err
        );
    }

    #[test]
    fn health_settings_default_to_3000_3_and_reuse() {
        let cfg = JmsPoolConfig::default();
        assert_eq!(cfg.health_timeout_ms, 3_000);
        assert_eq!(cfg.health_failure_threshold, 3);
        assert_eq!(cfg.health_broker_probe_mode, HealthBrokerProbeMode::Reuse);
        assert_eq!(cfg.health_check_interval_ms, 5_000);
    }

    #[test]
    fn partial_toml_receives_health_defaults() {
        let toml = r#"
[brokers.default]
broker_url = "tcp://localhost:61616"
broker_type = "activemq"
"#;
        let cfg: JmsPoolConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.health_timeout_ms, 3_000);
        assert_eq!(cfg.health_failure_threshold, 3);
        assert_eq!(cfg.health_broker_probe_mode, HealthBrokerProbeMode::Reuse);
    }

    #[test]
    fn toml_parses_skip_probe_mode() {
        let cfg: JmsPoolConfig = toml::from_str(r#"health_broker_probe_mode = "skip""#).unwrap();
        assert_eq!(cfg.health_broker_probe_mode, HealthBrokerProbeMode::Skip);
    }

    #[test]
    fn toml_rejects_unknown_probe_mode() {
        let err =
            toml::from_str::<JmsPoolConfig>(r#"health_broker_probe_mode = "probe""#).unwrap_err();
        assert!(
            err.to_string().contains("health_broker_probe_mode"),
            "got: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_zero_health_timeout() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.health_timeout_ms = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("health_timeout_ms"),
            "got: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_zero_health_failure_threshold() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.health_failure_threshold = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("health_failure_threshold"),
            "got: {}",
            err
        );
    }

    // ── JMS-006: max_bridges validation ──────────────────────────────────────

    #[test]
    fn rejects_zero_max_bridges() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.max_bridges = 0;
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("max_bridges"), "got: {}", err);
    }

    #[test]
    fn rejects_zero_timeout() {
        let mut cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        cfg.bridge_start_timeout_ms = 0;
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("bridge_start_timeout_ms"),
            "got: {}",
            err
        );
    }

    // ── JMS-016: broker URL scheme validation ────────────────────────────────

    #[test]
    fn rejects_empty_broker_url() {
        let cfg = JmsPoolConfig::single_broker("", BrokerType::ActiveMq);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("empty broker_url"), "got: {}", err);
    }

    #[test]
    fn rejects_unknown_broker_url_scheme() {
        let cfg = JmsPoolConfig::single_broker("amqp://localhost:5672", BrokerType::ActiveMq);
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("invalid broker_url"),
            "got: {}",
            err
        );
    }

    #[test]
    fn accepts_known_broker_url_schemes() {
        // ws:// and wss:// are syntactically known but rejected as
        // unsupported activemq transports by `validate_broker_tls`
        // (see `ws_and_nio_schemes_rejected`).
        for url in &[
            "tcp://localhost:61616",
            "ssl://localhost:61617",
            "failover://tcp://localhost:61616",
        ] {
            let cfg = JmsPoolConfig::single_broker(*url, BrokerType::ActiveMq);
            assert!(cfg.validate().is_ok(), "scheme should be accepted: {url}");
        }
    }

    #[test]
    fn rejects_failover_scheme_for_artemis_with_migration_hint() {
        let cfg =
            JmsPoolConfig::single_broker("failover://tcp://localhost:61616", BrokerType::Artemis);
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("failover"), "got: {}", err);
        assert!(
            err.to_string().contains("failover://tcp://localhost:61616"),
            "error must name the URL: {}",
            err
        );
        assert!(err.to_string().contains("broker entries"), "got: {}", err);
    }

    #[test]
    fn accepts_failover_scheme_for_classic() {
        let cfg =
            JmsPoolConfig::single_broker("failover://tcp://localhost:61616", BrokerType::ActiveMq);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn accepts_valid_config() {
        let cfg = JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::ActiveMq);
        // ensure max_bridges and timeout have valid defaults
        assert!(cfg.validate().is_ok());
    }

    // ── Bridge lockstep: scheme-only validation, TLS material left to sidecar ──

    /// A secure-scheme URL passes config validation even though the component
    /// holds no TLS material (truststore/keystore options do not exist here).
    /// Missing TLS material is the Java sidecar's fail-loud concern
    /// (BRIDGE_BROKER_* contract), not the Rust config layer's. Artemis is
    /// the only broker type the Java sidecar accepts with a secure scheme
    /// (broker-type guard in JmsClientFactory).
    #[test]
    fn secure_scheme_passes_through_without_material() {
        let cfg = JmsPoolConfig::single_broker("ssl://broker:61617", BrokerType::Artemis);
        cfg.validate().unwrap();
    }

    #[test]
    fn unknown_scheme_rejected() {
        let cfg = JmsPoolConfig::single_broker("stomp://broker:61613", BrokerType::ActiveMq);
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("invalid broker_url"),
            "got: {}",
            err
        );
    }

    // ── JMS-005/JMS-009/JMS-010/JMS-011/JMS-012/JMS-013/JMS-018: endpoint config ──

    #[test]
    fn default_endpoint_config_has_sensible_defaults() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        assert_eq!(cfg.acknowledgement_mode, AcknowledgementMode::Auto);
        assert_eq!(cfg.message_selector, None);
        assert_eq!(cfg.concurrent_consumers, 1);
        assert_eq!(cfg.transaction_mode, JmsTransactionMode::None);
        assert_eq!(cfg.time_to_live, None);
        assert_eq!(cfg.priority, None);
        assert!(cfg.persistent_delivery);
        assert!(cfg.map_jms_headers);
        assert_eq!(cfg.exchange_pattern, ExchangePattern::InOnly);
    }

    #[test]
    fn parse_acknowledgement_mode_client() {
        let cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?acknowledgementMode=Client").unwrap();
        assert_eq!(cfg.acknowledgement_mode, AcknowledgementMode::Client);
    }

    #[test]
    fn parse_acknowledgement_mode_dups_ok() {
        let cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?acknowledgement_mode=dups_ok").unwrap();
        assert_eq!(cfg.acknowledgement_mode, AcknowledgementMode::DupsOk);
    }

    #[test]
    fn parse_acknowledgement_mode_invalid() {
        let err = JmsEndpointConfig::from_uri("jms:queue:orders?acknowledgementMode=invalid")
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid acknowledgement mode"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_message_selector() {
        let cfg =
            JmsEndpointConfig::from_uri("jms:queue:orders?messageSelector=priority%20%3E%205")
                .unwrap();
        // URL encoding is NOT decoded by from_uri — the raw value is stored
        assert_eq!(cfg.message_selector, Some("priority%20%3E%205".to_string()));
    }

    #[test]
    fn parse_empty_message_selector_is_none() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?message_selector=").unwrap();
        assert_eq!(cfg.message_selector, None);
    }

    #[test]
    fn parse_concurrent_consumers() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?concurrentConsumers=4").unwrap();
        assert_eq!(cfg.concurrent_consumers, 4);
    }

    #[test]
    fn parse_concurrent_consumers_zero_rejected() {
        let err =
            JmsEndpointConfig::from_uri("jms:queue:orders?concurrentConsumers=0").unwrap_err();
        assert!(
            err.to_string()
                .contains("concurrent_consumers must be >= 1"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_concurrent_consumers_invalid() {
        let err =
            JmsEndpointConfig::from_uri("jms:queue:orders?concurrentConsumers=abc").unwrap_err();
        assert!(
            err.to_string().contains("invalid concurrent_consumers"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_transaction_mode_session() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?transactionMode=Session").unwrap();
        assert_eq!(cfg.transaction_mode, JmsTransactionMode::Session);
    }

    #[test]
    fn parse_transaction_mode_invalid() {
        let err =
            JmsEndpointConfig::from_uri("jms:queue:orders?transaction_mode=invalid").unwrap_err();
        assert!(
            err.to_string().contains("invalid transaction mode"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_time_to_live() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?timeToLive=30000").unwrap();
        assert_eq!(cfg.time_to_live, Some(30_000));
    }

    #[test]
    fn parse_priority() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?priority=5").unwrap();
        assert_eq!(cfg.priority, Some(5));
    }

    #[test]
    fn parse_priority_above_9_rejected() {
        let err = JmsEndpointConfig::from_uri("jms:queue:orders?priority=10").unwrap_err();
        assert!(err.to_string().contains("must be 0-9"), "got: {}", err);
    }

    /// Adversarial URI validation (jmsmutants 1.2): an unsupported scheme
    /// must fail with the scheme error, not be misparsed as a destination.
    #[test]
    fn from_uri_rejects_unsupported_scheme() {
        let err = JmsEndpointConfig::from_uri("http:queue:orders").unwrap_err();
        assert!(err.to_string().contains("expected scheme"), "got: {}", err);
    }

    /// Adversarial URI validation (jmsmutants 1.2): an empty destination name
    /// must surface the destination-format error, never the `jms:` shorthand
    /// ambiguity error — the shorthand guard cannot be replaced with an
    /// unconditional match arm.
    #[test]
    fn from_uri_rejects_empty_destination_name_with_format_error() {
        for uri in ["jms:queue:", "jms:topic:", "jms:"] {
            let err = JmsEndpointConfig::from_uri(uri).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("destination must be"),
                "uri {uri:?} got: {msg}"
            );
            assert!(
                !msg.contains("ambiguous"),
                "uri {uri:?} must not report ambiguity: {msg}"
            );
        }
    }

    /// Adversarial URI validation (jmsmutants 1.2): a non-empty `jms:<name>`
    /// shorthand is rejected with the exact ambiguity message, pinning the
    /// em-dash wording and the queue/topic guidance. Matches the
    /// `ProcessorError` payload so the assertion is not coupled to the
    /// framework's `Processor error: ` Display prefix.
    #[test]
    fn from_uri_rejects_ambiguous_jms_shorthand() {
        let err = JmsEndpointConfig::from_uri("jms:orders").unwrap_err();
        let message = match err {
            camel_component_api::CamelError::ProcessorError(message) => message,
            other => panic!("expected ProcessorError, got: {other:?}"),
        };
        assert_eq!(
            message,
            "URI 'jms:orders' is ambiguous — use 'jms:queue:orders' or 'jms:topic:orders'"
        );
    }

    /// Adversarial URI validation (jmsmutants 1.2): priority 9 is the highest
    /// accepted value and priority 10 is rejected, pinning the strict `p > 9`
    /// boundary.
    #[test]
    fn from_uri_enforces_priority_boundary() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?priority=9").unwrap();
        assert_eq!(cfg.priority, Some(9));
        assert!(
            JmsEndpointConfig::from_uri("jms:queue:orders?priority=10").is_err(),
            "priority 10 must be rejected"
        );
    }

    #[test]
    fn parse_persistent_delivery_false() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?persistentDelivery=false").unwrap();
        assert!(!cfg.persistent_delivery);
    }

    #[test]
    fn parse_map_jms_headers_false() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?mapJmsHeaders=false").unwrap();
        assert!(!cfg.map_jms_headers);
    }

    #[test]
    fn parse_exchange_pattern_inout() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?exchangePattern=InOut").unwrap();
        assert_eq!(cfg.exchange_pattern, ExchangePattern::InOut);
    }

    #[test]
    fn parse_exchange_pattern_invalid() {
        let err =
            JmsEndpointConfig::from_uri("jms:queue:orders?exchangePattern=invalid").unwrap_err();
        assert!(
            err.to_string().contains("invalid exchange pattern"),
            "got: {}",
            err
        );
    }

    #[test]
    fn parse_multiple_query_params() {
        let cfg = JmsEndpointConfig::from_uri(
            "jms:queue:orders?broker=primary&acknowledgementMode=Client&concurrentConsumers=3&persistentDelivery=false&priority=7",
        )
        .unwrap();
        assert_eq!(cfg.broker_name, Some("primary".to_string()));
        assert_eq!(cfg.acknowledgement_mode, AcknowledgementMode::Client);
        assert_eq!(cfg.concurrent_consumers, 3);
        assert!(!cfg.persistent_delivery);
        assert_eq!(cfg.priority, Some(7));
    }

    #[test]
    fn acknowledgement_mode_display_roundtrip() {
        for mode in &[
            AcknowledgementMode::Auto,
            AcknowledgementMode::Client,
            AcknowledgementMode::DupsOk,
            AcknowledgementMode::Transacted,
        ] {
            let s = mode.to_string();
            let parsed: AcknowledgementMode = s.parse().unwrap();
            assert_eq!(*mode, parsed);
        }
    }

    #[test]
    fn transaction_mode_display_roundtrip() {
        for mode in &[JmsTransactionMode::None, JmsTransactionMode::Session] {
            let s = mode.to_string();
            let parsed: JmsTransactionMode = s.parse().unwrap();
            assert_eq!(*mode, parsed);
        }
    }

    #[test]
    fn exchange_pattern_display_roundtrip() {
        for mode in &[ExchangePattern::InOnly, ExchangePattern::InOut] {
            let s = mode.to_string();
            let parsed: ExchangePattern = s.parse().unwrap();
            assert_eq!(*mode, parsed);
        }
    }

    #[test]
    fn build_exchange_without_header_mapping() {
        // Verify map_jms_headers=false prevents header mapping in consumer
        // (tested indirectly through the consumer module's build_exchange)
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?mapJmsHeaders=false").unwrap();
        assert!(!cfg.map_jms_headers);
    }

    #[test]
    fn jms_pool_config_has_reconnect_policy() {
        let cfg = JmsPoolConfig::default();
        assert_eq!(cfg.reconnect.max_attempts, 0); // unlimited
        assert!(cfg.reconnect.enabled);
    }

    /// Pins every field overridden by `jms_reconnect_default`. `multiplier`
    /// and `max_delay` are equivalent to the values supplied by
    /// `NetworkRetryPolicy::default()` (2.0 and 30s respectively), so their
    /// deletion would not change behavior; the remaining three fields
    /// (`max_attempts`, `initial_delay`, `jitter_factor`) differ from the
    /// defaults and are killed by these assertions.
    #[test]
    fn jms_reconnect_default_pins_all_overridden_fields() {
        let policy = jms_reconnect_default();
        assert_eq!(policy.max_attempts, 0);
        assert_eq!(policy.initial_delay, Duration::from_secs(5));
        assert_eq!(policy.jitter_factor, 0.0);
        assert_eq!(policy.max_delay, Duration::from_secs(30));
        assert_eq!(policy.multiplier, 2.0);
    }

    /// Pins the cross-crate cache subdirectory contract (jmsmutants 1.3):
    /// `default_bridge_cache_dir()` must delegate to the concrete
    /// `camel_bridge::download::default_cache_dir()` helper and must never
    /// fall back to an empty `PathBuf`.
    #[test]
    fn default_bridge_cache_dir_matches_bridge_download_default() {
        let delegated = camel_bridge::download::default_cache_dir();
        let component = default_bridge_cache_dir();
        assert_eq!(component, delegated);
        assert_ne!(component, PathBuf::default());
    }

    #[test]
    fn jms_header_allowlist_parses_comma_list() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders?jmsHeaderAllowlist=a,b").unwrap();
        assert_eq!(
            cfg.jms_header_allowlist,
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn jms_header_allowlist_defaults_empty() {
        let cfg = JmsEndpointConfig::from_uri("jms:queue:orders").unwrap();
        assert!(cfg.jms_header_allowlist.is_empty());
    }

    #[test]
    fn normalize_header_name_folds_separators() {
        assert_eq!(normalize_header_name("api-key"), "apikey");
        assert_eq!(normalize_header_name("API_KEY"), "apikey");
        assert_eq!(normalize_header_name("api_key"), "apikey");
    }

    // ── Task 3.6: Rust-owned journal directory and broker key ───────────

    /// The pure resolver takes the env values as arguments, so it never
    /// calls `std::env::set_var` (unsafe in edition 2024).
    #[test]
    fn resolve_attempt_journal_dir_uses_xdg_then_home() {
        let xdg = OsString::from("/var/lib/state");
        let home = OsString::from("/home/alice");
        let xdg_path = PathBuf::from("/var/lib/state/rust-camel/jms-bridge");
        let home_path = PathBuf::from("/home/alice/.local/state/rust-camel/jms-bridge");

        assert_eq!(
            resolve_attempt_journal_dir(Some(xdg.clone()), Some(home.clone())).unwrap(),
            xdg_path
        );
        // Empty XDG counts as unset.
        assert_eq!(
            resolve_attempt_journal_dir(Some(OsString::from("")), Some(home.clone())).unwrap(),
            home_path
        );
        // Relative XDG counts as unset.
        assert_eq!(
            resolve_attempt_journal_dir(Some(OsString::from("state")), Some(home.clone())).unwrap(),
            home_path
        );
        // HOME only.
        assert_eq!(
            resolve_attempt_journal_dir(None, Some(home.clone())).unwrap(),
            home_path
        );
        // XDG wins even when HOME is absent.
        assert_eq!(
            resolve_attempt_journal_dir(Some(xdg), None).unwrap(),
            xdg_path
        );

        // No-home error (never a fallback to the current directory).
        let err = resolve_attempt_journal_dir(None, None).unwrap_err();
        assert!(
            err.to_string().contains("XDG_STATE_HOME") || err.to_string().contains("HOME"),
            "error must name the missing variables: {err}"
        );
        assert!(
            resolve_attempt_journal_dir(None, Some(OsString::from("home/alice"))).is_err(),
            "relative HOME is not a usable root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn ensure_attempt_journal_dir_creates_0700() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("nested").join("jms-bridge");
        ensure_attempt_journal_dir(&child).unwrap();
        assert!(child.is_dir(), "missing directory must be created");
        let mode = std::fs::metadata(&child).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "created dir must be owner-private");

        // An existing directory is never chmodded.
        let existing = tmp.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_attempt_journal_dir(&existing).unwrap();
        let mode = std::fs::metadata(&existing).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "existing dir mode must be preserved");
    }

    #[test]
    fn broker_key_hash_is_stable() {
        let a = attempt_journal_broker_key("primary", &BrokerType::ActiveMq, "tcp://h:61616");
        let b = attempt_journal_broker_key("primary", &BrokerType::ActiveMq, "tcp://h:61616");
        assert_eq!(a, b, "same inputs must hash equal");
        assert_eq!(a.len(), 32, "key must be 32 hex chars");
        assert!(
            a.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "key must be lowercase hex: {a}"
        );

        assert_ne!(
            a,
            attempt_journal_broker_key("secondary", &BrokerType::ActiveMq, "tcp://h:61616"),
            "broker name must participate in the key"
        );
        assert_ne!(
            a,
            attempt_journal_broker_key("primary", &BrokerType::Artemis, "tcp://h:61616"),
            "broker type must participate in the key"
        );
        assert_ne!(
            a,
            attempt_journal_broker_key("primary", &BrokerType::ActiveMq, "tcp://h:61617"),
            "broker URL must participate in the key"
        );
    }

    /// Two long URLs that share a 256-byte prefix but differ after it must
    /// not collide: the key hashes the untruncated redaction.
    #[test]
    fn broker_key_distinguishes_long_urls_past_diagnostic_cap() {
        let shared = format!(
            "tcp://h:61616?keepAlive=true&brokerName={}",
            "a".repeat(300)
        );
        let url_a = format!("{shared}&distinctAddressA=1");
        let url_b = format!("{shared}&distinctAddressB=2");
        assert_ne!(
            attempt_journal_broker_key("primary", &BrokerType::ActiveMq, &url_a),
            attempt_journal_broker_key("primary", &BrokerType::ActiveMq, &url_b),
            "distinct addresses after the diagnostic cap must produce distinct keys"
        );
    }

    /// Credentials must not leak into the derived key: the hash input is the
    /// redacted URL, and the key is pure lowercase hex. Same credentials on
    /// the same broker are stable.
    #[test]
    fn broker_key_is_credential_free() {
        let a = attempt_journal_broker_key(
            "primary",
            &BrokerType::ActiveMq,
            "tcp://admin:secret@h:61616",
        );
        let b = attempt_journal_broker_key(
            "primary",
            &BrokerType::ActiveMq,
            "tcp://admin:secret@h:61616",
        );
        assert_eq!(a, b, "same redacted input must hash equal");
        assert!(
            a.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "key must be lowercase hex: {a}"
        );
    }

    // ── Task 1.1: BrokerTlsConfig + fail-closed broker TLS validation ────────

    fn tls_broker(url: &str, tls: Option<BrokerTlsConfig>) -> BrokerConfig {
        BrokerConfig {
            broker_url: url.to_string(),
            broker_type: BrokerType::ActiveMq,
            username: None,
            password: None,
            tls,
        }
    }

    fn empty_tls() -> BrokerTlsConfig {
        BrokerTlsConfig {
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            server_name: None,
        }
    }

    /// Dummy CA + client pair written into `dir`. Validation must only open
    /// these paths — never read their contents.
    fn file_backed_tls(dir: &std::path::Path) -> Result<BrokerTlsConfig, std::io::Error> {
        let ca = dir.join("ca.pem");
        let cert = dir.join("client.pem");
        let key = dir.join("client-key.pem");
        for p in [&ca, &cert, &key] {
            std::fs::write(p, b"-----NOT-REAL-PEM-----")?;
        }
        Ok(BrokerTlsConfig {
            ca_cert_path: Some(ca.display().to_string()),
            client_cert_path: Some(cert.display().to_string()),
            client_key_path: Some(key.display().to_string()),
            server_name: None,
        })
    }

    #[test]
    fn tls_config_accepts_pinned_ca_and_mtls() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let tls = file_backed_tls(dir.path())?;
        let bc = tls_broker("ssl://mq:61617", Some(tls));
        if let Err(e) = validate_broker_tls(&bc) {
            panic!("pinned CA + client pair must pass: {e}");
        }
        Ok(())
    }

    #[test]
    fn tls_on_artemis_rejected() {
        let bc = BrokerConfig {
            broker_type: BrokerType::Artemis,
            ..tls_broker("ssl://mq:61617", Some(empty_tls()))
        };
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(err.contains("artemis"), "got: {err}");
    }

    #[test]
    fn tls_on_plaintext_url_rejected() {
        let bc = tls_broker("tcp://mq:61616", Some(empty_tls()));
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(err.contains("tcp"), "error must name the scheme: {err}");
        assert!(err.contains("tls"), "error must name the tls table: {err}");
    }

    #[test]
    fn mixed_composite_rejected() {
        for tls in [None, Some(empty_tls())] {
            let bc = tls_broker("failover:(tcp://a:61616,ssl://b:61617)", tls);
            let err = validate_broker_tls(&bc).unwrap_err();
            assert!(err.contains("mixed"), "got: {err}");
            assert!(err.contains("tcp://a:61616"), "must name member: {err}");
            assert!(err.contains("ssl://b:61617"), "must name member: {err}");

            // The same rejection must flow through the production entry.
            let mut cfg = JmsPoolConfig::default();
            cfg.brokers.insert("primary".to_string(), bc);
            let prod_err = cfg.validate().unwrap_err().to_string();
            assert!(prod_err.contains("mixed"), "production path: {prod_err}");
            assert!(
                prod_err.contains("tcp://a:61616"),
                "production path must name member: {prod_err}"
            );
            assert!(
                prod_err.contains("ssl://b:61617"),
                "production path must name member: {prod_err}"
            );
        }
    }

    #[test]
    fn nested_failover_rejected() {
        for tls in [None, Some(empty_tls())] {
            let bc = tls_broker("failover:(failover:(tcp://a:61616,ssl://b:61617))", tls);
            let err = validate_broker_tls(&bc).unwrap_err();
            assert!(
                err.contains("nested failover"),
                "error must name nested failover: {err}"
            );

            // The same rejection must flow through the production entry.
            let mut cfg = JmsPoolConfig::default();
            cfg.brokers.insert("primary".to_string(), bc);
            let prod_err = cfg.validate().unwrap_err().to_string();
            assert!(
                prod_err.contains("nested failover"),
                "production path: {prod_err}"
            );
        }
    }

    #[test]
    fn all_ssl_composite_accepted() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let tls = file_backed_tls(dir.path())?;
        let bc = tls_broker("failover:(ssl://a:61617,ssl://b:61617)", Some(tls));
        assert!(
            validate_broker_tls(&bc).is_ok(),
            "all-ssl composite must pass"
        );

        // The parenthesized failover form must also clear the production
        // scheme check, not only validate_broker_tls.
        let mut cfg = JmsPoolConfig::default();
        cfg.brokers.insert("primary".to_string(), bc);
        if let Err(e) = cfg.validate() {
            panic!("all-ssl composite must pass production validation: {e}");
        }
        Ok(())
    }

    #[test]
    fn client_cert_without_key_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let cert = dir.path().join("client.pem");
        std::fs::write(&cert, b"-----NOT-REAL-PEM-----")?;
        let tls = BrokerTlsConfig {
            ca_cert_path: None,
            client_cert_path: Some(cert.display().to_string()),
            client_key_path: None,
            server_name: None,
        };
        let bc = tls_broker("ssl://mq:61617", Some(tls));
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(
            err.contains("client_cert_path"),
            "must name the pair: {err}"
        );
        assert!(err.contains("client_key_path"), "must name the pair: {err}");
        Ok(())
    }

    #[test]
    fn server_name_ip_literal_rejected() {
        let tls = BrokerTlsConfig {
            server_name: Some("10.0.0.5".to_string()),
            ..empty_tls()
        };
        let bc = tls_broker("ssl://mq:61617", Some(tls));
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(err.contains("server_name"), "rule must be named: {err}");
        assert!(err.contains("IP literal"), "rule must be named: {err}");

        let tls = BrokerTlsConfig {
            server_name: Some(String::new()),
            ..empty_tls()
        };
        let bc = tls_broker("ssl://mq:61617", Some(tls));
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(err.contains("server_name"), "rule must be named: {err}");
        assert!(err.contains("empty"), "rule must be named: {err}");
    }

    #[test]
    fn server_name_on_plaintext_rejected() {
        let tls = BrokerTlsConfig {
            server_name: Some("mq.internal.example".to_string()),
            ..empty_tls()
        };
        let bc = tls_broker("tcp://mq:61616", Some(tls));
        assert!(
            validate_broker_tls(&bc).is_err(),
            "server_name without an ssl member must be rejected"
        );
    }

    #[test]
    fn verify_hostname_socket_option_rejected() {
        let bc = tls_broker("ssl://h:1?socket.verifyHostName=false", None);
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(
            err.contains("socket.verifyHostName"),
            "forbidden option must be named: {err}"
        );
    }

    /// Pin-preservation (security review HIGH finding): a broker URL may carry
    /// `jms.trustStore`/`jms.keyStore` (and password/type variants) as `jms.*`
    /// query options; ActiveMQ's bean-setting applies them to the connection
    /// factory superclass, whose `createTransport()` then REBUILDS the TLS
    /// managers from those stores — overriding the tls-table PEM pinning
    /// (expert-reproduced: pinned issuer count 1 -> 111 with JDK cacerts).
    /// Rejected at validation.
    #[test]
    fn tls_store_url_options_rejected() {
        for option in ["jms.trustStore", "jms.keyStore"] {
            let bc = tls_broker(&format!("ssl://h:1?{option}=store.jks"), None);
            let err = validate_broker_tls(&bc).unwrap_err();
            assert!(err.contains(option), "rejected option must be named: {err}");
            assert!(
                err.contains("tls table"),
                "error must point at the tls table: {err}"
            );

            // The same rejection must flow through the production entry.
            let mut cfg = JmsPoolConfig::default();
            cfg.brokers.insert("primary".to_string(), bc);
            let prod_err = cfg.validate().unwrap_err().to_string();
            assert!(prod_err.contains(option), "production path: {prod_err}");
        }
    }

    /// A query key matching the sensitive-key-substring policy must never be
    /// echoed: the key bytes can embed the secret (`password-SECRET`), so the
    /// key position and the redacted URL both render a bare `<redacted>`.
    #[test]
    fn sensitive_key_in_error_redacted() {
        let bc = tls_broker("ssl://h:1?password-SECRET=x", None);
        let err = match validate_broker_tls(&bc) {
            Ok(()) => panic!("sensitive-key URL must be rejected"),
            Err(err) => err,
        };
        assert!(!err.contains("SECRET"), "raw key bytes leaked: {err}");
        assert_eq!(
            err,
            "broker URL 'ssl://h:1?<redacted>': ssl member 'ssl://h:1?<redacted>' has query option '<redacted>': only jms.* query options are allowed on ssl members"
        );
    }

    #[test]
    fn nested_outer_option_rejected() {
        let bc = tls_broker(
            "failover:(ssl://h:1)?nested.socket.verifyHostName=false",
            None,
        );
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(err.contains("nested"), "nested option must be named: {err}");
    }

    #[test]
    fn max_reconnect_attempts_outer_accepted() {
        let bc = tls_broker("failover:(ssl://h:1)?maxReconnectAttempts=10", None);
        assert!(
            validate_broker_tls(&bc).is_ok(),
            "maxReconnectAttempts is a legal outer option"
        );
    }

    #[test]
    fn jms_client_id_in_url_rejected() {
        let bc = tls_broker("ssl://h:1?jms.clientID=x", None);
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(
            err.contains("endpoint"),
            "error must name endpoint scope: {err}"
        );
    }

    #[test]
    fn ws_and_nio_schemes_rejected() {
        for (url, scheme) in [
            ("ws://h:1", "ws"),
            ("wss://h:1", "wss"),
            ("nio://h:1", "nio"),
        ] {
            let bc = tls_broker(url, None);
            let err = validate_broker_tls(&bc).unwrap_err();
            assert!(err.contains("unsupported transport"), "got: {err}");
            assert!(err.contains(scheme), "transport must be named: {err}");
        }
    }

    #[test]
    fn unknown_tls_key_rejected() {
        let toml_src = r#"
[brokers.default]
broker_url = "ssl://mq:61617"
broker_type = "activemq"

[brokers.default.tls]
insecure_skip_verify = true
"#;
        let err = toml::from_str::<JmsPoolConfig>(toml_src).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("insecure_skip_verify"), "unknown key: {msg}");
    }

    #[test]
    fn placeholder_path_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let p = dir.path().join("placeholder-ca.pem");
        std::fs::write(&p, b"-----NOT-REAL-PEM-----")?;
        let tls = BrokerTlsConfig {
            ca_cert_path: Some(p.display().to_string()),
            ..empty_tls()
        };
        let bc = tls_broker("ssl://mq:61617", Some(tls));
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(err.contains("placeholder"), "got: {err}");

        // A never-created path is rejected as unreadable.
        let missing = dir.path().join("missing-ca.pem");
        let tls = BrokerTlsConfig {
            ca_cert_path: Some(missing.display().to_string()),
            ..empty_tls()
        };
        let bc = tls_broker("ssl://mq:61617", Some(tls));
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(err.contains("unreadable"), "got: {err}");
        Ok(())
    }

    #[test]
    fn placeholder_directory_marker_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let material = dir.path().join("placeholder-material");
        std::fs::create_dir_all(&material)?;
        let p = material.join("ca.pem");
        std::fs::write(&p, b"-----NOT-REAL-PEM-----")?;
        let tls = BrokerTlsConfig {
            ca_cert_path: Some(p.display().to_string()),
            ..empty_tls()
        };
        let bc = tls_broker("ssl://mq:61617", Some(tls));
        let err = validate_broker_tls(&bc).unwrap_err();
        assert!(
            err.contains("placeholder"),
            "directory marker must trip the check: {err}"
        );
        Ok(())
    }

    #[test]
    fn debug_renders_paths_not_contents() -> Result<(), Box<dyn std::error::Error>> {
        // Type-level guarantee: the table holds path strings only, never
        // bytes, so file contents cannot appear in any output.
        fn assert_option_string_field(_: &Option<String>) {}
        let dir = tempfile::tempdir()?;
        let p = dir.path().join("ca.pem");
        std::fs::write(&p, b"MAGIC-CONTENTS-MARKER-42")?;
        let tls = BrokerTlsConfig {
            ca_cert_path: Some(p.display().to_string()),
            client_cert_path: Some("/etc/certs/client.pem".to_string()),
            client_key_path: Some("/etc/certs/client-key.pem".to_string()),
            server_name: Some("mq.internal.example".to_string()),
        };
        assert_option_string_field(&tls.ca_cert_path);
        assert_option_string_field(&tls.client_cert_path);
        assert_option_string_field(&tls.client_key_path);
        assert_option_string_field(&tls.server_name);
        let rendered = format!("{tls:?}");
        assert!(
            rendered.contains(&p.display().to_string()),
            "path must render: {rendered}"
        );
        assert!(
            rendered.contains("/etc/certs/client.pem"),
            "path must render: {rendered}"
        );
        assert!(
            !rendered.contains("MAGIC-CONTENTS-MARKER-42"),
            "file contents must never render: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn valid_dns_server_names_accepted() {
        for name in ["mq.internal.example", "a.b", "x-y.example"] {
            let tls = BrokerTlsConfig {
                server_name: Some(name.to_string()),
                ..empty_tls()
            };
            let bc = tls_broker("ssl://mq:61617", Some(tls));
            assert!(
                validate_broker_tls(&bc).is_ok(),
                "server_name {name} must be accepted"
            );
        }
    }
}
