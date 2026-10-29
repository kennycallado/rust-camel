//! Stream consumer-group I/O seam and failover loop.
//!
//! Mirrors [`crate::queue`] for Redis Streams: [`StreamReadIo`] abstracts the
//! `XREADGROUP`/`XACK` calls and [`StreamRecoveryIo`] abstracts
//! `XAUTOCLAIM`/`XPENDING` recovery, so the reconnect loop can be tested
//! without a broker, while [`RedisStreamIo`] is the real
//! multiplexed-connection factory.
//!
//! Read and recovery each own an independent connection slot (and thus
//! independent connect/retry state): a slow `XREADGROUP BLOCK` must never
//! serialize behind a recovery pass, and a recovery reconnect must not tear
//! down the read connection.
//!
//! Reads run on a deadline-free connection: the `BLOCK` argument of
//! `XREADGROUP` is the only bound on a read, so redis-rs's default response
//! deadline must not cut it short and force a spurious reconnect.

use async_trait::async_trait;
use camel_component_api::Body;
use camel_component_api::CamelError;
use camel_component_api::Exchange;
use camel_component_api::Message;
use camel_component_api::NetworkRetryPolicy;
use futures_util::FutureExt;
use std::any::Any;
use std::collections::VecDeque;
use std::future::Future;
use std::ops::ControlFlow;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::retry::transient_retry_step;
use crate::topology::{RedisTopology, ServerKind};
use crate::transport_error::{
    TransientByProse, TransportTimeout, is_retry_budget_exhausted, marker_camel, redis_error_raw,
};

/// Parameters for one `XREADGROUP` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamConsumerParams {
    /// Stream key to read from.
    pub(crate) key: String,
    /// Consumer-group name.
    pub(crate) group: String,
    /// Consumer name within the group.
    pub(crate) consumer: String,
    /// Optional `COUNT` bound on the number of entries returned.
    pub(crate) count: Option<u64>,
    /// `BLOCK` timeout in milliseconds.
    pub(crate) block_ms: u64,
    /// Whether pending entries are recovered before reading new ones.
    pub(crate) recover_enabled: bool,
    /// Minimum pending idle time in milliseconds before recovery.
    pub(crate) recover_min_idle_ms: u64,
    /// Maximum pending entries claimed per recovery batch.
    pub(crate) recover_batch: u64,
}

/// One stream entry returned by `XREADGROUP`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamEntry {
    /// Entry ID (for example `"1700000000000-0"`).
    pub(crate) id: String,
    /// Field/value pairs carried by the entry.
    pub(crate) fields: Vec<(String, String)>,
    /// Number of times the entry has been delivered to a consumer.
    pub(crate) delivery_count: u64,
}

/// Builds an [`Exchange`] from one stream entry, carrying its delivery
/// metadata.
///
/// The body is the single field's value when the entry has exactly one field.
/// Otherwise the body is a JSON object string that maps every field name to
/// its value, for example `{"a":"1","b":"2"}`.
///
/// Sets the following headers on `exchange.input`:
/// - `CamelRedis.Stream`: the stream key
/// - `CamelRedis.StreamId`: the entry id
/// - `CamelRedis.StreamGroup`: the consumer group
/// - `CamelRedis.StreamConsumer`: the consumer name
/// - `CamelRedis.StreamDeliveryCount`: the delivery count (`"1"` for `>`
///   reads; the claimed count for recovered entries)
pub(crate) fn build_exchange_from_stream_entry(
    stream_key: &str,
    group: &str,
    consumer: &str,
    entry: &StreamEntry,
) -> Exchange {
    let body = if entry.fields.len() == 1 {
        entry.fields[0].1.clone()
    } else {
        // Insert fields in name order so the serialized bytes are always
        // alphabetical, independent of whether `serde_json` is built with the
        // `preserve_order` feature (which an `IndexMap` would otherwise keep in
        // insertion order). Duplicate field names collapse with last-write-wins.
        let mut fields: Vec<&(String, String)> = entry.fields.iter().collect();
        fields.sort_by(|(a, _), (b, _)| a.cmp(b));
        let mut map = serde_json::Map::new();
        for (field, value) in fields {
            map.insert(field.clone(), serde_json::Value::String(value.clone()));
        }
        serde_json::Value::Object(map).to_string()
    };

    let mut exchange = Exchange::new(Message::new(Body::Text(body)));
    let headers = [
        ("CamelRedis.Stream", stream_key.to_string()),
        ("CamelRedis.StreamId", entry.id.clone()),
        ("CamelRedis.StreamGroup", group.to_string()),
        ("CamelRedis.StreamConsumer", consumer.to_string()),
        (
            "CamelRedis.StreamDeliveryCount",
            entry.delivery_count.to_string(),
        ),
    ];
    for (name, value) in headers {
        exchange
            .input
            .set_header(name, serde_json::Value::String(value));
    }

    exchange
}

/// Parses one raw `XREADGROUP` reply into stream entries.
///
/// `Ok(None)` means the reply carried no stream key (a `BLOCK` timeout). A
/// field value that is not valid UTF-8 is a decode failure: it propagates as
/// a non-transient [`CamelError`] instead of being silently replaced by an
/// empty string, because Redis stream fields are binary-safe and a corrupted
/// field must never be acknowledged as delivered.
pub(crate) fn entries_from_reply(
    reply: redis::Value,
) -> Result<Option<Vec<StreamEntry>>, CamelError> {
    let reply: Option<redis::streams::StreamReadReply> =
        redis::from_redis_value(reply).map_err(|err| redis_error_raw(err.into()))?;
    let Some(key) = reply.and_then(|reply| reply.keys.into_iter().next()) else {
        return Ok(None);
    };
    key.ids
        .into_iter()
        .map(entry_from_stream_id)
        .collect::<Result<Vec<_>, CamelError>>()
        .map(Some)
}

/// Maps one decoded stream id to a [`StreamEntry`], propagating invalid-UTF-8
/// field values rather than substituting empty strings.
fn entry_from_stream_id(stream_id: redis::streams::StreamId) -> Result<StreamEntry, CamelError> {
    let redis::streams::StreamId { id, map, .. } = stream_id;
    let fields = map
        .into_iter()
        .map(|(field, value)| {
            let value = <String as redis::FromRedisValue>::from_redis_value(value)
                .map_err(|err| redis_error_raw(err.into()))?;
            Ok((field, value))
        })
        .collect::<Result<Vec<_>, CamelError>>()?;
    Ok(StreamEntry {
        id,
        fields,
        // `>` reads always report a first delivery. Pending-entry reads
        // derive the real count from `XPENDING` instead.
        delivery_count: 1,
    })
}

/// Delivery verdict for one stream entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryOutcome {
    /// The route accepted the entry; it must be acknowledged.
    Completed,
    /// The route rejected the entry; it must not be acknowledged.
    Failed,
    /// The pipeline receiver was dropped; the entry must not be acknowledged
    /// and the session must end with [`CamelError::ChannelClosed`].
    ChannelGone,
}

/// One claimed pending entry plus the delivery count read for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveredEntry {
    /// The claimed stream entry.
    pub entry: StreamEntry,
    /// `XPENDING` delivery count for the entry, when the targeted read
    /// returned a row.
    pub delivery_count: Option<u64>,
}

/// Result of one recovery pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveryBatch {
    /// Aggregate pending count reported by the `XPENDING` summary, when any
    /// pending entry existed.
    pub pending_count: Option<u64>,
    /// Entries claimed by this pass, in claim order.
    pub recovered: Vec<RecoveredEntry>,
    /// Cursor the next pass resumes from.
    pub next_cursor: String,
}

