//! Scenario `redis:` prepare actions — datasource state preparation
//! for the integration tier (bd rc-6wodc, bd rc-ql8eq).
//!
//! Mirrors the scenario `sql:` branch (bd rc-25lup): `redis:` deletes
//! document-authored keys through a datasource before route
//! assertions run, or in a document-level `finally:` list as teardown;
//! reads stay in the `validate` redis target
//! (`datasource/key/type/ttl`) and the two vocabularies are never
//! mixed. Two action forms share the key:
//!
//! - `del:` deletes literal keys in ONE `DEL`;
//!   [`validate_redis_del_action`] enforces the literal-key law.
//! - `unlinkPrefix:` deletes keys under a harness-minted namespace
//!   prefix with `SCAN` + `UNLINK`; it is FINALLY-ONLY, and
//!   [`validate_redis_unlink_prefix`] proves the template names a
//!   declared namespace bindVar with a glob-free literal suffix.
//!
//! Delete-only by design: no `FLUSHDB`, no `FLUSHALL`, no
//! document-authored pattern delete, no generic command runner. A key
//! containing a glob metacharacter is a load-time grammar defect for
//! `del`; `unlinkPrefix` escapes the resolved prefix and audits every
//! scan reply before unlinking.
//!
//! The grammar and validation are ungated and run in every build;
//! executing either form is demand-gated behind the harness `redis`
//! feature (the `sql:`/`surreal:` precedent, ADR-0069 §8). Executor
//! diagnostics pass through [`crate::steering::sanitize_db_error`],
//! which redacts the datasource URL per ADR-0051 (Credential
//! Redaction at Diagnostic Boundaries).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;

/// Key under which a scenario action selects this vocabulary.
pub const REDIS_DEL_ACTION_KEY: &str = "redis";

/// Redis glob metacharacters the `unlinkPrefix` suffix must not carry
/// (bd rc-ql8eq). A distinct law from the file-glob set in
/// `document.rs`: this one guards a Redis `SCAN MATCH` pattern and
/// stays here.
pub const REDIS_GLOB_METACHARACTERS: &str = "*?[]\\^";

/// Raw serde shape of the scenario `redis:` action as parsed from the
/// document. Both action fields are optional so an unlink-only item
/// deserializes: a required `del` would fail serde before the owned
/// one-of validation could report the defect with the action index.
/// Field names stay snake_case except where `camelCase` renames them
/// (`unlinkPrefix`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RawRedisDelAction {
    datasource: String,
    del: Option<Vec<String>>,
    unlink_prefix: Option<String>,
}

/// Validated, typed twin of [`RawRedisDelAction`]: `del` becomes
/// `keys`, the literal keys one `DEL` removes.
#[derive(Debug, Clone)]
pub struct RedisDelAction {
    pub datasource: String,
    pub keys: Vec<String>,
}

/// Validated, typed twin of the `unlinkPrefix` form (bd rc-ql8eq):
/// `datasource` plus the still-unresolved `${env:BIND_VAR}SUFFIX`
/// template. The runner resolves the NAME from the harness-minted map
/// at execution time; the parser only proves the grammar.
#[derive(Debug, Clone)]
pub struct UnlinkPrefixAction {
    pub datasource: String,
    pub template: String,
}

/// The validated shape of one raw `redis:` item: exactly one of the
/// two action forms.
#[derive(Debug, Clone)]
pub enum ValidatedRedisAction {
    /// Literal-key delete (`del`).
    Del(RedisDelAction),
    /// Harness-minted namespace prefix delete (`unlinkPrefix`).
    UnlinkPrefix(UnlinkPrefixAction),
}

/// Dispatches one raw `redis:` item to its form validator, owning the
/// one-of rule: exactly one of `del` and `unlinkPrefix` must be
/// present. Returns owned error strings; the `document.rs` hook wraps
/// them into `DocError`.
pub fn validate_redis_action(
    raw: &RawRedisDelAction,
    declared_bind_vars: &[String],
    action_index: usize,
) -> Result<ValidatedRedisAction, String> {
    match (raw.del.as_ref(), raw.unlink_prefix.as_deref()) {
        (Some(_), Some(_)) => Err(one_of_error(action_index)),
        (None, None) => Err(one_of_error(action_index)),
        (Some(_), None) => {
            validate_redis_del_action(raw, action_index).map(ValidatedRedisAction::Del)
        }
        (None, Some(template)) => validate_redis_unlink_prefix(
            &raw.datasource,
            template,
            declared_bind_vars,
            action_index,
        )
        .map(ValidatedRedisAction::UnlinkPrefix),
    }
}

/// The one-of violation, naming both accepted fields.
fn one_of_error(action_index: usize) -> String {
    format!(
        "redis action {action_index}: exactly one of `del` or `unlinkPrefix` must be declared \
         (the one-of law)"
    )
}

