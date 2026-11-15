//! Server-side script execution support (`command=eval` and `command=fcall`).
//!
//! # Provenance over auto-reload
//!
//! The driver's `redis::Script::invoke_async` hides a `NOSCRIPT` auto-reload:
//! it re-invokes the script inside the same call, which would duplicate
//! effects that the first invocation may already have produced (see
//! redis-1.8.1 `src/script.rs:209`-`227`). This module never uses it. Every
//! invocation instead travels as ONE atomic provenance pair:
//!
//! ```text
//! eval:  MULTI; SCRIPT EXISTS <sha>; EVALSHA <sha> <n> <keys…> <args…>; EXEC
//! fcall: MULTI; FUNCTION LIST;     FCALL <function> <n> <keys…> <args…>; EXEC
//! ```
//!
//! queried with `.atomic().ignore_errors()` and decoded from the raw pair
//! values. The transaction is the provenance guarantee: no other client
//! command (for example `SCRIPT FLUSH` or `FUNCTION FLUSH`) can interleave
//! between the existence observation and the invocation, so the
//! `(present, result)` tuple describes exactly what happened on the server.
//! Pipeline decoding must bypass the default error aggregation (redis-1.8.1
//! `src/pipeline.rs:403`-`425`): an `EXEC` entry error would otherwise replace
//! the whole reply and destroy the tuple.
//!
//! Consequences, for `eval` where presence is `SCRIPT EXISTS <sha>`:
//!
//! - `exists == 1` and a normal reply: the ordinary path.
//! - `exists == 1` and an error reply: the reply is FORGED by the script
//!   (a cached script's invocation never answers `NOSCRIPT` on its own, and a
//!   script may `redis.error_reply('BUSY …')` after side effects). The
//!   invocation ran, its effects stand, and it is never re-invoked.
//! - `exists == 0` and `NOSCRIPT`: genuine non-execution; `SCRIPT LOAD` the
//!   source and re-invoke exactly once through a second pair.
//! - a second `NOSCRIPT`, a failed `SCRIPT LOAD`, or a malformed pair is
//!   terminal.
//!
//! For `fcall` presence is the function NAME inside a `FUNCTION LIST` reply
//! (Redis has no `FUNCTION EXISTS`), and there is no `NOSCRIPT`:
//!
//! - present and a normal reply: the ordinary path.
//! - present and an error reply: FORGED. A registered function cannot miss,
//!   so it ran and returned the error itself; never re-invoked.
//! - absent and a "function not found" reply: genuine non-execution;
//!   `FUNCTION LOAD REPLACE` the library and re-invoke exactly once.
//! - a repeated "function not found", a failed `FUNCTION LOAD`, or a malformed
//!   `FUNCTION LIST` reply is terminal.
//!
//! Because `FCALL` binds by function NAME, a pre-existing function with the
//! same name but different code would otherwise run. The first invocation on
//! an endpoint therefore runs `FUNCTION LOAD REPLACE` with the configured
//! library source ([`ScriptRuntime::init`], one load under concurrent first
//! use); repeat exchanges skip it.
//!
//! All of these terminal paths raise a [`ScriptTerminalError`] marked error,
//! so [`retry_class_script`](crate::transport_error::retry_class_script)
//! classifies them `NeverTransient` without sniffing prose.
//!
//! The two load paths ([`ScriptRuntime::reload`], the first-use
//! `FUNCTION LOAD REPLACE`) also keep the driver's `redis::RedisError` in the
//! chain beneath the marker: the attempt stays terminal, but connection
//! invalidation remains its own track, so a load that died on a dead socket
//! still evicts that connection and the next exchange redials.
//!
//! A transaction that aborts at queue time (`EXECABORT`, for example a
//! `READONLY` from a demoted master) bypasses the markers and keeps the
//! driver's aggregate error. Replay stays terminal, and the aggregate's
//! retained per-command replies still feed the invalidation track.
//!
//! # Trust boundary (ADR-0032)
//!
//! Script text is operator configuration: the inline `script=` param or the
//! `scriptFile=` path. It never comes from an exchange header or body.