/// Injectable I/O seam for the stream consumer's `XREADGROUP`/`XACK` half.
///
/// Lets the failover loop be tested without a broker: the real impl talks to
/// Redis, a test double records programmable outcomes.
#[async_trait]
pub(crate) trait StreamReadIo: Send {
    /// Establish a multiplexed connection to `client`.
    async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError>;
    /// Read one batch from a consumer group.
    ///
    /// `Ok(None)` means the `BLOCK` timeout elapsed with no entries; the
    /// connection is still healthy.
    async fn read_group(
        &mut self,
        params: &StreamConsumerParams,
    ) -> Result<Option<Vec<StreamEntry>>, CamelError>;
    /// Acknowledge `ids` in `group` for `key`, returning the acknowledged
    /// count.
    async fn ack(&mut self, key: &str, group: &str, ids: &[String]) -> Result<i64, CamelError>;
}

/// Injectable I/O seam for the pending-entry recovery half.
///
/// Owns its own connection so recovery never shares connect/retry state with
/// the blocking read loop.
#[async_trait]
pub(crate) trait StreamRecoveryIo: Send {
    /// Establish an independent multiplexed connection to `client`.
    async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError>;
    /// Run one bounded recovery pass starting from `cursor`, claiming at most
    /// `budget` entries.
    async fn recover(
        &mut self,
        key: &str,
        group: &str,
        consumer: &str,
        min_idle_ms: u64,
        cursor: &str,
        budget: u64,
    ) -> Result<RecoveryBatch, CamelError>;
    /// Acknowledge recovered `ids` in `group` for `key` on the recovery
    /// connection, returning the acknowledged count.
    async fn recover_ack(
        &mut self,
        key: &str,
        group: &str,
        ids: &[String],
    ) -> Result<i64, CamelError>;
}

/// Factory for the two real stream I/O halves.
///
/// Both halves are built from the same connection bounds but each owns an
/// independent connection slot.
pub(crate) struct RedisStreamIo {
    connection_timeout_secs: u64,
}

impl RedisStreamIo {
    /// Create a factory whose connects are bounded by
    /// `connection_timeout_secs`.
    pub(crate) fn new(connection_timeout_secs: u64) -> Self {
        Self {
            connection_timeout_secs,
        }
    }

    /// Build the read half and the recovery half.
    pub(crate) fn halves(self) -> (Box<dyn StreamReadIo>, Box<dyn StreamRecoveryIo>) {
        (
            Box::new(RedisStreamReadIo::new(self.connection_timeout_secs)),
            Box::new(RedisStreamRecoveryIo::new(self.connection_timeout_secs)),
        )
    }
}

/// Open one deadline-free multiplexed connection, bounded by
/// `connection_timeout_secs`.
///
/// Deadline-free keeps `XREADGROUP BLOCK` from being cut short by redis-rs's
/// default response deadline; recovery reuses the same bounds for uniformity.
async fn connect_with_timeout(
    client: &redis::Client,
    connection_timeout_secs: u64,
) -> Result<redis::aio::MultiplexedConnection, CamelError> {
    tokio::time::timeout(
        Duration::from_secs(connection_timeout_secs),
        client.get_multiplexed_async_connection_with_config(
            &redis::AsyncConnectionConfig::new()
                .set_response_timeout(None)
                .set_connection_timeout(None),
        ),
    )
    .await
    .map_err(|_| {
        marker_camel(
            format!("Stream connection timed out after {connection_timeout_secs}s"),
            TransportTimeout {
                stage: "stream connect",
            },
        )
    })?
    .map_err(|e| {
        marker_camel(
            format!("Failed to create connection: {e}"),
            TransientByProse {
                site: "stream connect",
            },
        )
    })
}

fn missing_connection(site: &'static str) -> CamelError {
    marker_camel(
        "Stream connection not established".into(),
        TransientByProse { site },
    )
}

/// Real [`StreamReadIo`] backed by a multiplexed Redis connection.
struct RedisStreamReadIo {
    conn: Option<redis::aio::MultiplexedConnection>,
    connection_timeout_secs: u64,
}

impl RedisStreamReadIo {
    fn new(connection_timeout_secs: u64) -> Self {
        Self {
            conn: None,
            connection_timeout_secs,
        }
    }
}

#[async_trait]
impl StreamReadIo for RedisStreamReadIo {
    async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError> {
        let conn = connect_with_timeout(client, self.connection_timeout_secs).await?;
        self.conn = Some(conn);
        Ok(())
    }

    async fn read_group(
        &mut self,
        params: &StreamConsumerParams,
    ) -> Result<Option<Vec<StreamEntry>>, CamelError> {
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| missing_connection("stream read guard"))?;

        let mut cmd = redis::cmd("XREADGROUP");
        cmd.arg("GROUP")
            .arg(params.group.as_str())
            .arg(params.consumer.as_str());
        if let Some(count) = params.count {
            cmd.arg("COUNT").arg(count);
        }
        cmd.arg("BLOCK")
            .arg(params.block_ms)
            .arg("STREAMS")
            .arg(params.key.as_str())
            .arg(">");

        let reply: redis::Value = cmd.query_async(conn).await.map_err(redis_error_raw)?;
        entries_from_reply(reply)
    }

    async fn ack(&mut self, key: &str, group: &str, ids: &[String]) -> Result<i64, CamelError> {
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| missing_connection("stream ack guard"))?;

        let mut cmd = redis::cmd("XACK");
        cmd.arg(key).arg(group);
        for id in ids {
            cmd.arg(id.as_str());
        }
        cmd.query_async(conn).await.map_err(redis_error_raw)
    }
}

/// Real [`StreamRecoveryIo`] backed by its own multiplexed Redis connection.
struct RedisStreamRecoveryIo {
    conn: Option<redis::aio::MultiplexedConnection>,
    connection_timeout_secs: u64,
}

impl RedisStreamRecoveryIo {
    fn new(connection_timeout_secs: u64) -> Self {
        Self {
            conn: None,
            connection_timeout_secs,
        }
    }
}

#[async_trait]
impl StreamRecoveryIo for RedisStreamRecoveryIo {
    async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError> {
        let conn = connect_with_timeout(client, self.connection_timeout_secs).await?;
        self.conn = Some(conn);
        Ok(())
    }

    async fn recover(
        &mut self,
        key: &str,
        group: &str,
        consumer: &str,
        min_idle_ms: u64,
        cursor: &str,
        budget: u64,
    ) -> Result<RecoveryBatch, CamelError> {
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| missing_connection("stream recovery guard"))?;

        // Aggregate pending summary before claiming. `None` and `Some(0)` both
        // end the pass immediately: no XAUTOCLAIM is issued.
        let summary: redis::Value = redis::cmd("XPENDING")
            .arg(key)
            .arg(group)
            .query_async(&mut *conn)
            .await
            .map_err(redis_error_raw)?;
        let pending_count = pending_summary_gate(&summary)?;
        match pending_count {
            None | Some(0) => {
                return Ok(RecoveryBatch {
                    pending_count,
                    recovered: Vec::new(),
                    next_cursor: cursor.to_string(),
                });
            }
            Some(_) => {}
        }

        let mut driver = ClaimLoopDriver::new(
            key,
            group,
            consumer,
            min_idle_ms,
            cursor.to_string(),
            budget,
        );
        let claim = {
            let mut step = driver.start();
            loop {
                match step {
                    ClaimStep::Cmd(args) => {
                        let reply = query_command_frame(&mut *conn, &args).await?;
                        step = driver.advance(Some(reply));
                    }
                    ClaimStep::Done(result) => break result?,
                }
            }
        };