/// Validates the raw del shape into the typed action. Returns owned
/// error strings; the `document.rs` hook wraps them into `DocError`.
///
/// The grammar is ungated: every defect here is a document defect,
/// independent of what the current build can execute, so a featureless
/// build reports it before the demand gate.
pub fn validate_redis_del_action(
    raw: &RawRedisDelAction,
    action_index: usize,
) -> Result<RedisDelAction, String> {
    // Identifier law: the datasource is a configured name, never an
    // env-interpolated URL.
    if raw.datasource.is_empty() {
        return Err(format!(
            "redis action {action_index}: `datasource` must not be empty (it names a configured \
             datasource)"
        ));
    }
    let Some(del) = raw.del.as_ref() else {
        return Err(one_of_error(action_index));
    };
    if raw.unlink_prefix.is_some() {
        return Err(one_of_error(action_index));
    }
    if del.is_empty() {
        return Err(format!(
            "redis action {action_index}: `del` must declare at least one key (the empty-list \
             law)"
        ));
    }
    for (i, key) in del.iter().enumerate() {
        if let Some(found) = crate::document::glob_metacharacters_in(key) {
            return Err(format!(
                "redis action {action_index}: key `{key}` (del entry {i}) contains glob \
                 metacharacters `{found}`; `del` takes literal keys only"
            ));
        }
    }
    Ok(RedisDelAction {
        datasource: raw.datasource.clone(),
        keys: del.clone(),
    })
}

/// Validates an `unlinkPrefix` template against the document's declared
/// namespace bindVars (bd rc-ql8eq).
///
/// Grammar: the template starts with `${env:`, the segment up to the
/// FIRST `}` is the NAME, and the remainder is the SUFFIX.
///
/// - NAME must be one of `declared_bind_vars` (namespace bindVars, not
///   document `env:` keys) and must contain no `:` at all, so a default
///   construct such as `${env:ns:-fallback}` is rejected. The SUFFIX is
///   free text and may contain `:-` literally.
/// - No second `${` anywhere: exactly one reference.
/// - The SUFFIX must be free of [`REDIS_GLOB_METACHARACTERS`], so the
///   scan pattern can never widen beyond the namespace.
///
/// Returns the typed [`UnlinkPrefixAction`]; the template is stored
/// unresolved so the runner can read the NAME from the harness-minted
/// map at execution time.
pub fn validate_redis_unlink_prefix(
    datasource: &str,
    raw_template: &str,
    declared_bind_vars: &[String],
    action_index: usize,
) -> Result<UnlinkPrefixAction, String> {
    if datasource.is_empty() {
        return Err(format!(
            "redis action {action_index}: `datasource` must not be empty (it names a configured \
             datasource)"
        ));
    }
    let (name, suffix) = split_unlink_template(raw_template).map_err(|message| {
        format!("redis action {action_index}: `unlinkPrefix` template `{raw_template}`: {message}")
    })?;
    if name.contains(':') {
        return Err(format!(
            "redis action {action_index}: `unlinkPrefix` template `{raw_template}` carries a \
             default construct in the NAME segment; a namespace scope takes no fallback"
        ));
    }
    if suffix.contains("${") {
        return Err(format!(
            "redis action {action_index}: `unlinkPrefix` template `{raw_template}` contains a \
             second `${{` reference; exactly one namespace reference is allowed"
        ));
    }
    if !declared_bind_vars.iter().any(|declared| declared == name) {
        return Err(format!(
            "redis action {action_index}: `unlinkPrefix` template `{raw_template}` names \
             `{name}`, which is not a declared namespace bindVar"
        ));
    }
    if let Some(found) = suffix
        .chars()
        .find(|c| REDIS_GLOB_METACHARACTERS.contains(*c))
    {
        return Err(format!(
            "redis action {action_index}: `unlinkPrefix` suffix `{suffix}` contains the Redis \
             glob metacharacter `{found}`; the suffix must be literal so the scan cannot widen \
             beyond the namespace"
        ));
    }
    Ok(UnlinkPrefixAction {
        datasource: datasource.to_string(),
        template: raw_template.to_string(),
    })
}

/// Splits `${env:NAME}SUFFIX` into `(NAME, SUFFIX)`.
///
/// Shared by load validation and the runner so the two can never
/// disagree about where the NAME ends: the FIRST `}` terminates it.
pub fn split_unlink_template(template: &str) -> Result<(&str, &str), String> {
    let Some(rest) = template.strip_prefix("${env:") else {
        return Err(
            "the template must start with `${env:` and name a declared namespace bindVar"
                .to_string(),
        );
    };
    let Some(close) = rest.find('}') else {
        return Err("the template must close the `${env:` reference with `}`".to_string());
    };
    Ok((&rest[..close], &rest[close + 1..]))
}