use crate::config::RedisEndpointConfig;
use crate::transport_error::redis_error_to_camel;
use crate::transport_error::script_terminal_error;
use camel_component_api::CamelError;
use redis::aio::MultiplexedConnection;
use std::path::Path;
use std::path::PathBuf;
use tokio::sync::OnceCell;

/// The resolved script text and, for `scriptFile`, the path it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSource {
    /// The script (or function library) source, used verbatim.
    pub code: String,
    /// The resolved file the source was read from, for `scriptFile`.
    pub resolved_path: Option<PathBuf>,
}

/// Which server-side script command the runtime drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptKind {
    /// Lua script via `EVALSHA` (`command=eval`).
    Eval,
    /// Redis function via `FCALL` (`command=fcall`).
    Fcall {
        /// The invoked function name (`function=`).
        function: String,
    },
}

/// Everything a script-kind request needs at dispatch time.
#[derive(Debug)]
pub struct ScriptRuntime {
    /// The resolved script text.
    pub source: ScriptSource,
    /// Which script command drives it.
    pub kind: ScriptKind,
    /// First-use initialization latch: the `FUNCTION LOAD REPLACE` that binds
    /// the configured library's function names to the configured code. Shared
    /// across producer clones, so concurrent first use loads exactly once.
    /// Unused for `eval`, whose load is the reactive `NOSCRIPT` reload.
    init: OnceCell<()>,
}

impl ScriptRuntime {
    /// Builds a runtime for `source` driven by `kind`.
    pub fn new(source: ScriptSource, kind: ScriptKind) -> Self {
        Self {
            source,
            kind,
            init: OnceCell::new(),
        }
    }

    /// The script's SHA1, computed the same way the driver does
    /// (redis-1.8.1 `src/script.rs:44`-`46`).
    fn sha(&self) -> String {
        redis::Script::new(&self.source.code).get_hash().to_string()
    }

    /// Runs the script with `keys`/`args` and returns the mapped-ready reply.
    ///
    /// See the module docs for the provenance protocol. Errors are either
    /// transport failures (the caller classifies them) or
    /// [`ScriptTerminalError`]-marked business/terminal failures.
    pub(crate) async fn invoke(
        &self,
        conn: &mut MultiplexedConnection,
        keys: &[String],
        args: &[String],
    ) -> Result<redis::Value, CamelError> {
        match &self.kind {
            ScriptKind::Eval => self.invoke_eval(conn, keys, args).await,
            ScriptKind::Fcall { function } => self.invoke_fcall(conn, function, keys, args).await,
        }
    }

    /// Runs one `EVALSHA` invocation through its `(SCRIPT EXISTS, result)`
    /// pair.
    async fn invoke_eval(
        &self,
        conn: &mut MultiplexedConnection,
        keys: &[String],
        args: &[String],
    ) -> Result<redis::Value, CamelError> {
        let sha = self.sha();
        let (cached, result) = provenance_pair(&sha, conn, keys, args).await?;

        if cached {
            if let Some(reply) = server_reply_error(&result) {
                // The SHA is cached, so this invocation ran: the error is the
                // script's own reply. Never re-invoke it.
                return Err(forged_script_reply_error(server_error_text(reply)));
            }
            return Ok(result);
        }

        // Not cached: the script cannot have run, so the only coherent reply
        // is NOSCRIPT. Anything else is a contradiction worth failing on.
        let Some(reply) = server_reply_error(&result) else {
            return Err(malformed_provenance_reply_error(format!(
                "SCRIPT EXISTS answered 0 but EVALSHA returned a value: {result:?}"
            )));
        };
        if !is_noscript(reply) {
            return Err(script_business_reply_error(reply));
        }

        self.reload(conn, &sha).await?;
        let (still_cached, second) = provenance_pair(&sha, conn, keys, args).await?;
        if let Some(reply) = server_reply_error(&second) {
            if is_noscript(reply) {
                return Err(noscript_reload_exhausted_error(&sha));
            }
            return Err(script_business_reply_error(reply));
        }
        if !still_cached {
            // The reload reported success but the script is still unknown:
            // the pair is contradictory, so the invocation never happened and
            // the request is terminal.
            return Err(noscript_reload_exhausted_error(&sha));
        }
        Ok(second)
    }