        // Targeted per-entry reads only after the claim loop finishes.
        let mut recovered = Vec::with_capacity(claim.recovered.len());
        for mut recovered_entry in claim.recovered {
            let id = recovered_entry.entry.id.clone();
            let reply: redis::Value = redis::cmd("XPENDING")
                .arg(key)
                .arg(group)
                .arg("IDLE")
                .arg(0)
                .arg(id.as_str())
                .arg(id.as_str())
                .arg(1)
                .query_async(&mut *conn)
                .await
                .map_err(redis_error_raw)?;
            let delivery_count = parse_targeted_xpending(&reply)?;
            // Delivery gating checks `RecoveredEntry.delivery_count`, so the
            // `1` placeholder in `StreamEntry.delivery_count` never reaches an
            // exchange header: `None` here means skip + warn before delivery.
            if let Some(count) = delivery_count {
                recovered_entry.entry.delivery_count = count;
            }
            recovered_entry.delivery_count = delivery_count;
            recovered.push(recovered_entry);
        }

        Ok(RecoveryBatch {
            pending_count,
            recovered,
            next_cursor: claim.next_cursor,
        })
    }

    async fn recover_ack(
        &mut self,
        key: &str,
        group: &str,
        ids: &[String],
    ) -> Result<i64, CamelError> {
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| missing_connection("stream recovery ack guard"))?;

        let mut cmd = redis::cmd("XACK");
        cmd.arg(key).arg(group);
        for id in ids {
            cmd.arg(id.as_str());
        }
        cmd.query_async(conn).await.map_err(redis_error_raw)
    }
}

/// Issue one raw command frame (`args[0]` is the command name).
async fn query_command_frame(
    conn: &mut redis::aio::MultiplexedConnection,
    args: &[String],
) -> Result<redis::Value, CamelError> {
    let Some((name, rest)) = args.split_first() else {
        return Err(reply_type_error(
            "recovery command frame must name a command",
        ));
    };
    let mut cmd = redis::cmd(name);
    for arg in rest {
        cmd.arg(arg);
    }
    cmd.query_async(conn).await.map_err(redis_error_raw)
}

/// Build an error for a reply whose shape cannot be parsed.
fn reply_type_error(what: &str) -> CamelError {
    redis_error_raw(redis::RedisError::from((
        redis::ErrorKind::UnexpectedReturnType,
        "unexpected stream recovery reply",
        what.to_string(),
    )))
}

fn value_to_string(value: redis::Value) -> Result<String, CamelError> {
    <String as redis::FromRedisValue>::from_redis_value(value)
        .map_err(|err| redis_error_raw(err.into()))
}

fn value_to_u64(value: redis::Value) -> Result<u64, CamelError> {
    <u64 as redis::FromRedisValue>::from_redis_value(value)
        .map_err(|err| redis_error_raw(err.into()))
}

/// Parsed XAUTOCLAIM reply: entries plus the cursor for the next pass.
struct ClaimReply {
    entries: Vec<RecoveredEntry>,
    next_cursor: String,
}

/// Parse one XAUTOCLAIM reply, accepting both RESP2 shapes.
///
/// - 2 elements: `[next-cursor, entries]` (Redis 6.2).
/// - 3 elements: `[next-cursor, entries, deleted-ids]` (later Redis). The
///   deleted-id list is ignored: those entries were removed while pending and
///   must not be delivered.
fn parse_claim_reply(reply: redis::Value) -> Result<ClaimReply, CamelError> {
    let items = match reply {
        redis::Value::Array(items) => items,
        _ => {
            return Err(reply_type_error("XAUTOCLAIM reply must be an array"));
        }
    };
    if items.len() < 2 {
        return Err(reply_type_error(
            "XAUTOCLAIM reply must carry a cursor and entries",
        ));
    }
    let mut items = items.into_iter();
    let Some(cursor) = items.next() else {
        return Err(reply_type_error("XAUTOCLAIM reply must carry a cursor"));
    };
    let Some(entries) = items.next() else {
        return Err(reply_type_error("XAUTOCLAIM reply must carry entries"));
    };
    Ok(ClaimReply {
        next_cursor: value_to_string(cursor)?,
        entries: parse_claimed_entries(entries)?,
    })
}

fn parse_claimed_entries(value: redis::Value) -> Result<Vec<RecoveredEntry>, CamelError> {
    let items = match value {
        redis::Value::Array(items) => items,
        _ => {
            return Err(reply_type_error("XAUTOCLAIM entries must be an array"));
        }
    };
    items
        .into_iter()
        .filter_map(|item| match item {
            // Redis 7.0+ reports entries deleted while pending (trimmed or
            // XDEL'ed) as NIL items inside the entries array. The server has
            // already dropped them from the PEL, so there is nothing to
            // deliver or acknowledge.
            redis::Value::Nil => None,
            other => Some(parse_claimed_entry(other)),
        })
        .collect()
}

fn parse_claimed_entry(value: redis::Value) -> Result<RecoveredEntry, CamelError> {
    let parts = match value {
        redis::Value::Array(parts) => parts,
        _ => {
            return Err(reply_type_error("XAUTOCLAIM entry must be an array"));
        }
    };
    if parts.len() != 2 {
        return Err(reply_type_error(
            "XAUTOCLAIM entry must carry an id and field values",
        ));
    }
    let mut parts = parts.into_iter();
    let Some(id) = parts.next() else {
        return Err(reply_type_error("XAUTOCLAIM entry must carry an id"));
    };
    let Some(fields) = parts.next() else {
        return Err(reply_type_error("XAUTOCLAIM entry must carry field values"));
    };
    Ok(RecoveredEntry {
        entry: StreamEntry {
            id: value_to_string(id)?,
            fields: parse_flat_fields(fields)?,
            // Filled from the targeted `XPENDING` read after the claim loop.
            delivery_count: 1,
        },
        delivery_count: None,
    })
}

/// Parse XAUTOCLAIM's flat `[field, value, field, value, ...]` list.
fn parse_flat_fields(value: redis::Value) -> Result<Vec<(String, String)>, CamelError> {
    let items = match value {
        redis::Value::Array(items) => items,
        _ => {
            return Err(reply_type_error("XAUTOCLAIM field values must be an array"));
        }
    };
    if items.len() % 2 != 0 {
        return Err(reply_type_error(
            "XAUTOCLAIM field values must pair every field with a value",
        ));
    }
    let mut fields = Vec::with_capacity(items.len() / 2);
    let mut items = items.into_iter();
    while let Some(field) = items.next() {
        let Some(value) = items.next() else {
            return Err(reply_type_error(
                "XAUTOCLAIM field values must pair every field with a value",
            ));
        };
        fields.push((value_to_string(field)?, value_to_string(value)?));
    }
    Ok(fields)
}

/// Classify the XAUTOCLAIM cursor from an aggregate `XPENDING` summary.
///
/// `Ok(None)` (nil/empty reply) and `Ok(Some(0))` (zero pending) both mean the
/// pass ends without claiming. `Ok(Some(n))` with `n > 0` allows a claim
/// loop. A malformed reply is an error.
pub(crate) fn pending_summary_gate(reply: &redis::Value) -> Result<Option<u64>, CamelError> {
    match reply {
        redis::Value::Nil => Ok(None),
        redis::Value::Array(items) if items.is_empty() => Ok(None),
        redis::Value::Array(items) => {
            let Some(count) = items.first() else {
                return Ok(None);
            };
            Ok(Some(value_to_u64(count.clone())?))
        }
        _ => Err(reply_type_error("XPENDING summary must be an array or nil")),
    }
}