/// Escapes every [`REDIS_GLOB_METACHARACTERS`] byte with a backslash,
/// so a literal prefix can never act as a pattern.
#[must_use]
pub fn escape_redis_glob(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        if REDIS_GLOB_METACHARACTERS.contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Builds the `SCAN MATCH` pattern for a literal prefix: the escaped
/// prefix followed by exactly one unescaped `*`.
#[must_use]
pub fn build_scan_pattern(prefix: &str) -> String {
    format!("{}*", escape_redis_glob(prefix))
}

/// Verifies that EVERY key in a scan reply lies under `prefix` (a
/// byte-prefix check). The FIRST violating key aborts the whole reply
/// with its position and value: the caller unlinks nothing from a
/// reply that contains an impostor, so the whole-reply law holds even
/// when `COUNT` returned more than 500 keys.
///
/// # Errors
///
/// Returns `(position, key)` for the first key that does not start
/// with `prefix`.
pub fn classify_scan_reply(keys: &[&str], prefix: &str) -> Result<(), (usize, String)> {
    for (position, key) in keys.iter().enumerate() {
        if !key.starts_with(prefix) {
            return Err((position, (*key).to_string()));
        }
    }
    Ok(())
}

#[cfg(feature = "redis")]
use std::sync::Arc;

#[cfg(feature = "redis")]
use camel_api::datasource::DatasourceCatalog;

/// Deletes every declared literal key through the named datasource's
/// connection in ONE `DEL` (feature `redis`).
///
/// Resolution goes through the booted catalog's steering axis — one
/// datasource name, one connection handle, never a self-constructed
/// connection (the `Redis state assertion` law). `DEL` is idempotent:
/// a missing key is success, so the deleted count is ignored. A driver
/// error is reported with the datasource name and the ADR-0051
/// sanitized driver text; the URL never renders.
#[cfg(feature = "redis")]
pub async fn execute_redis_del(
    catalog: &Arc<dyn DatasourceCatalog>,
    action: &RedisDelAction,
) -> Result<(), String> {
    let name = &action.datasource;
    let (conn, db_url) = crate::steering::resolve_datasource::<redis::aio::MultiplexedConnection>(
        catalog,
        name,
        "redis delete",
    )
    .await?;
    let mut cmd = redis::cmd("DEL");
    for key in &action.keys {
        cmd.arg(key);
    }
    let mut conn = (*conn).clone();
    cmd.query_async::<i64>(&mut conn).await.map_err(|e| {
        format!(
            "datasource '{name}': {}",
            crate::steering::sanitize_db_error(&e.to_string(), &db_url)
        )
    })?;
    Ok(())
}

/// Bounds for one [`execute_redis_unlink_prefix`] run.
///
/// The defaults (10 s, 100 000 keys) are the document-level teardown
/// limits; tests call the executor directly with tighter values. The
/// explicit zero-deadline guard in the executor exists because
/// `tokio::time::timeout(Duration::ZERO, fut)` polls the inner future
/// once before firing, which is not deterministic for network I/O.
#[derive(Debug, Clone)]
pub struct UnlinkLimits {
    /// Wall-clock budget for the whole SCAN/UNLINK loop.
    pub deadline: Duration,
    /// Maximum keys this run may delete; a breach reports the exact
    /// number deleted.
    pub key_cap: u64,
}

impl Default for UnlinkLimits {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(10),
            key_cap: 100_000,
        }
    }
}

/// Resolves a template NAME from the harness-minted namespace map
/// (bd rc-ql8eq). The lookup IS the byte-exact map value: no trimming,
/// no case folding, no prefix match. A missing entry or a value that
/// does not satisfy the `BASE~it~RUN~ORD~` mint grammar fails before
/// any SCAN runs (the defence-in-depth check for directly-constructed
/// documents).
///
/// # Errors
///
/// Returns an owned error naming the bind var when it has no mint or
/// its value is not a harness mint.
pub fn resolve_minted_scope<'a>(
    minted_namespaces: &'a BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str, String> {
    let Some(value) = minted_namespaces.get(name) else {
        return Err(format!(
            "redis unlink-prefix: unresolved namespace bind var `{name}`: the harness minted no \
             value for it, so the action never runs SCAN"
        ));
    };
    if !crate::namespace::is_valid_namespace_value(value) {
        return Err(format!(
            "redis unlink-prefix: namespace value for bind var `{name}` is not a harness mint \
             (`BASE~it~RUN~ORD~`), so the action never runs SCAN"
        ));
    }
    Ok(value.as_str())
}

#[cfg(feature = "redis")]
use std::sync::atomic::{AtomicU64, Ordering};