    /// Runs one `FCALL` invocation through its `(FUNCTION LIST, result)` pair.
    ///
    /// The first-use library load happens before the pair, so every measured
    /// invocation is a single `FCALL` side effect.
    async fn invoke_fcall(
        &self,
        conn: &mut MultiplexedConnection,
        function: &str,
        keys: &[String],
        args: &[String],
    ) -> Result<redis::Value, CamelError> {
        self.ensure_loaded(conn).await?;
        let (present, result) = fcall_pair(function, conn, keys, args).await?;

        if present {
            if let Some(reply) = server_reply_error(&result) {
                // The function name is registered, so this invocation ran:
                // the error is the function's own reply. Never re-invoke it.
                return Err(forged_function_reply_error(
                    function,
                    server_error_text(reply),
                ));
            }
            return Ok(result);
        }

        // Absent: the function cannot have run, so the only coherent reply is
        // "function not found". Anything else is a contradiction worth
        // failing on.
        let Some(reply) = server_reply_error(&result) else {
            return Err(malformed_fcall_pair_error(format!(
                "FUNCTION LIST does not register '{function}' but FCALL returned a value: {result:?}"
            )));
        };
        if !is_function_not_found(reply) {
            return Err(script_business_reply_error(reply));
        }

        self.load_library(conn).await?;
        let (still_present, second) = fcall_pair(function, conn, keys, args).await?;
        if let Some(reply) = server_reply_error(&second) {
            if is_function_not_found(reply) {
                return Err(function_reload_exhausted_error(function));
            }
            return Err(script_business_reply_error(reply));
        }
        if !still_present {
            // The reload reported success but the function is still absent:
            // the pair is contradictory, so the invocation never happened and
            // the request is terminal.
            return Err(function_reload_exhausted_error(function));
        }
        Ok(second)
    }

    /// `SCRIPT LOAD`s the source so the next `EVALSHA` can run it.
    async fn reload(&self, conn: &mut MultiplexedConnection, sha: &str) -> Result<(), CamelError> {
        redis::cmd("SCRIPT")
            .arg("LOAD")
            .arg(&self.source.code)
            .query_async::<redis::Value>(conn)
            .await
            .map_err(|err| script_load_failed_error(sha, err))?;
        Ok(())
    }

    /// Runs the first-use `FUNCTION LOAD REPLACE`, once per endpoint.
    ///
    /// `REPLACE` is load-bearing: `FCALL` binds by function NAME, so a
    /// pre-existing function with the same name but different code would
    /// otherwise run. The [`OnceCell`] latch makes concurrent first use load
    /// exactly once; a failed load leaves the latch unset so a later attempt
    /// can try again.
    async fn ensure_loaded(&self, conn: &mut MultiplexedConnection) -> Result<(), CamelError> {
        self.init
            .get_or_try_init(|| self.load_library(conn))
            .await?;
        Ok(())
    }

    /// `FUNCTION LOAD REPLACE` the configured library source.
    async fn load_library(&self, conn: &mut MultiplexedConnection) -> Result<(), CamelError> {
        redis::cmd("FUNCTION")
            .arg("LOAD")
            .arg("REPLACE")
            .arg(&self.source.code)
            .query_async::<redis::Value>(conn)
            .await
            .map_err(function_load_failed_error)?;
        Ok(())
    }
}