/// Parse a targeted `XPENDING <key> <group> IDLE 0 <id> <id> 1` reply.
///
/// A one-row reply yields the delivery count from the fourth column; an empty
/// (or nil) reply yields `None`.
fn parse_targeted_xpending(reply: &redis::Value) -> Result<Option<u64>, CamelError> {
    match reply {
        redis::Value::Nil => Ok(None),
        redis::Value::Array(rows) if rows.is_empty() => Ok(None),
        redis::Value::Array(rows) => {
            let Some(row) = rows.first() else {
                return Ok(None);
            };
            let columns = match row {
                redis::Value::Array(columns) if columns.len() >= 4 => columns,
                _ => {
                    return Err(reply_type_error(
                        "targeted XPENDING row must carry four columns",
                    ));
                }
            };
            let Some(delivery_count) = columns.get(3) else {
                return Err(reply_type_error(
                    "targeted XPENDING row must carry a delivery count",
                ));
            };
            Ok(Some(value_to_u64(delivery_count.clone())?))
        }
        _ => Err(reply_type_error(
            "targeted XPENDING reply must be an array or nil",
        )),
    }
}

/// Step produced by the resumable XAUTOCLAIM driver.
#[derive(Debug)]
pub(crate) enum ClaimStep {
    /// Write this command frame (`args[0]` is the command name), then feed the
    /// reply back through [`ClaimLoopDriver::advance`].
    Cmd(Vec<String>),
    /// The loop finished: either the claimed entries or a parse/classification
    /// error.
    Done(Result<ClaimLoopResult, CamelError>),
}

/// Final state of a completed claim loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimLoopResult {
    /// Entries claimed in this loop, in claim order.
    pub recovered: Vec<RecoveredEntry>,
    /// Cursor the next pass resumes from.
    pub next_cursor: String,
    /// Number of XAUTOCLAIM frames issued by this loop.
    pub commands_issued: u64,
}

/// Mutable progress of the claim loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimLoopState {
    /// Current resume cursor.
    pub cursor: String,
    /// Entry budget not yet claimed.
    pub remaining: u64,
    /// XAUTOCLAIM frames issued so far.
    pub commands_issued: u64,
    /// Entries claimed so far.
    pub recovered: Vec<RecoveredEntry>,
}

/// Resumable XAUTOCLAIM driver shared by production and unit tests.
///
/// `start`/`advance(None)` issue the first frame; each subsequent
/// `advance(Some(reply))` consumes the reply and either issues the next frame
/// or terminates. The loop stops when the cursor reaches `0-0`, the entry
/// budget is exhausted, or `budget` command frames were issued.
pub(crate) struct ClaimLoopDriver {
    key: String,
    group: String,
    consumer: String,
    min_idle_ms: u64,
    budget: u64,
    state: ClaimLoopState,
}

impl ClaimLoopDriver {
    /// Create a driver resuming from `cursor` with an entry budget of
    /// `budget` (also the command-frame cap).
    pub(crate) fn new(
        key: &str,
        group: &str,
        consumer: &str,
        min_idle_ms: u64,
        cursor: String,
        budget: u64,
    ) -> Self {
        Self {
            key: key.to_string(),
            group: group.to_string(),
            consumer: consumer.to_string(),
            min_idle_ms,
            budget,
            state: ClaimLoopState {
                cursor,
                remaining: budget,
                commands_issued: 0,
                recovered: Vec::new(),
            },
        }
    }

    /// Start the loop by issuing the first frame (or finishing immediately
    /// when the budget is zero).
    pub(crate) fn start(&mut self) -> ClaimStep {
        self.advance(None)
    }

    /// Consume `reply` (if any) and produce the next step.
    pub(crate) fn advance(&mut self, reply: Option<redis::Value>) -> ClaimStep {
        if self.state.commands_issued == 0 {
            if reply.is_some() {
                return self.finish_err(reply_type_error(
                    "claim loop received a reply before issuing a command",
                ));
            }
            return self.issue_next();
        }

        let Some(reply) = reply else {
            return self.finish_err(reply_type_error(
                "claim loop advance requires the issued command's reply",
            ));
        };

        let parsed = match parse_claim_reply(reply) {
            Ok(parsed) => parsed,
            Err(err) => return self.finish_err(err),
        };
        self.state.cursor = parsed.next_cursor;
        self.state.remaining = self
            .state
            .remaining
            .saturating_sub(parsed.entries.len() as u64);
        self.state.recovered.extend(parsed.entries);

        if self.state.cursor == "0-0"
            || self.state.remaining == 0
            || self.state.commands_issued >= self.budget
        {
            self.finish_ok()
        } else {
            self.issue_next()
        }
    }

    fn issue_next(&mut self) -> ClaimStep {
        if self.state.remaining == 0 || self.state.commands_issued >= self.budget {
            return self.finish_ok();
        }
        self.state.commands_issued += 1;
        ClaimStep::Cmd(vec![
            "XAUTOCLAIM".to_string(),
            self.key.clone(),
            self.group.clone(),
            self.consumer.clone(),
            self.min_idle_ms.to_string(),
            self.state.cursor.clone(),
            "COUNT".to_string(),
            self.state.remaining.to_string(),
        ])
    }

    fn finish_ok(&mut self) -> ClaimStep {
        ClaimStep::Done(Ok(ClaimLoopResult {
            recovered: std::mem::take(&mut self.state.recovered),
            next_cursor: self.state.cursor.clone(),
            commands_issued: self.state.commands_issued,
        }))
    }

    fn finish_err(&mut self, err: CamelError) -> ClaimStep {
        ClaimStep::Done(Err(err))
    }
}

/// Capacity of both channels between the session and its read task.
///
/// The protocol keeps at most one batch and its ack/batch-done commands in
/// flight, so a small buffer is ample and keeps the handoff bounded.
const READ_TASK_CHANNEL_CAPACITY: usize = 8;

/// Control message sent by the read task to the session.
enum ReadTaskMsg {
    /// The read half established its first connection.
    Connected,
    /// One readable batch of entries.
    Batch(Vec<StreamEntry>),
    /// The `BLOCK` timeout elapsed; the connection is still healthy.
    NilTick,
    /// The read half cannot continue; the session must fail supervision.
    ReadFailed(CamelError),
}

/// Control command sent by the session to the read task.
enum ReadTaskCmd {
    /// Acknowledge `ids`; the task replies with the `XACK` outcome.
    Ack {
        /// Entry ids to acknowledge.
        ids: Vec<String>,
        /// Reply channel for the `XACK` outcome.
        reply: oneshot::Sender<Result<i64, CamelError>>,
    },
    /// The session settled the current read outcome; the task may read again.
    BatchDone,
}

/// Aborts the wrapped read-task handle on drop.
///
/// Cancellation, a terminal session error, and a dropped session future all
/// drop this guard, which aborts the reader and closes the read socket
/// promptly. The handle is never awaited, so a blocked read cannot hang
/// teardown.
struct AbortOnDrop {
    handle: JoinHandle<()>,
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Builds the terminal error for an unexpectedly ended read task.
fn reader_channel_closed() -> CamelError {
    CamelError::ProcessorError("stream read task channel closed unexpectedly".into())
}

/// Classifies a closed/ended read-task channel: an orderly cancellation is
/// `Ok`, anything else is the terminal supervision error.
fn reader_gone(cancel: &CancellationToken) -> Result<(), CamelError> {
    if cancel.is_cancelled() {
        Ok(())
    } else {
        Err(reader_channel_closed())
    }
}

/// Converts a caught read-task panic payload into a terminal error.
fn reader_panic_error(payload: Box<dyn Any + Send>) -> CamelError {
    let message = if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic payload".to_string()
    };
    CamelError::ProcessorError(format!("stream read task panicked: {message}"))
}