/// Deletes every key under `prefix` with `SCAN` + `UNLINK` through the
/// named datasource's connection (feature `redis`).
///
/// Namespace-scoping law (bd rc-ql8eq):
///
/// - The `SCAN MATCH` pattern is the escaped literal prefix plus one
///   `*`; no document-authored glob survives.
/// - The ENTIRE scan reply is checked with [`classify_scan_reply`]
///   before any `UNLINK`: one impostor aborts the reply and unlinks
///   nothing from it.
/// - Replies are unlinked in chunks of at most 500 keys. Before each
///   chunk, the remaining cap is applied to the chunk start
///   (`min(key_cap - deleted_so_far, chunk.len())`), so a cap breach
///   reports an exact deleted count and stops immediately.
/// - A zero deadline fails before the first command; any other
///   deadline wraps the whole loop and reports the same
///   deadline-plus-deleted-count reason.
///
/// Diagnostic strings carry the datasource name and the ADR-0051
/// sanitized driver text; the URL never renders.
#[cfg(feature = "redis")]
pub async fn execute_redis_unlink_prefix(
    catalog: &Arc<dyn DatasourceCatalog>,
    datasource: &str,
    prefix: &str,
    limits: &UnlinkLimits,
) -> Result<u64, String> {
    // Explicit zero-deadline guard FIRST: tokio's timeout polls the
    // inner future once at Duration::ZERO, which could issue a SCAN or
    // UNLINK before firing.
    if limits.deadline.is_zero() {
        return Err(deadline_error(0));
    }
    let (conn, db_url) = crate::steering::resolve_datasource::<redis::aio::MultiplexedConnection>(
        catalog,
        datasource,
        "redis unlink-prefix",
    )
    .await?;
    let mut conn = (*conn).clone();
    let pattern = build_scan_pattern(prefix);
    let deleted_so_far = AtomicU64::new(0);
    let loop_future = unlink_scan_loop(
        &mut conn,
        datasource,
        prefix,
        &pattern,
        limits.key_cap,
        &deleted_so_far,
        &db_url,
    );
    match tokio::time::timeout(limits.deadline, loop_future).await {
        Err(_elapsed) => Err(deadline_error(deleted_so_far.load(Ordering::Relaxed))),
        Ok(result) => result,
    }
}

#[cfg(feature = "redis")]
async fn unlink_scan_loop(
    conn: &mut redis::aio::MultiplexedConnection,
    datasource: &str,
    prefix: &str,
    pattern: &str,
    key_cap: u64,
    deleted_so_far: &AtomicU64,
    db_url: &str,
) -> Result<u64, String> {
    let mut cursor: u64 = 0;
    loop {
        let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(500u64)
            .query_async(&mut *conn)
            .await
            .map_err(|error| driver_error(datasource, &error, db_url))?;

        // Whole-reply law: classify BEFORE any UNLINK. A reply with an
        // impostor contributes nothing, whatever its chunking.
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        if let Err((position, key)) = classify_scan_reply(&key_refs, prefix) {
            return Err(format!(
                "redis unlink-prefix: scan reply key `{key}` at position {position} lies outside \
                 the namespace prefix `{prefix}`; nothing from that reply was unlinked; {} keys \
                 deleted so far",
                deleted_so_far.load(Ordering::Relaxed)
            ));
        }

        for chunk in keys.chunks(500) {
            let deleted = deleted_so_far.load(Ordering::Relaxed);
            let allowed = key_cap.saturating_sub(deleted).min(chunk.len() as u64);
            if allowed == 0 {
                return Err(cap_error(key_cap, deleted));
            }
            let mut unlink = redis::cmd("UNLINK");
            for key in &chunk[..allowed as usize] {
                unlink.arg(key);
            }
            unlink
                .query_async::<i64>(&mut *conn)
                .await
                .map_err(|error| driver_error(datasource, &error, db_url))?;
            deleted_so_far.fetch_add(allowed, Ordering::Relaxed);
            if allowed < chunk.len() as u64 {
                return Err(cap_error(key_cap, deleted_so_far.load(Ordering::Relaxed)));
            }
        }

        if next_cursor == 0 {
            break;
        }
        cursor = next_cursor;
    }
    Ok(deleted_so_far.load(Ordering::Relaxed))
}

#[cfg(feature = "redis")]
fn deadline_error(deleted: u64) -> String {
    format!("redis unlink-prefix: deadline elapsed; {deleted} keys deleted so far")
}

#[cfg(feature = "redis")]
fn cap_error(key_cap: u64, deleted: u64) -> String {
    format!(
        "redis unlink-prefix: key cap {key_cap} exhausted; {deleted} keys deleted so far (the \
         validated reply still held keys)"
    )
}

#[cfg(feature = "redis")]
fn driver_error(datasource: &str, error: &redis::RedisError, db_url: &str) -> String {
    format!(
        "redis unlink-prefix: datasource '{datasource}': {}",
        crate::steering::sanitize_db_error(&error.to_string(), db_url)
    )
}