/// Reads the endpoint's script source: inline `script=` verbatim, or
/// `scriptFile=` from disk.
///
/// A `scriptFile` path resolved by the route loader arrives absolute and is
/// read as-is. A programmatically built config has no route-file context, so a
/// relative path resolves against the process working directory — the
/// documented fallback of the DSL's `ScriptLoadContext`. A read failure names
/// the resolved path.
pub fn load_script_source(cfg: &RedisEndpointConfig) -> Result<ScriptSource, CamelError> {
    match (&cfg.script, &cfg.script_file) {
        (Some(code), None) => Ok(ScriptSource {
            code: code.clone(),
            resolved_path: None,
        }),
        (None, Some(path)) => {
            let resolved = resolve_programmatic_script_path(path);
            let code = std::fs::read_to_string(&resolved).map_err(|err| {
                CamelError::Config(format!(
                    "failed to read scriptFile '{}': {err}",
                    resolved.display()
                ))
            })?;
            Ok(ScriptSource {
                code,
                resolved_path: Some(resolved),
            })
        }
        // `from_uri` fails closed on both of these already; the arms keep the
        // match exhaustive for programmatically built configs.
        (Some(_), Some(_)) => Err(CamelError::Config(
            "script and scriptFile are mutually exclusive: set exactly one script source".into(),
        )),
        (None, None) => Err(CamelError::Config(
            "a script command requires a script source: set exactly one of script (inline source) or scriptFile (path)"
                .into(),
        )),
    }
}

/// The path a programmatically configured `scriptFile` reads from: absolute
/// paths are used as-is, relative ones join the process working directory.
/// When the working directory is unavailable the relative path is kept, so
/// the read still fails with a diagnostic naming what was tried.
fn resolve_programmatic_script_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

/// Runs the atomic provenance pair and decodes `(exists, result)` from the
/// raw `EXEC` entries.
///
/// A queue-time failure inside `MULTI` (for example a `READONLY` from a
/// demoted master) surfaces as the driver's `Server(ExecAbort)` aggregate.
/// The aggregate is forwarded unchanged: redis-rs retains the per-command
/// replies inside it, so [`invalidates_connection`] still sees the demoted
/// master and evicts the connection. The replay bucket stays
/// `NeverTransient`.
///
/// [`invalidates_connection`]: crate::transport_error::invalidates_connection
async fn provenance_pair(
    sha: &str,
    conn: &mut MultiplexedConnection,
    keys: &[String],
    args: &[String],
) -> Result<(bool, redis::Value), CamelError> {
    let mut pipe = redis::pipe();
    // `.ignore_errors()` is load-bearing: without it the default pipeline
    // query aggregates an entry error into a single `Err`, which would
    // destroy the `(exists, result)` tuple.
    pipe.atomic().ignore_errors();
    pipe.cmd("SCRIPT").arg("EXISTS").arg(sha);
    pipe.cmd("EVALSHA")
        .arg(sha)
        .arg(keys.len())
        .arg(keys)
        .arg(args);

    let values = pipe
        .query_async::<Vec<redis::Value>>(conn)
        .await
        .map_err(|err| redis_error_to_camel("EVALSHA", err))?;

    decode_pair(values)
}

/// Runs the atomic FCALL provenance pair and decodes `(present, result)` from
/// the raw `EXEC` entries.
async fn fcall_pair(
    function: &str,
    conn: &mut MultiplexedConnection,
    keys: &[String],
    args: &[String],
) -> Result<(bool, redis::Value), CamelError> {
    let mut pipe = redis::pipe();
    // Same load-bearing `.ignore_errors()` as the eval pair: a `FCALL` entry
    // error must survive as a value, not replace the whole reply.
    pipe.atomic().ignore_errors();
    pipe.cmd("FUNCTION").arg("LIST");
    pipe.cmd("FCALL")
        .arg(function)
        .arg(keys.len())
        .arg(keys)
        .arg(args);

    let values = pipe
        .query_async::<Vec<redis::Value>>(conn)
        .await
        .map_err(|err| redis_error_to_camel("FCALL", err))?;

    decode_function_pair(values, function)
}