/// Next item observed while waiting on the session→task command channel.
enum NextCmd {
    /// A command arrived.
    Command(ReadTaskCmd),
    /// The session dropped its sender; the task must stop.
    Closed,
    /// The cancellation token fired.
    Cancelled,
}

/// Waits for the next command or cancellation.
async fn next_cmd(rx: &mut mpsc::Receiver<ReadTaskCmd>, cancel: &CancellationToken) -> NextCmd {
    tokio::select! {
        _ = cancel.cancelled() => NextCmd::Cancelled,
        command = rx.recv() => match command {
            Some(command) => NextCmd::Command(command),
            None => NextCmd::Closed,
        },
    }
}

/// Outcome of the read half's reconnect attempt while settling an ack.
enum AckReconnect {
    /// The read half reconnected; the ack outcome stays unknown.
    Ready,
    /// Cancellation fired during the reconnect.
    Cancelled,
    /// The reconnect failed terminally (non-transient or budget exhausted).
    Failed(CamelError),
}

/// Re-establishes the read half after a transient ack failure, sharing the
/// read task's retry budget.
async fn reconnect_after_ack(
    read: &mut dyn StreamReadIo,
    topology: &dyn RedisTopology,
    policy: &NetworkRetryPolicy,
    attempt: &mut u32,
    cancel: &CancellationToken,
) -> AckReconnect {
    loop {
        if cancel.is_cancelled() {
            return AckReconnect::Cancelled;
        }
        let client = match topology.resolve(ServerKind::Master).await {
            Ok(client) => client,
            Err(e) => {
                match transient_retry_step(policy, attempt, e, "resolving master for stream ack")
                    .await
                {
                    ControlFlow::Continue(()) => continue,
                    ControlFlow::Break(e) => return AckReconnect::Failed(e),
                }
            }
        };
        match read.connect(&client).await {
            Ok(()) => return AckReconnect::Ready,
            Err(e) => {
                match transient_retry_step(policy, attempt, e, "reconnecting for stream ack").await
                {
                    ControlFlow::Continue(()) => continue,
                    ControlFlow::Break(e) => return AckReconnect::Failed(e),
                }
            }
        }
    }
}

/// Outcome of settling one `Ack` command.
enum AckDispatch {
    /// The ack was answered (possibly as unknown after a reconnect); keep
    /// settling the remaining commands.
    Continue,
    /// The task must stop; the reply (if any) was already sent.
    Stop,
}

/// Executes one `XACK` and answers its reply channel.
///
/// A transient failure reconnects the read half under the task's budget and
/// replies with the original transient error: the session invalidates the ids
/// as unknown (only recovery may redeliver) without retrying the ack. A
/// non-transient failure or budget exhaustion replies with the terminal error.
#[allow(clippy::too_many_arguments)]
async fn settle_ack(
    read: &mut dyn StreamReadIo,
    topology: &dyn RedisTopology,
    policy: &NetworkRetryPolicy,
    attempt: &mut u32,
    cancel: &CancellationToken,
    key: &str,
    group: &str,
    ids: Vec<String>,
    reply: oneshot::Sender<Result<i64, CamelError>>,
) -> AckDispatch {
    match read.ack(key, group, &ids).await {
        Ok(count) => {
            let _ = reply.send(Ok(count));
            AckDispatch::Continue
        }
        Err(err) => {
            if !crate::transport_error::is_transient_redis_error(&err) {
                let _ = reply.send(Err(err));
                return AckDispatch::Stop;
            }
            match reconnect_after_ack(read, topology, policy, attempt, cancel).await {
                AckReconnect::Ready => {
                    let _ = reply.send(Err(err));
                    AckDispatch::Continue
                }
                AckReconnect::Cancelled => AckDispatch::Stop,
                AckReconnect::Failed(fatal) => {
                    let _ = reply.send(Err(fatal));
                    AckDispatch::Stop
                }
            }
        }
    }
}

/// Sends a terminal read failure to the session, ignoring a closed channel.
async fn send_read_failed(msg_tx: &mpsc::Sender<ReadTaskMsg>, err: CamelError) {
    let _ = msg_tx.send(ReadTaskMsg::ReadFailed(err)).await;
}

/// Owns the read half and services the session's control commands.
///
/// Start handshake: after the first successful connect the task sends
/// [`ReadTaskMsg::Connected`] and waits for one [`ReadTaskCmd::BatchDone`]
/// before its first read. Afterwards it issues exactly one `XREADGROUP` read at
/// a time, sends the outcome, and settles every queued command up to the next
/// `BatchDone` before reading again. That ordering guarantees an `XACK` can
/// never queue behind the next blocked read.
async fn read_task_loop(
    mut read: Box<dyn StreamReadIo>,
    mut cmd_rx: mpsc::Receiver<ReadTaskCmd>,
    msg_tx: mpsc::Sender<ReadTaskMsg>,
    topology: Arc<dyn RedisTopology>,
    params: StreamConsumerParams,
    policy: NetworkRetryPolicy,
    cancel: CancellationToken,
) {
    let key = params.key.clone();
    let group = params.group.clone();
    let mut attempt: u32 = 0;
    let mut started = false;

    'session: loop {
        if cancel.is_cancelled() {
            return;
        }

        let client = match topology.resolve(ServerKind::Master).await {
            Ok(client) => client,
            Err(e) => {
                match transient_retry_step(
                    &policy,
                    &mut attempt,
                    e,
                    "resolving master for XREADGROUP",
                )
                .await
                {
                    ControlFlow::Continue(()) => continue 'session,
                    ControlFlow::Break(e) => {
                        send_read_failed(&msg_tx, e).await;
                        return;
                    }
                }
            }
        };
        if let Err(e) = read.connect(&client).await {
            match transient_retry_step(&policy, &mut attempt, e, "connecting for XREADGROUP").await
            {
                ControlFlow::Continue(()) => continue 'session,
                ControlFlow::Break(e) => {
                    send_read_failed(&msg_tx, e).await;
                    return;
                }
            }
        }

        if !started {
            started = true;
            if msg_tx.send(ReadTaskMsg::Connected).await.is_err() {
                return;
            }
            match next_cmd(&mut cmd_rx, &cancel).await {
                NextCmd::Command(ReadTaskCmd::BatchDone) => {}
                NextCmd::Command(ReadTaskCmd::Ack { reply, .. }) => {
                    // No read happened yet, so an ack here is a protocol
                    // violation. Answer non-transiently so the session fails
                    // supervision instead of hanging.
                    let _ = reply.send(Err(CamelError::ProcessorError(
                        "stream read task received an ack before the start signal".into(),
                    )));
                    return;
                }
                NextCmd::Closed | NextCmd::Cancelled => return,
            }
        }

        // Stay on this connection across successful reads. Only a transient
        // error falls back to the reconnect at the top of `'session`.
        loop {
            let read_result = tokio::select! {
                _ = cancel.cancelled() => return,
                result = read.read_group(&params) => result,
            };

            match read_result {
                Ok(Some(entries)) => {
                    if msg_tx.send(ReadTaskMsg::Batch(entries)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    if msg_tx.send(ReadTaskMsg::NilTick).await.is_err() {
                        return;
                    }
                }
                Err(e) => {
                    match transient_retry_step(&policy, &mut attempt, e, "reading with XREADGROUP")
                        .await
                    {
                        ControlFlow::Continue(()) => break, // → reconnect
                        ControlFlow::Break(e) => {
                            send_read_failed(&msg_tx, e).await;
                            return;
                        }
                    }
                }
            }

            // Settle exactly the commands queued for this outcome, then read
            // again. The session sends every ack before the batch-done signal.
            loop {
                match next_cmd(&mut cmd_rx, &cancel).await {
                    NextCmd::Command(ReadTaskCmd::Ack { ids, reply }) => {
                        let dispatch = settle_ack(
                            &mut *read,
                            topology.as_ref(),
                            &policy,
                            &mut attempt,
                            &cancel,
                            &key,
                            &group,
                            ids,
                            reply,
                        )
                        .await;
                        if matches!(dispatch, AckDispatch::Stop) {
                            return;
                        }
                    }
                    NextCmd::Command(ReadTaskCmd::BatchDone) => break,
                    NextCmd::Closed | NextCmd::Cancelled => return,
                }
            }
        }
    }
}

