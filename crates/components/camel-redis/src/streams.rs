//! Stream consumer-group I/O seam and failover loop.
//!
//! Mirrors [`crate::queue`] for Redis Streams: [`StreamIo`] abstracts the
//! `XREADGROUP`/`XACK` calls so the reconnect loop can be tested without a
//! broker, while [`RedisStreamIo`] is the real multiplexed-connection
//! implementation.
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
use std::future::Future;
use std::ops::ControlFlow;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::retry::transient_retry_step;
use crate::topology::{RedisTopology, ServerKind};
use crate::transport_error::{TransientByProse, TransportTimeout, marker_camel, redis_error_raw};

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
}

/// One stream entry returned by `XREADGROUP`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamEntry {
    /// Entry ID (for example `"1700000000000-0"`).
    pub(crate) id: String,
    /// Field/value pairs carried by the entry.
    pub(crate) fields: Vec<(String, String)>,
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
/// - `CamelRedis.StreamDeliveryCount`: the delivery count (`"1"` for `>` reads)
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
        ("CamelRedis.Stream", stream_key),
        ("CamelRedis.StreamId", entry.id.as_str()),
        ("CamelRedis.StreamGroup", group),
        ("CamelRedis.StreamConsumer", consumer),
        ("CamelRedis.StreamDeliveryCount", "1"),
    ];
    for (name, value) in headers {
        exchange
            .input
            .set_header(name, serde_json::Value::String(value.to_string()));
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
    Ok(StreamEntry { id, fields })
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

/// Injectable I/O seam for the stream consumer's `XREADGROUP` loop.
///
/// Lets the failover loop be tested without a broker: the real impl talks to
/// Redis, a test double records programmable outcomes.
#[async_trait]
pub(crate) trait StreamIo: Send {
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

/// Real [`StreamIo`] backed by a multiplexed Redis connection.
pub(crate) struct RedisStreamIo {
    conn: Option<redis::aio::MultiplexedConnection>,
    connection_timeout_secs: u64,
}

impl RedisStreamIo {
    /// Create an unconnected stream I/O handle whose connect is bounded by
    /// `connection_timeout_secs`.
    pub(crate) fn new(connection_timeout_secs: u64) -> Self {
        Self {
            conn: None,
            connection_timeout_secs,
        }
    }
}

#[async_trait]
impl StreamIo for RedisStreamIo {
    async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError> {
        // Deadline-free: `XREADGROUP BLOCK` owns the wait; a response
        // deadline would cut long blocks short.
        let conn = tokio::time::timeout(
            Duration::from_secs(self.connection_timeout_secs),
            client.get_multiplexed_async_connection_with_config(
                &redis::AsyncConnectionConfig::new()
                    .set_response_timeout(None)
                    .set_connection_timeout(None),
            ),
        )
        .await
        .map_err(|_| {
            marker_camel(
                format!(
                    "Stream connection timed out after {}s",
                    self.connection_timeout_secs
                ),
                TransportTimeout {
                    stage: "stream connect",
                },
            )
        })?
        .map_err(|e| {
            marker_camel(
                format!("Failed to create connection: {}", e),
                TransientByProse {
                    site: "stream connect",
                },
            )
        })?;
        self.conn = Some(conn);
        Ok(())
    }

    async fn read_group(
        &mut self,
        params: &StreamConsumerParams,
    ) -> Result<Option<Vec<StreamEntry>>, CamelError> {
        let conn = self.conn.as_mut().ok_or_else(|| {
            marker_camel(
                "Stream connection not established".into(),
                TransientByProse {
                    site: "stream read guard",
                },
            )
        })?;

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
        let conn = self.conn.as_mut().ok_or_else(|| {
            marker_camel(
                "Stream connection not established".into(),
                TransientByProse {
                    site: "stream ack guard",
                },
            )
        })?;

        let mut cmd = redis::cmd("XACK");
        cmd.arg(key).arg(group);
        for id in ids {
            cmd.arg(id.as_str());
        }
        cmd.query_async(conn).await.map_err(redis_error_raw)
    }
}

/// Run a whole consumer-group session on ONE connection.
///
/// Resolves the current master, connects, and performs `XREADGROUP` reads on
/// that single connection, delivering every entry through `deliver`. An entry
/// whose delivery returns [`DeliveryOutcome::Completed`] is acknowledged; a
/// [`DeliveryOutcome::Failed`] entry is skipped without an ack. A
/// [`DeliveryOutcome::ChannelGone`] entry ends the session immediately with
/// [`CamelError::ChannelClosed`], without an ack. A `BLOCK` timeout
/// (`Ok(None)`) is normal and KEEPS the connection.
///
/// `on_ready` fires exactly once, after the first successful connect.
///
/// On a transient error (connection refused/EOF/timeout/reset/readonly) at any
/// stage the loop re-resolves the master and reconnects, bounded by `policy`;
/// each such failure consumes one attempt. A non-transient error, or budget
/// exhaustion, returns `Err` so Route supervision fires (ADR-0007).
pub(crate) async fn stream_session<D, F>(
    topology: &dyn RedisTopology,
    io: &mut dyn StreamIo,
    params: &StreamConsumerParams,
    policy: &NetworkRetryPolicy,
    cancel: &CancellationToken,
    on_ready: Option<Box<dyn FnOnce() + Send>>,
    mut deliver: D,
) -> Result<(), CamelError>
where
    D: FnMut(StreamEntry) -> F,
    F: Future<Output = DeliveryOutcome>,
{
    let key = params.key.as_str();
    let group = params.group.as_str();
    let resolve_stage = "resolving master for XREADGROUP".to_string();
    let connect_stage = "connecting for XREADGROUP".to_string();
    let read_stage = "reading with XREADGROUP".to_string();
    let ack_stage = "acking with XREADGROUP".to_string();
    let mut attempt: u32 = 0;
    let mut ready = on_ready;

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        // Resolve the current master (re-resolves on every call for sentinel).
        let client = match topology.resolve(ServerKind::Master).await {
            Ok(client) => client,
            Err(e) => match transient_retry_step(policy, &mut attempt, e, &resolve_stage).await {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(e) => return Err(e),
            },
        };

        // Establish a fresh connection to the resolved master.
        if let Err(e) = io.connect(&client).await {
            match transient_retry_step(policy, &mut attempt, e, &connect_stage).await {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(e) => return Err(e),
            }
        }

        // Readiness is not eager: it fires only after a connect succeeded.
        if let Some(callback) = ready.take() {
            callback();
        }

        // Read and deliver from this one connection until a transient error
        // forces a reconnect.
        'connection: loop {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                read = io.read_group(params) => {
                    match read {
                        Ok(Some(entries)) => {
                            for entry in entries {
                                // Cancellation between entries: stop consuming
                                // the rest of this batch. An entry already
                                // delivered+acked stays acked; an entry whose
                                // delivery is in flight is never skipped before
                                // the ack attempt (at-least-once).
                                if cancel.is_cancelled() {
                                    return Ok(());
                                }
                                let id = entry.id.clone();
                                match deliver(entry).await {
                                    DeliveryOutcome::Completed => {
                                        match io.ack(key, group, &[id]).await {
                                            Ok(_) => {}
                                            Err(e) => {
                                                match transient_retry_step(
                                                    policy,
                                                    &mut attempt,
                                                    e,
                                                    &ack_stage,
                                                )
                                                .await
                                                {
                                                    ControlFlow::Continue(()) => break 'connection,
                                                    ControlFlow::Break(e) => return Err(e),
                                                }
                                            }
                                        }
                                    }
                                    DeliveryOutcome::Failed => continue,
                                    DeliveryOutcome::ChannelGone => {
                                        // The handoff target is gone: no ack,
                                        // and the session stops. Reacts like a
                                        // non-transient error (supervision).
                                        return Err(CamelError::ChannelClosed);
                                    }
                                }
                            }
                        }
                        Ok(None) => continue, // BLOCK timeout — stay on this connection
                        Err(e) => {
                            match transient_retry_step(policy, &mut attempt, e, &read_stage).await {
                                ControlFlow::Continue(()) => break 'connection, // → reconnect
                                ControlFlow::Break(e) => return Err(e),
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::FakeTopology;
    use crate::transport_error::io_reset_error;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::{Notify, oneshot};

    type ReadOutcome = Result<Option<Vec<StreamEntry>>, CamelError>;

    fn io_reset() -> CamelError {
        redis_error_raw(io_reset_error())
    }

    fn entry(id: &str, fields: &[(&str, &str)]) -> StreamEntry {
        StreamEntry {
            id: id.to_string(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn batch(entries: Vec<StreamEntry>) -> ReadOutcome {
        Ok(Some(entries))
    }

    #[test]
    fn exchange_single_field_body_and_headers() {
        let entry = entry("1-1", &[("payload", "hello")]);

        let exchange = build_exchange_from_stream_entry("s1", "g", "c", &entry);

        assert_eq!(exchange.input.body.as_text(), Some("hello"));
        assert_eq!(
            exchange.input.headers.len(),
            5,
            "single-field entry must set exactly the five delivery headers"
        );
        assert_eq!(
            exchange.input.header("CamelRedis.Stream"),
            Some(&serde_json::json!("s1"))
        );
        assert_eq!(
            exchange.input.header("CamelRedis.StreamId"),
            Some(&serde_json::json!("1-1"))
        );
        assert_eq!(
            exchange.input.header("CamelRedis.StreamGroup"),
            Some(&serde_json::json!("g"))
        );
        assert_eq!(
            exchange.input.header("CamelRedis.StreamConsumer"),
            Some(&serde_json::json!("c"))
        );
        assert_eq!(
            exchange.input.header("CamelRedis.StreamDeliveryCount"),
            Some(&serde_json::json!("1"))
        );
    }

    #[test]
    fn exchange_multi_field_body_is_json_object() {
        let entry = entry("2-2", &[("b", "2"), ("a", "1")]);

        let exchange = build_exchange_from_stream_entry("s1", "g", "c", &entry);

        let body = exchange
            .input
            .body
            .as_text()
            .expect("multi-field body must be text");
        assert_eq!(
            body, r#"{"a":"1","b":"2"}"#,
            "multi-field body must be byte-for-byte alphabetical"
        );
        let parsed: serde_json::Value =
            serde_json::from_str(body).expect("multi-field body must be JSON");
        assert_eq!(parsed, serde_json::json!({"a": "1", "b": "2"}));
    }

    /// Test double for [`StreamIo`]: programmable connect/read/ack outcomes
    /// and recorded successful acks.
    ///
    /// Once the read queue drains, `read_group` pends forever, so session tests
    /// end by cancellation rather than by inventing extra entries.
    struct FakeStreamIo {
        connect_outcomes: Vec<Result<(), CamelError>>,
        read_queue: Vec<ReadOutcome>,
        ack_outcomes: VecDeque<Result<i64, CamelError>>,
        connects: Arc<AtomicUsize>,
        connect_changed: Arc<Notify>,
        acks: Arc<Mutex<Vec<String>>>,
        ack_changed: Arc<Notify>,
        gate: Option<oneshot::Receiver<()>>,
        gate_entered: Option<oneshot::Sender<()>>,
    }

    impl FakeStreamIo {
        fn new(
            connect_outcomes: Vec<Result<(), CamelError>>,
            read_outcomes: Vec<ReadOutcome>,
        ) -> Self {
            Self {
                connect_outcomes,
                read_queue: read_outcomes,
                ack_outcomes: VecDeque::new(),
                connects: Arc::new(AtomicUsize::new(0)),
                connect_changed: Arc::new(Notify::new()),
                acks: Arc::new(Mutex::new(Vec::new())),
                ack_changed: Arc::new(Notify::new()),
                gate: None,
                gate_entered: None,
            }
        }

        fn with_ack_outcomes(mut self, outcomes: Vec<Result<i64, CamelError>>) -> Self {
            self.ack_outcomes = outcomes.into();
            self
        }

        /// Gate the first successful connect return behind `gate`; `entered`
        /// fires just before the fake blocks on it.
        fn with_connect_gate(
            mut self,
            gate: oneshot::Receiver<()>,
            entered: oneshot::Sender<()>,
        ) -> Self {
            self.gate = Some(gate);
            self.gate_entered = Some(entered);
            self
        }

        fn connect_count(&self) -> usize {
            self.connects.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl StreamIo for FakeStreamIo {
        async fn connect(&mut self, _client: &redis::Client) -> Result<(), CamelError> {
            let idx = self.connects.load(Ordering::SeqCst);
            self.connects.fetch_add(1, Ordering::SeqCst);
            self.connect_changed.notify_waiters();

            if let Some(entered) = self.gate_entered.take() {
                let _ = entered.send(());
            }
            if let Some(gate) = self.gate.take() {
                let _ = gate.await;
            }

            match self.connect_outcomes.get(idx) {
                Some(outcome) => outcome.clone(),
                None => self.connect_outcomes.last().cloned().unwrap_or(Ok(())),
            }
        }

        async fn read_group(
            &mut self,
            _params: &StreamConsumerParams,
        ) -> Result<Option<Vec<StreamEntry>>, CamelError> {
            if !self.read_queue.is_empty() {
                return self.read_queue.remove(0);
            }
            std::future::pending().await
        }

        async fn ack(
            &mut self,
            _key: &str,
            _group: &str,
            ids: &[String],
        ) -> Result<i64, CamelError> {
            let outcome = self.ack_outcomes.pop_front().unwrap_or(Ok(1));
            if outcome.is_ok() {
                let mut acks = self.acks.lock().expect("acks mutex");
                acks.extend(ids.iter().cloned());
                drop(acks);
                self.ack_changed.notify_waiters();
            }
            outcome
        }
    }

    fn fast_policy(max_attempts: u32) -> NetworkRetryPolicy {
        NetworkRetryPolicy {
            max_attempts,
            initial_delay: Duration::from_millis(0),
            ..NetworkRetryPolicy::default()
        }
    }

    fn stream_params() -> StreamConsumerParams {
        StreamConsumerParams {
            key: "stream".to_string(),
            group: "g".to_string(),
            consumer: "c".to_string(),
            count: Some(10),
            block_ms: 1_000,
        }
    }

    /// Await `condition` without sleeping; `change` wakes the waiters on every
    /// state mutation.
    async fn wait_until(change: &Notify, mut condition: impl FnMut() -> bool) {
        loop {
            let notified = change.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if condition() {
                return;
            }
            notified.await; // allow-test-wait: every caller wraps wait_until in tokio::time::timeout
        }
    }

    async fn wait_for_acks(acks: &Mutex<Vec<String>>, ack_changed: &Notify, target: usize) {
        tokio::time::timeout(
            Duration::from_secs(5),
            wait_until(ack_changed, || {
                acks.lock().expect("acks mutex").len() >= target
            }),
        )
        .await
        .expect("acks must arrive before the timeout");
    }

    #[tokio::test]
    async fn session_delivers_and_acks_each_entry() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeStreamIo::new(
            vec![Ok(())],
            vec![batch(vec![
                entry("1-1", &[("payload", "hello")]),
                entry("2-2", &[("a", "1")]),
            ])],
        )
        .with_ack_outcomes(vec![Ok(1), Ok(1)]);
        let acks = io.acks.clone();
        let ack_changed = io.ack_changed.clone();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let seen = delivered.clone();
        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        let policy = fast_policy(2);
        let params = stream_params();

        let handle = tokio::spawn(async move {
            let deliver = move |entry: StreamEntry| {
                let seen = seen.clone();
                async move {
                    seen.lock().expect("delivered mutex").push(entry.id);
                    DeliveryOutcome::Completed
                }
            };
            stream_session(
                &topology,
                &mut io,
                &params,
                &policy,
                &cancel_task,
                None,
                deliver,
            )
            .await
        });

        wait_for_acks(&acks, &ack_changed, 2).await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("session must finish")
            .expect("session task must not panic");

        assert!(result.is_ok(), "session must end Ok: {result:?}");
        assert_eq!(
            *acks.lock().expect("acks mutex"),
            vec!["1-1".to_string(), "2-2".to_string()],
            "both entries must be acked in delivery order"
        );
        assert_eq!(
            *delivered.lock().expect("delivered mutex"),
            vec!["1-1".to_string(), "2-2".to_string()],
            "both entries must be delivered in order"
        );
    }

    #[tokio::test]
    async fn session_no_ack_when_deliver_fails() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeStreamIo::new(
            vec![Ok(())],
            vec![batch(vec![entry("1-1", &[]), entry("2-2", &[])])],
        )
        .with_ack_outcomes(vec![Ok(1)]);
        let acks = io.acks.clone();
        let ack_changed = io.ack_changed.clone();
        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        let policy = fast_policy(2);
        let params = stream_params();

        let handle = tokio::spawn(async move {
            let deliver = move |entry: StreamEntry| async move {
                if entry.id == "1-1" {
                    DeliveryOutcome::Failed
                } else {
                    DeliveryOutcome::Completed
                }
            };
            stream_session(
                &topology,
                &mut io,
                &params,
                &policy,
                &cancel_task,
                None,
                deliver,
            )
            .await
        });

        wait_for_acks(&acks, &ack_changed, 1).await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("session must finish")
            .expect("session task must not panic");

        assert!(result.is_ok(), "session must end Ok: {result:?}");
        assert_eq!(
            *acks.lock().expect("acks mutex"),
            vec!["2-2".to_string()],
            "a Failed delivery must not be acked"
        );
    }

    #[tokio::test]
    async fn session_channel_gone_returns_err_without_ack() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeStreamIo::new(
            vec![Ok(())],
            vec![batch(vec![entry("1-1", &[]), entry("2-2", &[])])],
        );
        let acks = io.acks.clone();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let seen = delivered.clone();
        let cancel = CancellationToken::new();
        let policy = fast_policy(2);
        let params = stream_params();

        let deliver = move |entry: StreamEntry| {
            let seen = seen.clone();
            async move {
                seen.lock().expect("delivered mutex").push(entry.id.clone());
                if entry.id == "1-1" {
                    DeliveryOutcome::ChannelGone
                } else {
                    DeliveryOutcome::Completed
                }
            }
        };

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            stream_session(&topology, &mut io, &params, &policy, &cancel, None, deliver),
        )
        .await
        .expect("session must terminate on ChannelGone");

        match result {
            Err(CamelError::ChannelClosed) => {}
            other => {
                panic!("ChannelGone must end the session with ChannelClosed: {other:?}")
            }
        }
        assert!(
            acks.lock().expect("acks mutex").is_empty(),
            "ChannelGone must not ack the delivered entry"
        );
        assert_eq!(
            *delivered.lock().expect("delivered mutex"),
            vec!["1-1".to_string()],
            "session must stop delivering after ChannelGone"
        );
    }

    #[tokio::test]
    async fn session_stops_batch_processing_on_cancellation() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeStreamIo::new(
            vec![Ok(())],
            vec![batch(vec![entry("1-1", &[]), entry("2-2", &[])])],
        );
        let acks = io.acks.clone();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let seen = delivered.clone();
        let cancel = CancellationToken::new();
        let cancel_in_deliver = cancel.clone();
        let policy = fast_policy(2);
        let params = stream_params();

        let deliver = move |entry: StreamEntry| {
            let seen = seen.clone();
            let cancel_in_deliver = cancel_in_deliver.clone();
            async move {
                seen.lock().expect("delivered mutex").push(entry.id.clone());
                if entry.id == "1-1" {
                    cancel_in_deliver.cancel();
                }
                DeliveryOutcome::Completed
            }
        };

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            stream_session(&topology, &mut io, &params, &policy, &cancel, None, deliver),
        )
        .await
        .expect("cancelled session must finish");

        assert!(result.is_ok(), "cancelled session must end Ok: {result:?}");
        assert_eq!(
            *acks.lock().expect("acks mutex"),
            vec!["1-1".to_string()],
            "the entry delivered before cancellation must stay acked"
        );
        assert_eq!(
            *delivered.lock().expect("delivered mutex"),
            vec!["1-1".to_string()],
            "cancellation must stop the batch before delivering 2-2"
        );
    }

    #[tokio::test]
    async fn session_block_timeout_keeps_connection_then_delivers() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeStreamIo::new(
            vec![Ok(())],
            vec![Ok(None), batch(vec![entry("1-1", &[("payload", "hello")])])],
        );
        let acks = io.acks.clone();
        let ack_changed = io.ack_changed.clone();
        let connects = io.connects.clone();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let seen = delivered.clone();
        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        let policy = fast_policy(2);
        let params = stream_params();

        let handle = tokio::spawn(async move {
            let deliver = move |entry: StreamEntry| {
                let seen = seen.clone();
                async move {
                    seen.lock().expect("delivered mutex").push(entry.id);
                    DeliveryOutcome::Completed
                }
            };
            stream_session(
                &topology,
                &mut io,
                &params,
                &policy,
                &cancel_task,
                None,
                deliver,
            )
            .await
        });

        wait_for_acks(&acks, &ack_changed, 1).await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("session must finish")
            .expect("session task must not panic");

        assert!(result.is_ok(), "session must end Ok: {result:?}");
        assert_eq!(
            connects.load(Ordering::SeqCst),
            1,
            "an Ok(None) BLOCK timeout must keep the connection"
        );
        assert_eq!(
            *acks.lock().expect("acks mutex"),
            vec!["1-1".to_string()],
            "only the real entry must be acked"
        );
        assert_eq!(
            *delivered.lock().expect("delivered mutex"),
            vec!["1-1".to_string()],
            "the BLOCK timeout must not deliver a phantom entry"
        );
    }

    #[tokio::test]
    async fn session_cancel_while_blocked_no_ack() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeStreamIo::new(vec![Ok(())], vec![]);
        let connects = io.connects.clone();
        let connect_changed = io.connect_changed.clone();
        let acks = io.acks.clone();
        let cancel = CancellationToken::new();
        let cancel_control = cancel.clone();

        let controller = tokio::spawn(async move {
            wait_until(&connect_changed, || connects.load(Ordering::SeqCst) >= 1).await;
            cancel_control.cancel();
        });

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            stream_session(
                &topology,
                &mut io,
                &stream_params(),
                &fast_policy(2),
                &cancel,
                None,
                |_entry| async { DeliveryOutcome::Completed },
            ),
        )
        .await
        .expect("cancelled session must finish");

        tokio::time::timeout(Duration::from_secs(5), controller)
            .await
            .expect("controller must finish")
            .expect("controller must not panic");
        assert!(result.is_ok(), "cancelled session must end Ok: {result:?}");
        assert!(
            acks.lock().expect("acks mutex").is_empty(),
            "a blocked read must not ack anything"
        );
    }

    #[tokio::test]
    async fn session_transient_read_error_reconnects_and_delivers() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeStreamIo::new(
            vec![Ok(()), Ok(())],
            vec![Err(io_reset()), batch(vec![entry("1-1", &[])])],
        )
        .with_ack_outcomes(vec![Ok(1)]);
        let acks = io.acks.clone();
        let ack_changed = io.ack_changed.clone();
        let connects = io.connects.clone();
        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        let policy = fast_policy(2);
        let params = stream_params();

        let handle = tokio::spawn(async move {
            stream_session(
                &topology,
                &mut io,
                &params,
                &policy,
                &cancel_task,
                None,
                |_entry| async { DeliveryOutcome::Completed },
            )
            .await
        });

        wait_for_acks(&acks, &ack_changed, 1).await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("session must finish")
            .expect("session task must not panic");

        assert!(result.is_ok(), "session must end Ok: {result:?}");
        assert_eq!(
            connects.load(Ordering::SeqCst),
            2,
            "a transient read error must reconnect"
        );
        assert_eq!(
            *acks.lock().expect("acks mutex"),
            vec!["1-1".to_string()],
            "the entry read after reconnect must be acked"
        );
    }

    #[tokio::test]
    async fn session_non_transient_read_error_fails_supervision() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let nogroup = redis_error_raw(redis::RedisError::from((
            redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
            "NOGROUP No such consumer group 'g'",
        )));
        let mut io = FakeStreamIo::new(vec![Ok(())], vec![Err(nogroup)]);

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            stream_session(
                &topology,
                &mut io,
                &stream_params(),
                &fast_policy(3),
                &CancellationToken::new(),
                None,
                |_entry| async { DeliveryOutcome::Completed },
            ),
        )
        .await
        .expect("session must finish");

        assert!(
            result.is_err(),
            "a NOGROUP response must fail supervision, got Ok"
        );
        assert_eq!(
            io.connect_count(),
            1,
            "a non-transient read error must not reconnect"
        );
    }

    #[tokio::test]
    async fn session_fires_on_ready_once_after_connect() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let (gate_tx, gate_rx) = oneshot::channel();
        let (entered_tx, entered_rx) = oneshot::channel();
        let mut io = FakeStreamIo::new(vec![Ok(()), Ok(())], vec![Err(io_reset())])
            .with_connect_gate(gate_rx, entered_tx);
        let connects = io.connects.clone();
        let connect_changed = io.connect_changed.clone();
        let ready_count = Arc::new(AtomicUsize::new(0));
        let ready_for_callback = ready_count.clone();
        let on_ready: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
            ready_for_callback.fetch_add(1, Ordering::SeqCst);
        }));
        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        let policy = fast_policy(2);
        let params = stream_params();

        let handle = tokio::spawn(async move {
            stream_session(
                &topology,
                &mut io,
                &params,
                &policy,
                &cancel_task,
                on_ready,
                |_entry| async { DeliveryOutcome::Completed },
            )
            .await
        });

        entered_rx
            .await
            .expect("session must reach the gated connect");
        assert_eq!(
            ready_count.load(Ordering::SeqCst),
            0,
            "on_ready must not fire before the connect completes"
        );

        gate_tx.send(()).expect("release the gated connect");
        tokio::time::timeout(
            Duration::from_secs(5),
            wait_until(&connect_changed, || connects.load(Ordering::SeqCst) >= 2),
        )
        .await
        .expect("session must reconnect after the transient read error");

        assert_eq!(
            ready_count.load(Ordering::SeqCst),
            1,
            "on_ready must fire exactly once across both connections"
        );

        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("session must finish")
            .expect("session task must not panic");
        assert!(result.is_ok(), "session must end Ok: {result:?}");
    }

    #[tokio::test]
    async fn session_transient_ack_error_reconnects_no_replay() {
        let topology = FakeTopology::addrs(vec!["redis://a:6379".into()]);
        let mut io = FakeStreamIo::new(
            vec![Ok(()), Ok(())],
            vec![
                batch(vec![entry("1-1", &[])]),
                batch(vec![entry("2-2", &[])]),
            ],
        )
        .with_ack_outcomes(vec![Err(io_reset()), Ok(1)]);
        let acks = io.acks.clone();
        let ack_changed = io.ack_changed.clone();
        let connects = io.connects.clone();
        let cancel = CancellationToken::new();
        let cancel_task = cancel.clone();
        let policy = fast_policy(2);
        let params = stream_params();

        let handle = tokio::spawn(async move {
            stream_session(
                &topology,
                &mut io,
                &params,
                &policy,
                &cancel_task,
                None,
                |_entry| async { DeliveryOutcome::Completed },
            )
            .await
        });

        wait_for_acks(&acks, &ack_changed, 1).await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("session must finish")
            .expect("session task must not panic");

        assert!(result.is_ok(), "session must end Ok: {result:?}");
        assert_eq!(
            connects.load(Ordering::SeqCst),
            2,
            "a transient ack error must reconnect"
        );
        assert_eq!(
            *acks.lock().expect("acks mutex"),
            vec!["2-2".to_string()],
            "the unknown-outcome ack must never be retried after reconnect"
        );
        assert!(
            !acks
                .lock()
                .expect("acks mutex")
                .contains(&"1-1".to_string()),
            "the 1-1 ack must never land: its outcome is unknown"
        );
    }

    // Regression: a valid stream entry whose field value is not valid UTF-8
    // must fail decoding. Substituting an empty string would corrupt the
    // payload and let the entry be acknowledged as delivered.
    #[test]
    fn invalid_utf8_field_fails_decoding_instead_of_becoming_empty() {
        let reply = redis::Value::Array(vec![redis::Value::Array(vec![
            redis::Value::BulkString(b"events".to_vec()),
            redis::Value::Array(vec![redis::Value::Array(vec![
                redis::Value::BulkString(b"1-1".to_vec()),
                redis::Value::Array(vec![
                    redis::Value::BulkString(b"payload".to_vec()),
                    redis::Value::BulkString(b"\xff\xfe".to_vec()),
                ]),
            ])]),
        ])]);

        let err = entries_from_reply(reply).expect_err("invalid UTF-8 must fail decoding");

        assert!(
            err.to_string().contains("UTF-8"),
            "decode failure must name the UTF-8 failure: {err}"
        );
        assert!(
            !crate::transport_error::is_transient_redis_error(&err),
            "a field decode failure must be non-transient so supervision fires: {err}"
        );
    }
}