/// Decodes the `EXEC` reply of an `FCALL` provenance pair into
/// `(present, result)`.
///
/// `FUNCTION LIST` has no scalar presence flag: the decoder walks the library
/// entries and looks for the function NAME in a library's `functions` list.
fn decode_function_pair(
    mut values: Vec<redis::Value>,
    function: &str,
) -> Result<(bool, redis::Value), CamelError> {
    if values.len() != 2 {
        return Err(malformed_fcall_pair_error(format!(
            "expected the (FUNCTION LIST, FCALL) pair, got {} values",
            values.len()
        )));
    }
    let result = values.remove(1);
    let present = function_present(&values.remove(0), function)?;
    Ok((present, result))
}

/// Whether `list`, a `FUNCTION LIST` reply, registers `function`.
///
/// Redis answers an array of libraries. RESP2 encodes each library (and each
/// function entry) as a flat alternating field/value array; RESP3 uses maps.
/// Both shapes are accepted and anything else is malformed, so a nonsense
/// reply fails closed instead of reading as "absent" and triggering a reload.
fn function_present(list: &redis::Value, function: &str) -> Result<bool, CamelError> {
    let Some(libraries) = reply_elements(list) else {
        return Err(malformed_fcall_pair_error(format!(
            "FUNCTION LIST must answer an array of libraries, got {list:?}"
        )));
    };
    let mut found = false;
    for library in libraries {
        let Some(fields) = reply_fields(library) else {
            return Err(malformed_fcall_pair_error(format!(
                "a FUNCTION LIST library must be a field/value array or map, got {library:?}"
            )));
        };
        let Some(functions) = field_value(&fields, "functions") else {
            return Err(malformed_fcall_pair_error(format!(
                "a FUNCTION LIST library has no 'functions' field: {library:?}"
            )));
        };
        let Some(entries) = reply_elements(functions) else {
            return Err(malformed_fcall_pair_error(format!(
                "the 'functions' field must be an array, got {functions:?}"
            )));
        };
        for entry in entries {
            let Some(entry_fields) = reply_fields(entry) else {
                return Err(malformed_fcall_pair_error(format!(
                    "a FUNCTION LIST function entry must be a field/value array or map, got {entry:?}"
                )));
            };
            let Some(name) = field_value(&entry_fields, "name") else {
                return Err(malformed_fcall_pair_error(format!(
                    "a FUNCTION LIST function entry has no 'name' field: {entry:?}"
                )));
            };
            // A non-text `name` is a malformed reply, never an absent function:
            // reading it as a non-match would trigger a recovery load and a
            // re-invocation on a reply that proves nothing about registration.
            let Some(name) = value_text(name) else {
                return Err(malformed_fcall_pair_error(format!(
                    "a FUNCTION LIST function entry's 'name' must be a text value, got {name:?}: {entry:?}"
                )));
            };
            if name == function {
                found = true;
            }
        }
    }
    Ok(found)
}

/// The value of `field` in a decoded field/value list.
fn field_value<'a>(
    fields: &[(&'a redis::Value, &'a redis::Value)],
    field: &str,
) -> Option<&'a redis::Value> {
    fields
        .iter()
        .find(|(key, _)| value_text(key) == Some(field))
        .map(|(_, value)| *value)
}

/// The elements of an array-shaped reply (`Array`/`Set`), if `value` is one.
fn reply_elements(value: &redis::Value) -> Option<&[redis::Value]> {
    match value {
        redis::Value::Array(items) | redis::Value::Set(items) => Some(items),
        _ => None,
    }
}