/// Waits for the in-flight delivery future, if any.
async fn await_in_flight<F: Future>(slot: Option<&mut (StreamEntry, Pin<Box<F>>)>) -> F::Output {
    match slot {
        Some((_, future)) => future.as_mut().await,
        None => std::future::pending().await,
    }
}

/// Run a whole consumer-group session on the read connection.
///
/// Spawns a dedicated read task that owns the read half and performs
/// `XREADGROUP`/`XACK` on it, then drives [`session_core`] from the messages
/// that task produces. The read task is wrapped in an abort-on-drop guard so
/// cancellation, a terminal error, and a dropped session future all end it and
/// close the read socket promptly. A reader panic surfaces as a terminal
/// outcome instead of a detached task.
///
/// The session delivers every entry through `deliver`. An entry whose delivery
/// returns [`DeliveryOutcome::Completed`] is acknowledged in one batch `XACK` per
/// read batch; a [`DeliveryOutcome::Failed`] entry is skipped without an ack. A
/// [`DeliveryOutcome::ChannelGone`] entry ends the session immediately with
/// [`CamelError::ChannelClosed`], without an ack. A `BLOCK` timeout (`Ok(None)`)
/// is normal and keeps the connection.
///
/// On a transient error (connection refused/EOF/timeout/reset/readonly) at any
/// stage the read task re-resolves the master and reconnects, bounded by
/// `policy`; each such failure consumes one attempt. A non-transient error, or
/// budget exhaustion, ends the session with `Err` so Route supervision fires
/// (ADR-0007).
///
/// `on_ready` fires exactly once, after the first successful read connect.
///
/// The recovery half owns an independent connection and retry state. When
/// recovery is enabled, the session runs one pass immediately after `on_ready`
/// (before the reader's first `XREADGROUP`) and then once per
/// `recover_min_idle_ms` deadline. A recovery reconnect never tears down the
/// read connection.
// The seam contract pins these eight inputs (two owned IO halves, topology,
// params, policy, cancellation, readiness, delivery); grouping them into a
// struct would hide the ownership split this task exists to enforce.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_session<D, F>(
    read: Box<dyn StreamReadIo>,
    recovery: Box<dyn StreamRecoveryIo>,
    topology: Arc<dyn RedisTopology>,
    params: StreamConsumerParams,
    policy: NetworkRetryPolicy,
    cancel: CancellationToken,
    on_ready: Option<Box<dyn FnOnce() + Send>>,
    deliver: D,
) -> Result<(), CamelError>
where
    D: FnMut(StreamEntry) -> F,
    F: Future<Output = DeliveryOutcome>,
{
    let (msg_tx, msg_rx) = mpsc::channel(READ_TASK_CHANNEL_CAPACITY);
    let (cmd_tx, cmd_rx) = mpsc::channel(READ_TASK_CHANNEL_CAPACITY);
    let panic_tx = msg_tx.clone();
    let read_topology = Arc::clone(&topology);
    let read_params = params.clone();
    let read_policy = policy.clone();
    let read_cancel = cancel.clone();

    let handle = tokio::spawn(async move {
        let outcome = AssertUnwindSafe(read_task_loop(
            read,
            cmd_rx,
            msg_tx,
            read_topology,
            read_params,
            read_policy,
            read_cancel,
        ))
        .catch_unwind()
        .await;
        if let Err(payload) = outcome {
            let _ = panic_tx
                .send(ReadTaskMsg::ReadFailed(reader_panic_error(payload)))
                .await;
        }
    });
    let _guard = AbortOnDrop { handle };

    session_core(
        msg_rx, cmd_tx, recovery, topology, params, policy, cancel, on_ready, deliver,
    )
    .await
}

/// Reconnects the recovery half after a transient failure.
///
/// Mirrors the read side: each transient failure consumes one attempt of the
/// recovery half's own budget. Returns `Ok(false)` when cancellation wins
/// before the connection is re-established.
async fn connect_recovery(
    recovery: &mut dyn StreamRecoveryIo,
    topology: &dyn RedisTopology,
    policy: &NetworkRetryPolicy,
    attempt: &mut u32,
    cancel: &CancellationToken,
) -> Result<bool, CamelError> {
    loop {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let client = match topology.resolve(ServerKind::Master).await {
            Ok(client) => client,
            Err(e) => {
                match transient_retry_step(
                    policy,
                    attempt,
                    e,
                    "resolving master for stream recovery",
                )
                .await
                {
                    ControlFlow::Continue(()) => continue,
                    ControlFlow::Break(e) => return Err(e),
                }
            }
        };
        match recovery.connect(&client).await {
            Ok(()) => return Ok(true),
            Err(e) => {
                match transient_retry_step(policy, attempt, e, "connecting for stream recovery")
                    .await
                {
                    ControlFlow::Continue(()) => continue,
                    ControlFlow::Break(e) => return Err(e),
                }
            }
        }
    }
}

/// Outcome of one recovery pass attempt.
enum RecoveryPass {
    /// The pass completed; any claimed entries were appended to the queue.
    Completed,
    /// Cancellation was observed before the pass produced a verdict.
    Cancelled,
}

/// Runs one recovery pass on the recovery half and enqueues its claimed
/// entries.
///
/// `last_pass` is stamped at pass start, before the connect or `recover`
/// call, so the next deadline is measured from when the pass began. A
/// transient failure reconnects the recovery half under its own retry budget
/// and retries the pass from the same cursor. A non-transient failure (for
/// example `NOGROUP`) returns `Err` so supervision fires without retrying.
#[allow(clippy::too_many_arguments)]
async fn recovery_pass(
    recovery: &mut dyn StreamRecoveryIo,
    topology: &dyn RedisTopology,
    policy: &NetworkRetryPolicy,
    params: &StreamConsumerParams,
    attempt: &mut u32,
    connected: &mut bool,
    cursor: &mut String,
    last_pass: &mut Option<tokio::time::Instant>,
    queue: &mut VecDeque<RecoveredEntry>,
    cancel: &CancellationToken,
) -> Result<RecoveryPass, CamelError> {
    *last_pass = Some(tokio::time::Instant::now());
    loop {
        if cancel.is_cancelled() {
            return Ok(RecoveryPass::Cancelled);
        }
        if !*connected {
            if !connect_recovery(recovery, topology, policy, attempt, cancel).await? {
                return Ok(RecoveryPass::Cancelled);
            }
            *connected = true;
        }

        let result = {
            let recover = recovery.recover(
                &params.key,
                &params.group,
                &params.consumer,
                params.recover_min_idle_ms,
                cursor,
                params.recover_batch,
            );
            tokio::pin!(recover);
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(RecoveryPass::Cancelled),
                reply = &mut recover => reply,
            }
        };

        match result {
            Ok(batch) => {
                // Log the inner pending count, not `Option`: `pending = 2`
                // when the server reported one, `pending = -` otherwise.
                let pending = batch
                    .pending_count
                    .map_or_else(|| "-".to_string(), |count| count.to_string());
                info!(
                    stream = %params.key,
                    group = %params.group,
                    pending = %pending,
                    claimed = batch.recovered.len(),
                    "Stream recovery pass completed"
                );
                *cursor = batch.next_cursor.clone();
                queue.extend(batch.recovered);
                return Ok(RecoveryPass::Completed);
            }
            Err(err) => {
                // The connection is suspect after any recovery failure.
                *connected = false;
                match transient_retry_step(
                    policy,
                    attempt,
                    err,
                    "recovering pending stream entries",
                )
                .await
                {
                    ControlFlow::Continue(()) => continue,
                    ControlFlow::Break(err) => return Err(err),
                }
            }
        }
    }
}