/// Decodes a field/value reply entry into pairs.
///
/// RESP2 sends an even-length alternating array; RESP3 sends a map. An odd
/// array length cannot be a field/value list.
fn reply_fields(value: &redis::Value) -> Option<Vec<(&redis::Value, &redis::Value)>> {
    match value {
        redis::Value::Array(items) | redis::Value::Set(items) => {
            if items.len() % 2 != 0 {
                return None;
            }
            Some(
                items
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| (&pair[0], &pair[1]))
                    .collect(),
            )
        }
        redis::Value::Map(entries) => {
            Some(entries.iter().map(|(key, value)| (key, value)).collect())
        }
        _ => None,
    }
}

/// The text of a string-shaped reply value, if it has one.
fn value_text(value: &redis::Value) -> Option<&str> {
    match value {
        redis::Value::BulkString(bytes) => std::str::from_utf8(bytes).ok(),
        redis::Value::SimpleString(text) | redis::Value::VerbatimString { text, .. } => Some(text),
        _ => None,
    }
}

/// Decodes the `EXEC` reply of a provenance pair into `(exists, result)`.
///
/// The existence element keeps the reply shape of the observation command:
/// the driver surfaces each `EXEC` entry verbatim, so the shape here is the
/// one Redis puts on the wire, not a scalar.
///
/// # Reply shapes per target
///
/// - `SCRIPT EXISTS <sha>` (this target) answers a **one-element integer
///   array**, `Array([Int(0|1)])`: the command takes several SHAs and returns
///   one flag per SHA, even for a single argument. Both RESP2 and RESP3 keep
///   it an array. A scalar `Int` is NOT a real reply and stays malformed.
/// - `FUNCTION LIST` (`fcall`) answers a **nested array of library entries**
///   and needs its own decoder: [`decode_function_pair`] inspects the
///   registered function NAMES, not a flag.
fn decode_pair(mut values: Vec<redis::Value>) -> Result<(bool, redis::Value), CamelError> {
    if values.len() != 2 {
        return Err(malformed_provenance_reply_error(format!(
            "expected the (SCRIPT EXISTS, EVALSHA) pair, got {} values",
            values.len()
        )));
    }
    let result = values.remove(1);
    let exists = match values.remove(0) {
        // Real Redis: one flag per requested SHA, so always an array.
        redis::Value::Array(flags) => match flags.as_slice() {
            [redis::Value::Int(1)] => true,
            [redis::Value::Int(0)] => false,
            other => {
                return Err(malformed_provenance_reply_error(format!(
                    "SCRIPT EXISTS must answer a one-element 0-or-1 array, got {other:?}"
                )));
            }
        },
        other => {
            return Err(malformed_provenance_reply_error(format!(
                "SCRIPT EXISTS must answer a one-element 0-or-1 array, got {other:?}"
            )));
        }
    };
    Ok((exists, result))
}

/// The `ServerError` payload of an error reply, if `value` is one.
fn server_reply_error(value: &redis::Value) -> Option<&redis::ServerError> {
    match value {
        redis::Value::ServerError(err) => Some(err),
        _ => None,
    }
}

fn is_noscript(err: &redis::ServerError) -> bool {
    err.code() == "NOSCRIPT"
}

/// True for the `FCALL` reply of an unregistered function name.
///
/// Redis answers `-ERR Function not found`; the classification is by code and
/// detail, never by the rendered message of an arbitrary error.
fn is_function_not_found(err: &redis::ServerError) -> bool {
    err.code() == "ERR"
        && err
            .details()
            .is_some_and(|detail| detail.starts_with("Function not found"))
}

fn server_error_text(err: &redis::ServerError) -> String {
    crate::commands::server_error_text(err)
}

/// The script is cached but its invocation answered an error reply: the
/// script forged the reply (it ran and produced the error itself).
pub(crate) fn forged_script_reply_error(detail: impl Into<String>) -> CamelError {
    script_terminal_error(format!(
        "the script is cached, so its invocation ran and returned this error reply itself ({}); the reply is a business error, its side effects stand, and it is never re-invoked",
        detail.into()
    ))
}

/// A `NOSCRIPT` survived the one allowed reload: the script still is not
/// cached, so the invocation is terminal.
pub(crate) fn noscript_reload_exhausted_error(sha: &str) -> CamelError {
    script_terminal_error(format!(
        "NOSCRIPT after reloading script {sha}: the script is still not cached, so the invocation cannot run and the request is terminal"
    ))
}

/// `SCRIPT LOAD` failed, so the re-invocation never happened.
///
/// The driver error rides in the source chain beneath the terminal marker, so
/// a dead socket still invalidates the cached connection (the attempt itself
/// is never replayed).
pub(crate) fn script_load_failed_error(sha: &str, err: redis::RedisError) -> CamelError {
    let text =
        format!("failed to SCRIPT LOAD script {sha}: {err}; the invocation cannot be retried");
    crate::transport_error::script_terminal_error_with_cause(text, err)
}

/// The provenance pair reply did not have the shape the protocol requires.
fn malformed_provenance_reply_error(detail: impl std::fmt::Display) -> CamelError {
    script_terminal_error(format!("malformed EVALSHA provenance pair: {detail}"))
}

/// The script ran and answered a server error reply that is not `NOSCRIPT`.
pub(crate) fn script_business_reply_error(err: &redis::ServerError) -> CamelError {
    script_terminal_error(format!(
        "the script execution returned a server error reply ({}); the script ran, its side effects stand, and the request is not replayed",
        server_error_text(err)
    ))
}

/// The function is registered but its invocation answered an error reply: the
/// function forged the reply (it ran and produced the error itself).
pub(crate) fn forged_function_reply_error(function: &str, detail: impl Into<String>) -> CamelError {
    script_terminal_error(format!(
        "FUNCTION LIST registers '{function}', so its invocation ran and returned this error reply itself ({}); the reply is a business error, its side effects stand, and it is never re-invoked",
        detail.into()
    ))
}

/// A "function not found" survived the one allowed reload: the function is
/// still unregistered, so the invocation is terminal.
pub(crate) fn function_reload_exhausted_error(function: &str) -> CamelError {
    script_terminal_error(format!(
        "FCALL '{function}' still reports the function missing after FUNCTION LOAD REPLACE: the invocation cannot run and the request is terminal"
    ))
}

/// `FUNCTION LOAD REPLACE` failed, so the re-invocation never happened.
///
/// The driver error rides in the source chain beneath the terminal marker, so
/// a dead socket still invalidates the cached connection. Without it the
/// first-use [`OnceCell`] latch stays unset while the poisoned connection
/// survives, and every later exchange would retry the load against the same
/// dead socket.
pub(crate) fn function_load_failed_error(err: redis::RedisError) -> CamelError {
    let text = format!(
        "failed to FUNCTION LOAD REPLACE the function library: {err}; the invocation cannot be retried"
    );
    crate::transport_error::script_terminal_error_with_cause(text, err)
}