/// Outcome of waiting on a session future while the recovery deadline may run.
enum Waited<T> {
    /// The awaited future completed.
    Ready(T),
    /// Cancellation was observed before the future completed.
    Cancelled,
    /// A recovery deadline pass completed; the caller decides what to do.
    PassCompleted,
}

/// Waits for `future` while servicing the recovery deadline.
///
/// `future` is re-polled across deadline passes (it is never dropped or
/// restarted while the caller retains it), so an in-flight delivery, a
/// read-side ack, or a message receive keeps its progress. Cancellation is
/// polled first, so a completion that races a cancel cannot turn into an ack.
#[allow(clippy::too_many_arguments)]
async fn wait_with_recovery<T, Fut>(
    future: &mut Pin<Box<Fut>>,
    recovery: &mut dyn StreamRecoveryIo,
    topology: &dyn RedisTopology,
    policy: &NetworkRetryPolicy,
    params: &StreamConsumerParams,
    attempt: &mut u32,
    connected: &mut bool,
    cursor: &mut String,
    last_pass: &mut Option<tokio::time::Instant>,
    queue: &mut VecDeque<RecoveredEntry>,
    cancel: &CancellationToken,
) -> Result<Waited<T>, CamelError>
where
    Fut: Future<Output = T>,
{
    if cancel.is_cancelled() {
        return Ok(Waited::Cancelled);
    }
    let deadline = if params.recover_enabled {
        last_pass.map(|started| started + Duration::from_millis(params.recover_min_idle_ms))
    } else {
        None
    };
    match deadline {
        Some(deadline) => tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(Waited::Cancelled),
            _ = tokio::time::sleep_until(deadline) => {
                match recovery_pass(
                    recovery,
                    topology,
                    policy,
                    params,
                    attempt,
                    connected,
                    cursor,
                    last_pass,
                    queue,
                    cancel,
                )
                .await?
                {
                    RecoveryPass::Completed => Ok(Waited::PassCompleted),
                    RecoveryPass::Cancelled => Ok(Waited::Cancelled),
                }
            }
            outcome = future.as_mut() => Ok(Waited::Ready(outcome)),
        },
        None => tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(Waited::Cancelled),
            outcome = future.as_mut() => Ok(Waited::Ready(outcome)),
        },
    }
}

/// Delivers the queued recovered entries one at a time and acknowledges each
/// accepted entry on the recovery half.
///
/// A claimed entry without a delivery count is skipped with a `warn!` (it
/// cannot be represented faithfully in the exchange header, so it is neither
/// delivered nor acknowledged). A [`DeliveryOutcome::Completed`] entry is
/// acknowledged only when cancellation was not observed at the ack decision
/// point; [`DeliveryOutcome::Failed`] leaves it pending and continues with the
/// next claimed entry; [`DeliveryOutcome::ChannelGone`] ends the session.
///
/// The route owns the operational signal for delivery failures (warn log and
/// error metric live in the consumer's `deliver` closure); the session only
/// sees [`DeliveryOutcome`].
#[allow(clippy::too_many_arguments)]
async fn drain_recovered<D, F>(
    recovery: &mut dyn StreamRecoveryIo,
    topology: &dyn RedisTopology,
    policy: &NetworkRetryPolicy,
    params: &StreamConsumerParams,
    attempt: &mut u32,
    connected: &mut bool,
    cursor: &mut String,
    last_pass: &mut Option<tokio::time::Instant>,
    queue: &mut VecDeque<RecoveredEntry>,
    cancel: &CancellationToken,
    deliver: &mut D,
) -> Result<(), CamelError>
where
    D: FnMut(StreamEntry) -> F,
    F: Future<Output = DeliveryOutcome>,
{
    while let Some(recovered) = queue.pop_front() {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let RecoveredEntry {
            mut entry,
            delivery_count,
        } = recovered;
        let Some(count) = delivery_count else {
            // log-policy: outside-contract — the route owns operational signals.
            warn!(
                stream = %params.key,
                group = %params.group,
                id = %entry.id,
                "Recovered stream entry has no delivery count; skipping"
            );
            continue;
        };
        entry.delivery_count = count;

        let future = deliver(entry.clone());
        let mut in_flight = Box::pin(future);
        let outcome = loop {
            match wait_with_recovery(
                &mut in_flight,
                recovery,
                topology,
                policy,
                params,
                attempt,
                connected,
                cursor,
                last_pass,
                queue,
                cancel,
            )
            .await?
            {
                Waited::Ready(outcome) => break outcome,
                Waited::Cancelled => return Ok(()),
                Waited::PassCompleted => {}
            }
        };

        match outcome {
            DeliveryOutcome::Completed => {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                match recovery
                    .recover_ack(&params.key, &params.group, std::slice::from_ref(&entry.id))
                    .await
                {
                    Ok(_) => {}
                    Err(err) => {
                        if !crate::transport_error::is_transient_redis_error(&err) {
                            return Err(err);
                        }
                        // Transient: the connection is suspect and the ack
                        // outcome is unknown. Reconnect under the recovery
                        // budget; the entry stays pending.
                        *connected = false;
                        if !connect_recovery(recovery, topology, policy, attempt, cancel).await? {
                            return Ok(());
                        }
                        *connected = true;
                    }
                }
            }
            DeliveryOutcome::Failed => {}
            DeliveryOutcome::ChannelGone => {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                return Err(CamelError::ChannelClosed);
            }
        }
    }
    Ok(())
}