/// The FCALL provenance pair reply did not have the shape the protocol
/// requires.
fn malformed_fcall_pair_error(detail: impl std::fmt::Display) -> CamelError {
    script_terminal_error(format!("malformed FCALL provenance pair: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport_error::RetryClass;
    use crate::transport_error::SendPhase;
    use crate::transport_error::retry_class_script;
    use std::path::Path;

    /// Serializes the tests that mutate the process working directory.
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores the process working directory when dropped.
    struct CwdGuard(std::path::PathBuf);

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    /// Programmatic endpoint construction carries no route-file context, so a
    /// relative `scriptFile` resolves against the process working directory
    /// (the documented `ScriptLoadContext` fallback) and the resolved absolute
    /// path is what the source records.
    #[test]
    fn programmatic_relative_scriptfile_uses_cwd() {
        let _lock = CWD_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("create temp dir");
        std::fs::write(dir.path().join("rel.lua"), "return 'from-cwd'").expect("write rel.lua");

        let previous = std::env::current_dir().expect("read prior cwd");
        let _guard = CwdGuard(previous);
        std::env::set_current_dir(dir.path()).expect("chdir into the temp dir");

        let cfg =
            RedisEndpointConfig::from_uri("redis://localhost:6379?command=eval&scriptFile=rel.lua")
                .expect("valid eval endpoint URI");

        let source = load_script_source(&cfg).expect("the relative path resolves against the cwd");
        assert_eq!(source.code, "return 'from-cwd'", "the CWD file is read");
        let expected = std::env::current_dir()
            .expect("read the current cwd")
            .join(Path::new("rel.lua"));
        assert_eq!(
            source.resolved_path,
            Some(expected),
            "the resolved path is the absolute CWD-joined path"
        );
    }

    /// Parses a real RESP `EXEC` reply into the raw entries the driver hands
    /// to [`decode_pair`] after `ignore_errors()`.
    fn exec_entries(reply: &[u8]) -> Vec<redis::Value> {
        match redis::parse_redis_value(reply).expect("valid RESP EXEC reply") {
            redis::Value::Array(entries) => entries,
            other => panic!("an EXEC reply is always an array, got {other:?}"),
        }
    }

    /// The decoder must match the real wire contract, not the scalar shape
    /// the original fixtures encoded: `SCRIPT EXISTS` returns one flag per
    /// requested SHA, so its `EXEC` entry is an array even for one SHA.
    #[test]
    fn eval_provenance_pair_decodes_real_script_exists_shape() {
        let cached = exec_entries(b"*2\r\n*1\r\n:1\r\n:1\r\n");
        let (exists, result) = decode_pair(cached).expect("the real cached pair must decode");
        assert!(exists, "SCRIPT EXISTS [1] means the script is cached");
        assert_eq!(
            result,
            redis::Value::Int(1),
            "the result entry is untouched"
        );

        let uncached =
            exec_entries(b"*2\r\n*1\r\n:0\r\n-NOSCRIPT No matching script. Please use EVAL.\r\n");
        let (exists, result) = decode_pair(uncached).expect("the real uncached pair must decode");
        assert!(!exists, "SCRIPT EXISTS [0] means the script is not cached");
        assert!(
            server_reply_error(&result).is_some_and(is_noscript),
            "a ServerError result survives the array-shaped existence entry, got: {result:?}"
        );

        // Shapes Redis never sends for SCRIPT EXISTS stay malformed: a scalar
        // flag (the pre-fix fixture shape), a non-0/1 flag, a multi-flag
        // array, an empty array, an error in the existence slot, and the
        // wrong pair arity.
        let malformed: [&[u8]; 7] = [
            b"*2\r\n:1\r\n:1\r\n",
            b"*2\r\n*1\r\n:7\r\n:1\r\n",
            b"*2\r\n*2\r\n:1\r\n:0\r\n:1\r\n",
            b"*2\r\n*0\r\n:1\r\n",
            b"*2\r\n-WRONGTYPE not an array\r\n:1\r\n",
            b"*1\r\n*1\r\n:1\r\n",
            b"*3\r\n*1\r\n:1\r\n:1\r\n:1\r\n",
        ];
        for reply in malformed {
            let err = decode_pair(exec_entries(reply))
                .expect_err("a shape Redis never sends must stay malformed");
            assert!(
                err.to_string()
                    .contains("malformed EVALSHA provenance pair"),
                "the terminal error names the malformed pair, got: {err}"
            );
            assert_eq!(
                retry_class_script(&err, SendPhase::PostFlush),
                RetryClass::NeverTransient,
                "a malformed pair is terminal: {err}"
            );
        }
    }
}