/// Drives one session from the read task's messages.
///
/// Test-facing seam: the caller supplies both channel ends, so unit tests can
/// drive the session without spawning the real read task. `on_ready` fires
/// exactly once, when [`ReadTaskMsg::Connected`] arrives. When recovery is
/// enabled, the first recovery pass runs (and its claimed entries are
/// delivered and acknowledged) before the start [`ReadTaskCmd::BatchDone`] is
/// sent, so the first `XREADGROUP` always happens after the initial pass.
///
/// A batch is settled in a bounded state machine: `pending` entries feed one
/// in-flight delivery at a time; completed ids collect in `awaiting_acks` and
/// are acknowledged in one command before `BatchDone` is sent. The session
/// never holds two outstanding deliveries. A recovery deadline may fire at any
/// wait: it runs a pass on the independent recovery half and queues the claimed
/// entries. Queued entries are drained only once no read-side delivery is
/// outstanding, FIFO, one at a time.
///
/// Cancellation returns `Ok` without acking work that had not been acked yet.
/// A non-transient read, ack, or recovery error, budget exhaustion, a reader
/// panic (which closes the message channel after `ReadFailed`), or an
/// unexpected channel closure returns `Err` so supervision fires.
#[allow(clippy::too_many_arguments)]
async fn session_core<D, F>(
    mut msg_rx: mpsc::Receiver<ReadTaskMsg>,
    cmd_tx: mpsc::Sender<ReadTaskCmd>,
    mut recovery: Box<dyn StreamRecoveryIo>,
    topology: Arc<dyn RedisTopology>,
    params: StreamConsumerParams,
    policy: NetworkRetryPolicy,
    cancel: CancellationToken,
    mut on_ready: Option<Box<dyn FnOnce() + Send>>,
    mut deliver: D,
) -> Result<(), CamelError>
where
    D: FnMut(StreamEntry) -> F,
    F: Future<Output = DeliveryOutcome>,
{
    let mut recovery_cursor = String::from("0-0");
    let mut last_recovery_pass: Option<tokio::time::Instant> = None;
    // The recovery half keeps its own retry budget, independent of the read
    // task's attempt counter.
    let mut recovery_attempt: u32 = 0;
    let mut recovery_connected = false;
    let mut recovered: VecDeque<RecoveredEntry> = VecDeque::new();

    loop {
        // Claimed entries are delivered only when no read-side delivery is
        // outstanding, so drain before waiting for the next read message.
        drain_recovered(
            recovery.as_mut(),
            topology.as_ref(),
            &policy,
            &params,
            &mut recovery_attempt,
            &mut recovery_connected,
            &mut recovery_cursor,
            &mut last_recovery_pass,
            &mut recovered,
            &cancel,
            &mut deliver,
        )
        .await?;

        let message = loop {
            let mut recv = Box::pin(msg_rx.recv());
            match wait_with_recovery(
                &mut recv,
                recovery.as_mut(),
                topology.as_ref(),
                &policy,
                &params,
                &mut recovery_attempt,
                &mut recovery_connected,
                &mut recovery_cursor,
                &mut last_recovery_pass,
                &mut recovered,
                &cancel,
            )
            .await?
            {
                Waited::Cancelled => return Ok(()),
                Waited::Ready(message) => break message,
                Waited::PassCompleted => {
                    drain_recovered(
                        recovery.as_mut(),
                        topology.as_ref(),
                        &policy,
                        &params,
                        &mut recovery_attempt,
                        &mut recovery_connected,
                        &mut recovery_cursor,
                        &mut last_recovery_pass,
                        &mut recovered,
                        &cancel,
                        &mut deliver,
                    )
                    .await?;
                }
            }
        };
        let Some(message) = message else {
            return reader_gone(&cancel);
        };

        match message {
            ReadTaskMsg::Connected => {
                if let Some(callback) = on_ready.take() {
                    callback();
                }
                if params.recover_enabled {
                    match recovery_pass(
                        recovery.as_mut(),
                        topology.as_ref(),
                        &policy,
                        &params,
                        &mut recovery_attempt,
                        &mut recovery_connected,
                        &mut recovery_cursor,
                        &mut last_recovery_pass,
                        &mut recovered,
                        &cancel,
                    )
                    .await?
                    {
                        RecoveryPass::Completed => {}
                        RecoveryPass::Cancelled => return Ok(()),
                    }
                    drain_recovered(
                        recovery.as_mut(),
                        topology.as_ref(),
                        &policy,
                        &params,
                        &mut recovery_attempt,
                        &mut recovery_connected,
                        &mut recovery_cursor,
                        &mut last_recovery_pass,
                        &mut recovered,
                        &cancel,
                        &mut deliver,
                    )
                    .await?;
                }
                if cmd_tx.send(ReadTaskCmd::BatchDone).await.is_err() {
                    return reader_gone(&cancel);
                }
            }
            ReadTaskMsg::Batch(entries) => {
                let mut pending: VecDeque<StreamEntry> = entries.into();
                let mut awaiting_acks: Vec<String> = Vec::new();
                let mut in_flight: Option<(StreamEntry, Pin<Box<F>>)> = None;

                loop {
                    if in_flight.is_none() {
                        // Pre-XACK cancellation point: stop the batch before
                        // starting another delivery or acknowledging anything
                        // that was only delivered before the cancel arrived.
                        if cancel.is_cancelled() {
                            return Ok(());
                        }
                        match pending.pop_front() {
                            Some(entry) => {
                                let future = deliver(entry.clone());
                                in_flight = Some((entry, Box::pin(future)));
                            }
                            None => break,
                        }
                    }

                    let outcome = {
                        let mut in_flight_future = Box::pin(await_in_flight(in_flight.as_mut()));
                        loop {
                            match wait_with_recovery(
                                &mut in_flight_future,
                                recovery.as_mut(),
                                topology.as_ref(),
                                &policy,
                                &params,
                                &mut recovery_attempt,
                                &mut recovery_connected,
                                &mut recovery_cursor,
                                &mut last_recovery_pass,
                                &mut recovered,
                                &cancel,
                            )
                            .await?
                            {
                                Waited::Ready(outcome) => break outcome,
                                Waited::Cancelled => return Ok(()),
                                // A read-side delivery is outstanding, so the
                                // queued recovery batch waits for the drain
                                // after this batch settles.
                                Waited::PassCompleted => {}
                            }
                        }
                    };
                    let entry = match in_flight.take() {
                        Some((entry, _)) => entry,
                        None => return Err(reader_channel_closed()),
                    };
                    match outcome {
                        DeliveryOutcome::Completed => awaiting_acks.push(entry.id),
                        DeliveryOutcome::Failed => {}
                        DeliveryOutcome::ChannelGone => {
                            if cancel.is_cancelled() {
                                return Ok(());
                            }
                            return Err(CamelError::ChannelClosed);
                        }
                    }
                }

                if cancel.is_cancelled() {
                    return Ok(());
                }
                if !awaiting_acks.is_empty() {
                    let ids = std::mem::take(&mut awaiting_acks);
                    let (reply_tx, reply_rx) = oneshot::channel();
                    if cmd_tx
                        .send(ReadTaskCmd::Ack {
                            ids,
                            reply: reply_tx,
                        })
                        .await
                        .is_err()
                    {
                        return reader_gone(&cancel);
                    }
                    let mut reply_rx = Box::pin(reply_rx);
                    let reply = loop {
                        match wait_with_recovery(
                            &mut reply_rx,
                            recovery.as_mut(),
                            topology.as_ref(),
                            &policy,
                            &params,
                            &mut recovery_attempt,
                            &mut recovery_connected,
                            &mut recovery_cursor,
                            &mut last_recovery_pass,
                            &mut recovered,
                            &cancel,
                        )
                        .await?
                        {
                            Waited::Ready(reply) => break reply,
                            Waited::Cancelled => return Ok(()),
                            // No delivery is outstanding while the ack is in
                            // flight, but keep the batch's ack obligation ahead
                            // of the queued recovery work so its progress is
                            // never restarted.
                            Waited::PassCompleted => {}
                        }
                    };
                    match reply {
                        Ok(Ok(_)) => {}
                        Ok(Err(err)) => {
                            if cancel.is_cancelled() {
                                return Ok(());
                            }
                            if is_retry_budget_exhausted(&err)
                                || !crate::transport_error::is_transient_redis_error(&err)
                            {
                                return Err(err);
                            }
                            // Transient ack failure: the read half reconnected
                            // and the outcome is unknown. The ids stay
                            // unacknowledged; only recovery may redeliver them.
                        }
                        Err(_) => {
                            return reader_gone(&cancel);
                        }
                    }
                }

                if cancel.is_cancelled() {
                    return Ok(());
                }
                if cmd_tx.send(ReadTaskCmd::BatchDone).await.is_err() {
                    return reader_gone(&cancel);
                }
            }
            ReadTaskMsg::NilTick => {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                if cmd_tx.send(ReadTaskCmd::BatchDone).await.is_err() {
                    return reader_gone(&cancel);
                }
            }
            ReadTaskMsg::ReadFailed(err) => {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                return Err(err);
            }
        }
    }
}

#[cfg(test)]
#[path = "stream_session_tests.rs"]
mod tests;
