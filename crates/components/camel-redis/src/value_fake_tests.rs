//! Wire-level value tests against the scripted RESP server.
//!
//! These tests drive the real [`crate::producer::RedisProducer`] so the value
//! bytes of the payload commands are asserted on the wire: the exchange body
//! is the only value source, bodies that cannot carry a value are rejected
//! before any command reaches the server, and the payload headers removed in
//! 0.69 fail loudly instead of being silently ignored. The reply side is
//! covered too: `GET` and `MGET` keep binary payloads losslessly while text
//! replies keep their `Text` / JSON-array shapes.

use std::sync::Arc;

use camel_component_api::Body;
use camel_component_api::CamelError;
use camel_component_api::Exchange;
use camel_component_api::StreamBody;

use crate::stream_fake_tests::{
    ScriptedAction, StreamFakeServer, args_as_strings, body_exchange, call_producer,
    keyed_exchange, resp_array, resp_bulk, resp_int, resp_nil,
};

#[tokio::test]
async fn set_carries_the_body_as_the_value() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "SET",
        reply: b"+OK\r\n".to_vec(),
    }])
    .await
    .expect("fake server must bind");

    call_producer(
        &server,
        "SET",
        "",
        keyed_exchange(Body::Text("hello".to_string()), "k"),
    )
    .await
    .expect("SET must succeed");

    let frames = server.args_of("SET");
    assert_eq!(frames.len(), 1, "exactly one SET must reach the wire");
    assert_eq!(
        frames[0],
        vec![b"k".to_vec(), b"hello".to_vec()],
        "SET must send the key and the exchange body as its value"
    );
}

#[tokio::test]
async fn empty_and_stream_bodies_fail_before_sending() {
    let server = StreamFakeServer::start(Vec::new())
        .await
        .expect("fake server must bind");

    // SET and XADD both resolve their value through `encode_body_value`, so
    // both commands must reject the same two body variants before any command
    // reaches the wire. The stream variant carries valid UTF-8 content to pin
    // that the variant itself is rejected, before any materialization attempt.
    for command in ["SET", "XADD"] {
        let stream_bytes = Body::Text("valid UTF-8 content".to_string())
            .into_bytes(1024)
            .await
            .expect("in-memory body must convert");
        let stream_body = Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(
                futures_util::stream::iter(vec![Ok::<_, CamelError>(stream_bytes)]),
            )))),
            metadata: Default::default(),
        });

        let cases = [("Body::Empty", Body::Empty), ("Body::Stream", stream_body)];
        for (variant, body) in cases {
            let err = match call_producer(&server, command, "", keyed_exchange(body, "k")).await {
                Err(err) => err,
                Ok(_) => panic!("{variant} must be rejected before sending {command}"),
            };
            let msg = match err {
                CamelError::ProcessorError(msg) => msg,
                other => panic!("{variant} must fail with a processor error: {other}"),
            };
            assert!(
                msg.contains(command),
                "{variant} error must name the command: {msg}"
            );
            assert!(
                msg.contains(variant),
                "{variant} error must name the rejected variant: {msg}"
            );
        }
    }

    assert!(
        server.recorded().is_empty(),
        "no frame may reach the wire for a rejected body: {:?}",
        server.recorded()
    );
}

// ── Binary-safe GET / MGET replies (redisvalue 1.3) ─────────────────────────

/// Encode a RESP bulk string holding arbitrary (possibly non-UTF-8) bytes.
fn resp_bulk_bytes(value: &[u8]) -> Vec<u8> {
    let mut out = format!("${}\r\n", value.len()).into_bytes();
    out.extend_from_slice(value);
    out.extend_from_slice(b"\r\n");
    out
}

/// Split the next CRLF-terminated RESP header off `buf`, returning it as text
/// plus the remaining bytes.
fn resp_line(buf: &[u8]) -> (&str, &[u8]) {
    let end = buf
        .windows(2)
        .position(|pair| pair == b"\r\n".as_slice())
        .expect("RESP frame must carry CRLF line endings");
    let line = std::str::from_utf8(&buf[..end]).expect("RESP header must be UTF-8");
    (line, &buf[end + 2..])
}

/// Decode the RESP array of bulk strings that
/// [`crate::commands::string::encode_mget_reply_bytes`] emits: an array
/// header, then bulk strings and `$-1` nils.
fn decode_resp_array(buf: &[u8]) -> Vec<Option<Vec<u8>>> {
    let (header, mut rest) = resp_line(buf);
    let count: usize = header
        .strip_prefix('*')
        .expect("MGET body must start with an array header")
        .parse()
        .expect("array header must carry an element count");

    let mut elements = Vec::with_capacity(count);
    for _ in 0..count {
        let (header, after) = resp_line(rest);
        let len: i64 = header
            .strip_prefix('$')
            .expect("every element must be a bulk string")
            .parse()
            .expect("bulk header must carry a length");
        let Some(len) = usize::try_from(len).ok() else {
            elements.push(None);
            rest = after;
            continue;
        };
        assert!(after.len() >= len + 2, "bulk payload must fit the frame");
        assert_eq!(
            &after[len..len + 2],
            b"\r\n",
            "bulk payload must end in CRLF"
        );
        elements.push(Some(after[..len].to_vec()));
        rest = &after[len + 2..];
    }
    assert!(
        rest.is_empty(),
        "decoder must consume the whole reply: {rest:?}"
    );
    elements
}

#[tokio::test]
async fn bytes_body_round_trips_through_set_and_get() {
    let payload = vec![0xFF, 0xFE, 0x00, 0x01];
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "SET",
            reply: b"+OK\r\n".to_vec(),
        },
        ScriptedAction::Reply {
            matches_command: "GET",
            reply: resp_bulk_bytes(&payload),
        },
    ])
    .await
    .expect("fake server must bind");

    call_producer(
        &server,
        "SET",
        "",
        keyed_exchange(Body::Bytes(payload.clone().into()), "k"),
    )
    .await
    .expect("SET must succeed");

    let outcome = call_producer(&server, "GET", "", keyed_exchange(Body::Empty, "k"))
        .await
        .expect("GET must succeed");

    let set_frames = server.args_of("SET");
    assert_eq!(set_frames.len(), 1, "exactly one SET must reach the wire");
    assert_eq!(
        set_frames[0],
        vec![b"k".to_vec(), payload.clone()],
        "SET must send the key and the body bytes verbatim"
    );
    assert_eq!(
        outcome.input.body,
        Body::Bytes(payload.into()),
        "GET must return the stored bytes verbatim"
    );
}

#[tokio::test]
async fn get_reply_is_text_when_utf8_and_bytes_otherwise() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "GET",
            reply: resp_bulk("café"),
        },
        ScriptedAction::Reply {
            matches_command: "GET",
            reply: resp_bulk_bytes(&[0xC3, 0x28]),
        },
    ])
    .await
    .expect("fake server must bind");

    let text_reply = call_producer(&server, "GET", "", keyed_exchange(Body::Empty, "text"))
        .await
        .expect("GET with a UTF-8 reply must succeed");
    let bytes_reply = call_producer(&server, "GET", "", keyed_exchange(Body::Empty, "binary"))
        .await
        .expect("GET with a binary reply must succeed");

    assert_eq!(
        text_reply.input.body,
        Body::Text("café".to_string()),
        "a UTF-8 reply must keep the Text body shape"
    );
    assert_eq!(
        bytes_reply.input.body,
        Body::Bytes(vec![0xC3, 0x28].into()),
        "a non-UTF-8 reply must become a Bytes body"
    );
}

#[tokio::test]
async fn mget_keeps_the_json_array_for_text_replies() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "MGET",
        reply: resp_array(&[resp_bulk("a"), resp_nil(), resp_bulk("c")]),
    }])
    .await
    .expect("fake server must bind");

    let mut exchange = body_exchange(Body::Empty);
    exchange
        .input
        .set_header("CamelRedis.Keys", serde_json::json!(["a", "b", "c"]));

    let outcome = call_producer(&server, "MGET", "", exchange)
        .await
        .expect("MGET must succeed");

    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!(["a", null, "c"])),
        "a text-only MGET reply must keep the JSON array shape"
    );
}

#[tokio::test]
async fn mget_encodes_binary_elements_losslessly() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "MGET",
        reply: resp_array(&[resp_bulk("a"), resp_nil(), resp_bulk_bytes(&[0x00, 0xFF])]),
    }])
    .await
    .expect("fake server must bind");

    let mut exchange = body_exchange(Body::Empty);
    exchange
        .input
        .set_header("CamelRedis.Keys", serde_json::json!(["a", "b", "c"]));

    let outcome = call_producer(&server, "MGET", "", exchange)
        .await
        .expect("MGET must succeed");

    let Body::Bytes(buf) = outcome.input.body else {
        panic!("MGET with a binary element must yield a Bytes body");
    };
    assert_eq!(
        decode_resp_array(&buf),
        vec![Some(b"a".to_vec()), None, Some(vec![0x00, 0xFF])],
        "the binary MGET reply must keep every element in server order"
    );
}

// ── List / hash value payloads (redisvalue 1.4) ─────────────────────────────

#[tokio::test]
async fn lpush_takes_the_value_from_the_body() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "LPUSH",
        reply: b":1\r\n".to_vec(),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "LPUSH",
        "",
        keyed_exchange(Body::Text("job".to_string()), "q"),
    )
    .await
    .expect("LPUSH must succeed");

    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!(1)),
        "LPUSH must keep the pushed-length reply as the body"
    );
    let frames = server.args_of("LPUSH");
    assert_eq!(frames.len(), 1, "exactly one LPUSH must reach the wire");
    assert_eq!(
        frames[0],
        vec![b"q".to_vec(), b"job".to_vec()],
        "LPUSH must send the key and the exchange body as its value"
    );
}

#[tokio::test]
async fn hmset_takes_a_json_object_body() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "HMSET",
        reply: b"+OK\r\n".to_vec(),
    }])
    .await
    .expect("fake server must bind");

    let exchange = keyed_exchange(Body::Json(serde_json::json!({"field": "value"})), "h");

    call_producer(&server, "HMSET", "", exchange)
        .await
        .expect("HMSET must succeed");

    let frames = server.args_of("HMSET");
    assert_eq!(frames.len(), 1, "exactly one HMSET must reach the wire");
    assert_eq!(
        frames[0],
        vec![b"h".to_vec(), b"field".to_vec(), b"value".to_vec()],
        "HMSET must send the key plus the body map field/value pairs"
    );
}

// ── GEO member and PUBLISH message from the body (redisvalue 1.5) ───────────

/// Exchange carrying `body`, the `CamelRedis.Key` header, and the coordinate
/// headers GEOADD requires. The promoted harness builders only set body/Key.
fn geo_exchange(body: Body, key: &str, lon: f64, lat: f64) -> Exchange {
    let mut exchange = keyed_exchange(body, key);
    exchange.input.set_header("CamelRedis.Longitude", lon);
    exchange.input.set_header("CamelRedis.Latitude", lat);
    exchange
}

#[tokio::test]
async fn geoadd_takes_the_body_member_when_member_header_is_absent() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "GEOADD",
        reply: resp_int(1),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "GEOADD",
        "",
        geo_exchange(
            Body::Text("Palermo".to_string()),
            "sicily",
            13.361389,
            38.115556,
        ),
    )
    .await
    .expect("GEOADD must succeed");

    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!(1)),
        "GEOADD must keep the added-count reply as the body"
    );
    let frames = server.args_of("GEOADD");
    assert_eq!(frames.len(), 1, "exactly one GEOADD must reach the wire");
    assert_eq!(
        frames[0].last(),
        Some(&b"Palermo".to_vec()),
        "GEOADD must send the exchange body in the member position"
    );
}

#[tokio::test]
async fn geoadd_missing_member_with_empty_body_fails_before_sending() {
    let server = StreamFakeServer::start(Vec::new())
        .await
        .expect("fake server must bind");

    let err = match call_producer(
        &server,
        "GEOADD",
        "",
        geo_exchange(Body::Empty, "sicily", 13.361389, 38.115556),
    )
    .await
    {
        Err(err) => err,
        Ok(_) => panic!("GEOADD without member header or body must be rejected before sending"),
    };
    let msg = match err {
        CamelError::ProcessorError(msg) => msg,
        other => panic!("GEOADD must fail with a processor error: {other}"),
    };
    assert!(msg.contains("GEOADD"), "error must name the command: {msg}");
    assert!(
        msg.contains("Body::Empty"),
        "error must name the rejected body variant: {msg}"
    );
    assert!(
        server.recorded().is_empty(),
        "no GEOADD frame may reach the wire: {:?}",
        server.recorded()
    );
}

#[tokio::test]
async fn publish_takes_the_message_from_the_body() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "PUBLISH",
        reply: resp_int(0),
    }])
    .await
    .expect("fake server must bind");

    let mut exchange = body_exchange(Body::Text("from-body".to_string()));
    exchange.input.set_header("CamelRedis.Channel", "news");

    call_producer(&server, "PUBLISH", "", exchange)
        .await
        .expect("PUBLISH with a body message must succeed");

    let frames = server.args_of("PUBLISH");
    assert_eq!(frames.len(), 1, "exactly one PUBLISH must reach the wire");
    assert_eq!(
        frames[0],
        vec![b"news".to_vec(), b"from-body".to_vec()],
        "PUBLISH must take the message from the exchange body"
    );
}

// ── XADD body value through the shared codec (redisvalue 1.6) ───────────────

#[tokio::test]
async fn xadd_accepts_a_bytes_body() {
    let payload = vec![0x00, 0xFF, 0x10];
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "XADD",
        reply: resp_bulk("1-1"),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "XADD",
        "",
        keyed_exchange(Body::Bytes(payload.clone().into()), "s1"),
    )
    .await
    .expect("XADD with a Bytes body must succeed");

    let frames = server.args_of("XADD");
    assert_eq!(frames.len(), 1, "exactly one XADD must reach the wire");
    assert_eq!(
        frames[0],
        vec![
            b"s1".to_vec(),
            b"*".to_vec(),
            b"body".to_vec(),
            payload.clone()
        ],
        "XADD must send the key, the `*` id, the `body` field, and the raw body bytes"
    );
    assert_eq!(
        outcome.input.header("CamelRedis.StreamId"),
        Some(&serde_json::json!("1-1")),
        "the server-assigned entry id must be published on CamelRedis.StreamId"
    );
    assert_eq!(
        outcome.input.body,
        Body::Bytes(payload.into()),
        "XADD must leave the exchange body unchanged"
    );
}

// ── Body-only contract: tombstones, codec, binary, maps, empty strings ──────

/// Exchange with `body`, the `CamelRedis.Key` header, and one extra header.
fn keyed_with_header(body: Body, key: &str, header: &str, value: serde_json::Value) -> Exchange {
    let mut exchange = keyed_exchange(body, key);
    exchange.input.set_header(header, value);
    exchange
}

#[tokio::test]
async fn value_header_tombstone_fails_before_send() {
    let server = StreamFakeServer::start(Vec::new())
        .await
        .expect("fake server must bind");

    // Every removed payload header, on a command that used to honour it. The
    // body is valid in all four rows, so only the tombstone can fail the call.
    let matrix = [
        (
            "SET",
            "CamelRedis.Value",
            keyed_with_header(
                Body::Text("body-value".to_string()),
                "k",
                "CamelRedis.Value",
                serde_json::json!("header-value"),
            ),
        ),
        (
            "HMSET",
            "CamelRedis.Values",
            keyed_with_header(
                Body::Json(serde_json::json!({"field": "value"})),
                "h",
                "CamelRedis.Values",
                serde_json::json!({"field": "header"}),
            ),
        ),
        (
            "MSET",
            "CamelRedis.Values",
            keyed_with_header(
                Body::Json(serde_json::json!({"k1": "v1"})),
                "k",
                "CamelRedis.Values",
                serde_json::json!({"k1": "header"}),
            ),
        ),
        ("GEOADD", "CamelRedis.Member", {
            let mut exchange = geo_exchange(
                Body::Text("Palermo".to_string()),
                "sicily",
                13.361389,
                38.115556,
            );
            exchange
                .input
                .set_header("CamelRedis.Member", serde_json::json!("Palermo"));
            exchange
        }),
    ];

    for (command, header, exchange) in matrix {
        let err = match call_producer(&server, command, "", exchange).await {
            Err(err) => err,
            Ok(_) => panic!("{command} must reject the removed {header} header before sending"),
        };
        let text = err.to_string();
        assert!(
            text.contains("was removed in 0.69"),
            "{command} error must report the 0.69 removal: {text}"
        );
        assert!(
            text.contains(header),
            "{command} error must name {header}: {text}"
        );
        assert!(
            text.contains("put the value in the body"),
            "{command} error must point at the body: {text}"
        );
        let frames = server.args_of(command);
        assert!(
            frames.is_empty(),
            "a tombstoned {command} must not reach the wire: {frames:?}"
        );
    }
    assert!(
        server.recorded().is_empty(),
        "no command may reach the wire for a tombstoned exchange: {:?}",
        server.recorded()
    );
}

#[tokio::test]
async fn json_string_body_contributes_raw_characters() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "SET",
        reply: b"+OK\r\n".to_vec(),
    }])
    .await
    .expect("fake server must bind");

    call_producer(
        &server,
        "SET",
        "",
        keyed_exchange(Body::Json(serde_json::json!("abc")), "k"),
    )
    .await
    .expect("SET with a JSON string body must succeed");

    let frames = server.args_of("SET");
    assert_eq!(frames.len(), 1, "exactly one SET must reach the wire");
    assert_eq!(
        frames[0],
        vec![b"k".to_vec(), b"abc".to_vec()],
        "a JSON string body must contribute its raw characters, without quote bytes"
    );
}

#[tokio::test]
async fn json_null_body_is_rejected() {
    let server = StreamFakeServer::start(Vec::new())
        .await
        .expect("fake server must bind");

    let err = match call_producer(
        &server,
        "SET",
        "",
        keyed_exchange(Body::Json(serde_json::Value::Null), "k"),
    )
    .await
    {
        Err(err) => err,
        Ok(_) => panic!("SET with a JSON null body must be rejected before sending"),
    };
    let text = err.to_string();
    assert!(text.contains("SET"), "the rejection must name SET: {text}");
    assert!(
        text.contains("null"),
        "the rejection must name the JSON null body: {text}"
    );
    assert!(
        server.args_of("SET").is_empty(),
        "no SET frame may reach the wire: {:?}",
        server.recorded()
    );
}

#[tokio::test]
async fn bytes_body_round_trips_zadd_then_zscore() {
    let member = vec![0xFF, 0xFE, b'b', b'i', b'n'];
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "ZADD",
            reply: resp_int(1),
        },
        ScriptedAction::Reply {
            matches_command: "ZSCORE",
            reply: resp_bulk("1.5"),
        },
    ])
    .await
    .expect("fake server must bind");

    call_producer(
        &server,
        "ZADD",
        "",
        keyed_with_header(
            Body::Bytes(member.clone().into()),
            "z",
            "CamelRedis.Score",
            serde_json::json!(1.5),
        ),
    )
    .await
    .expect("ZADD with a Bytes body must succeed");

    let outcome = call_producer(
        &server,
        "ZSCORE",
        "",
        keyed_exchange(Body::Bytes(member.clone().into()), "z"),
    )
    .await
    .expect("ZSCORE with the same Bytes body must succeed");

    let zadd_frames = server.args_of("ZADD");
    assert_eq!(zadd_frames.len(), 1, "exactly one ZADD must reach the wire");
    assert_eq!(
        zadd_frames[0],
        vec![b"z".to_vec(), b"1.5".to_vec(), member.clone()],
        "ZADD must send the key, the score, and the member bytes verbatim"
    );

    let zscore_frames = server.args_of("ZSCORE");
    assert_eq!(
        zscore_frames.len(),
        1,
        "exactly one ZSCORE must reach the wire"
    );
    assert_eq!(
        zscore_frames[0],
        vec![b"z".to_vec(), member],
        "ZSCORE must send the same member bytes verbatim"
    );
    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!(1.5)),
        "the scripted score reply must reach the body as a JSON number"
    );
}

#[tokio::test]
async fn bytes_body_round_trips_sadd_then_sismember() {
    let member = vec![0xFF, 0xFE, b'b', b'i', b'n'];
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "SADD",
            reply: resp_int(1),
        },
        ScriptedAction::Reply {
            matches_command: "SISMEMBER",
            reply: resp_int(1),
        },
    ])
    .await
    .expect("fake server must bind");

    call_producer(
        &server,
        "SADD",
        "",
        keyed_exchange(Body::Bytes(member.clone().into()), "s"),
    )
    .await
    .expect("SADD with a Bytes body must succeed");

    let outcome = call_producer(
        &server,
        "SISMEMBER",
        "",
        keyed_exchange(Body::Bytes(member.clone().into()), "s"),
    )
    .await
    .expect("SISMEMBER with the same Bytes body must succeed");

    let sadd_frames = server.args_of("SADD");
    assert_eq!(sadd_frames.len(), 1, "exactly one SADD must reach the wire");
    assert_eq!(
        sadd_frames[0],
        vec![b"s".to_vec(), member.clone()],
        "SADD must send the member bytes verbatim"
    );

    let sismember_frames = server.args_of("SISMEMBER");
    assert_eq!(
        sismember_frames.len(),
        1,
        "exactly one SISMEMBER must reach the wire"
    );
    assert_eq!(
        sismember_frames[0],
        vec![b"s".to_vec(), member],
        "SISMEMBER must send the same member bytes verbatim"
    );
    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!(true)),
        "the scripted integer reply must reach the body as a JSON boolean"
    );
}

#[tokio::test]
async fn mset_takes_a_json_object_body() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "MSET",
        reply: b"+OK\r\n".to_vec(),
    }])
    .await
    .expect("fake server must bind");

    call_producer(
        &server,
        "MSET",
        "",
        body_exchange(Body::Json(serde_json::json!({"k1": "v1", "k2": "v2"}))),
    )
    .await
    .expect("MSET with a JSON object body must succeed");

    let frames = server.args_of("MSET");
    assert_eq!(frames.len(), 1, "exactly one MSET must reach the wire");
    assert_eq!(
        frames[0],
        vec![
            b"k1".to_vec(),
            b"v1".to_vec(),
            b"k2".to_vec(),
            b"v2".to_vec()
        ],
        "MSET must send the map entries with raw, unquoted string values"
    );
}

#[tokio::test]
async fn hmset_map_values_follow_the_scalar_rule() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "HMSET",
        reply: b"+OK\r\n".to_vec(),
    }])
    .await
    .expect("fake server must bind");

    call_producer(
        &server,
        "HMSET",
        "",
        keyed_exchange(
            Body::Json(serde_json::json!({"n": 42, "s": "txt", "o": {"k": 1}})),
            "h",
        ),
    )
    .await
    .expect("HMSET with a JSON object body must succeed");

    let frames = server.args_of("HMSET");
    assert_eq!(frames.len(), 1, "exactly one HMSET must reach the wire");
    let args = args_as_strings(&frames[0]);
    assert_eq!(args[0], "h", "HMSET must send the key first");
    let mut entries: Vec<(String, String)> = args[1..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec![
            ("n".to_string(), "42".to_string()),
            ("o".to_string(), "{\"k\":1}".to_string()),
            ("s".to_string(), "txt".to_string()),
        ],
        "a JSON string keeps its raw characters, a number/object its compact text"
    );

    let err = match call_producer(
        &server,
        "HMSET",
        "",
        keyed_exchange(Body::Json(serde_json::json!({"bad": null})), "h"),
    )
    .await
    {
        Err(err) => err,
        Ok(_) => panic!("HMSET with a null map entry must be rejected before sending"),
    };
    let text = err.to_string();
    assert!(
        text.contains("HMSET") && text.contains("null"),
        "the rejection must name HMSET and the null entry: {text}"
    );
    assert_eq!(
        server.args_of("HMSET").len(),
        1,
        "a rejected HMSET must not add a frame: {:?}",
        server.args_of("HMSET")
    );

    let err = match call_producer(
        &server,
        "MSET",
        "",
        keyed_exchange(Body::Json(serde_json::json!({"bad": null})), "k"),
    )
    .await
    {
        Err(err) => err,
        Ok(_) => panic!("MSET with a null map entry must be rejected before sending"),
    };
    let text = err.to_string();
    assert!(
        text.contains("MSET") && text.contains("null"),
        "the rejection must name MSET and the null entry: {text}"
    );
    assert!(
        server.args_of("MSET").is_empty(),
        "a rejected MSET must not reach the wire: {:?}",
        server.args_of("MSET")
    );
}

#[tokio::test]
async fn smove_destination_stays_a_header_member_comes_from_the_body() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "SMOVE",
        reply: resp_int(1),
    }])
    .await
    .expect("fake server must bind");

    let mut exchange = keyed_exchange(Body::Text("member1".to_string()), "src");
    exchange.input.set_header("CamelRedis.Destination", "dest");

    call_producer(&server, "SMOVE", "", exchange)
        .await
        .expect("SMOVE must succeed");

    let frames = server.args_of("SMOVE");
    assert_eq!(frames.len(), 1, "exactly one SMOVE must reach the wire");
    assert_eq!(
        frames[0],
        vec![b"src".to_vec(), b"dest".to_vec(), b"member1".to_vec()],
        "SMOVE must send source, the header destination, then the body member"
    );
}

#[tokio::test]
async fn empty_string_body_is_accepted_and_empty_body_is_not() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "SET",
            reply: b"+OK\r\n".to_vec(),
        },
        ScriptedAction::Reply {
            matches_command: "SET",
            reply: b"+OK\r\n".to_vec(),
        },
    ])
    .await
    .expect("fake server must bind");

    call_producer(
        &server,
        "SET",
        "",
        keyed_exchange(Body::Text(String::new()), "k"),
    )
    .await
    .expect("an empty Text body must be a valid empty value");

    call_producer(
        &server,
        "SET",
        "",
        keyed_exchange(Body::Json(serde_json::json!("")), "k"),
    )
    .await
    .expect("an empty JSON string body must be a valid empty value");

    let err = match call_producer(&server, "SET", "", keyed_exchange(Body::Empty, "k")).await {
        Err(err) => err,
        Ok(_) => panic!("SET with Body::Empty must be rejected before sending"),
    };
    let text = err.to_string();
    assert!(
        text.contains("SET") && text.contains("Body::Empty"),
        "the rejection must name SET and Body::Empty: {text}"
    );

    let frames = server.args_of("SET");
    assert_eq!(
        frames.len(),
        2,
        "only the two empty-string SETs may reach the wire: {frames:?}"
    );
    assert_eq!(
        frames[0],
        vec![b"k".to_vec(), Vec::new()],
        "an empty Text body must send an empty value"
    );
    assert_eq!(
        frames[1],
        vec![b"k".to_vec(), Vec::new()],
        "an empty JSON string body must send an empty value"
    );
}

// ── SET options, GETDEL/GETEX, SETRANGE/GETRANGE (rediscmds 1.5) ─────────────

/// Exchange carrying `body`, the `CamelRedis.Key` header, and every option
/// header of one case.
fn keyed_with_headers(body: Body, key: &str, headers: &[(&str, serde_json::Value)]) -> Exchange {
    let mut exchange = keyed_exchange(body, key);
    for (header, value) in headers {
        exchange.input.set_header(*header, value.clone());
    }
    exchange
}

/// Render expected wire arguments as recorded RESP argument bytes.
fn expect_args(args: &[&str]) -> Vec<Vec<u8>> {
    args.iter().map(|arg| arg.as_bytes().to_vec()).collect()
}

/// One wire case: the option headers to set plus the expected frame arguments.
type WireCase<'a> = (Vec<(&'a str, serde_json::Value)>, Vec<&'a str>);

#[tokio::test]
async fn set_with_nx_and_xx_conditions_on_wire() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "SET",
            reply: b"+OK\r\n".to_vec(),
        },
        ScriptedAction::Reply {
            matches_command: "SET",
            reply: b"+OK\r\n".to_vec(),
        },
    ])
    .await
    .expect("fake server must bind");

    let mut expected = Vec::new();
    for condition in ["NX", "XX"] {
        call_producer(
            &server,
            "SET",
            "",
            keyed_with_headers(
                Body::Text("v".to_string()),
                "k",
                &[("CamelRedis.SetCondition", serde_json::json!(condition))],
            ),
        )
        .await
        .expect("a conditioned SET must succeed");
        expected.push(expect_args(&["k", "v", condition]));
    }

    assert_eq!(
        server.args_of("SET"),
        expected,
        "SET must render NX/XX right after the value"
    );
}

#[tokio::test]
async fn set_expiry_units_on_wire() {
    let reply = ScriptedAction::Reply {
        matches_command: "SET",
        reply: b"+OK\r\n".to_vec(),
    };
    let server = StreamFakeServer::start(vec![
        reply.clone(),
        reply.clone(),
        reply.clone(),
        reply.clone(),
        reply,
    ])
    .await
    .expect("fake server must bind");

    let cases: Vec<WireCase> = vec![
        // A bare Timeout means seconds, so the wire option is EX.
        (
            vec![("CamelRedis.Timeout", serde_json::json!(15u64))],
            vec!["k", "v", "EX", "15"],
        ),
        // The unit discriminator switches the relative expiry to PX.
        (
            vec![
                ("CamelRedis.Timeout", serde_json::json!(500u64)),
                ("CamelRedis.ExpireUnit", serde_json::json!("MILLISECONDS")),
            ],
            vec!["k", "v", "PX", "500"],
        ),
        // A bare Timestamp means seconds, so the wire option is EXAT.
        (
            vec![("CamelRedis.Timestamp", serde_json::json!(1731000000i64))],
            vec!["k", "v", "EXAT", "1731000000"],
        ),
        // The unit discriminator switches the absolute expiry to PXAT.
        (
            vec![
                ("CamelRedis.Timestamp", serde_json::json!(1731000000000i64)),
                ("CamelRedis.ExpireUnit", serde_json::json!("MILLISECONDS")),
            ],
            vec!["k", "v", "PXAT", "1731000000000"],
        ),
        // KEEPTTL stands alone and carries no value.
        (
            vec![("CamelRedis.KeepTtl", serde_json::json!(true))],
            vec!["k", "v", "KEEPTTL"],
        ),
    ];

    let mut expected = Vec::new();
    for (headers, frame) in &cases {
        call_producer(
            &server,
            "SET",
            "",
            keyed_with_headers(Body::Text("v".to_string()), "k", headers),
        )
        .await
        .expect("an expiring SET must succeed");
        expected.push(expect_args(frame));
    }

    assert_eq!(
        server.args_of("SET"),
        expected,
        "each expiry header must render its own unit and value"
    );
}

#[tokio::test]
async fn set_with_get_returns_old_value_binary_safe() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "SET",
        reply: resp_bulk_bytes(b"old"),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "SET",
        "",
        keyed_with_headers(
            Body::Text("new".to_string()),
            "k",
            &[("CamelRedis.ReturnOldValue", serde_json::json!(true))],
        ),
    )
    .await
    .expect("SET with ReturnOldValue must succeed");

    let frames = server.args_of("SET");
    assert_eq!(frames.len(), 1, "exactly one SET must reach the wire");
    assert_eq!(
        frames[0],
        expect_args(&["k", "new", "GET"]),
        "GET must be the last SET argument, after the value"
    );
    assert_eq!(
        outcome.input.body,
        Body::Text("old".to_string()),
        "the bulk reply must reach the body through the GET reply path"
    );
}

#[tokio::test]
async fn set_conflicts_fail_before_any_wire_frame() {
    // The server would answer SET, so an empty `args_of` proves the conflict
    // was refused locally instead of being reported by the server.
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "SET",
        reply: b"+OK\r\n".to_vec(),
    }])
    .await
    .expect("fake server must bind");

    let err = call_producer(
        &server,
        "SET",
        "",
        keyed_with_headers(
            Body::Text("v".to_string()),
            "k",
            &[
                ("CamelRedis.KeepTtl", serde_json::json!(true)),
                ("CamelRedis.Timeout", serde_json::json!(15u64)),
            ],
        ),
    )
    .await
    .expect_err("KEEPTTL with an expiry must fail before send");
    let msg = err.to_string();
    assert!(
        msg.contains("CamelRedis.KeepTtl") && msg.contains("CamelRedis.Timeout"),
        "the conflict error must name both headers: {msg}"
    );
    assert!(
        server.args_of("SET").is_empty(),
        "a conflicting SET must not reach the wire: {:?}",
        server.args_of("SET")
    );
}

#[tokio::test]
async fn getdel_sends_key_and_returns_binary_value() {
    let payload = vec![0xFF, 0x00];
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "GETDEL",
        reply: resp_bulk_bytes(&payload),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(&server, "GETDEL", "", keyed_exchange(Body::Empty, "k"))
        .await
        .expect("GETDEL must succeed");

    assert_eq!(
        server.args_of("GETDEL"),
        vec![expect_args(&["k"])],
        "GETDEL must send the key and no option argument"
    );
    assert_eq!(
        outcome.input.body,
        Body::Bytes(payload.into()),
        "GETDEL must keep the reply bytes binary-safe"
    );
}

#[tokio::test]
async fn getex_expiry_and_persist_frames() {
    let actions: Vec<ScriptedAction> = (0..6)
        .map(|_| ScriptedAction::Reply {
            matches_command: "GETEX",
            reply: resp_nil(),
        })
        .collect();
    let server = StreamFakeServer::start(actions)
        .await
        .expect("fake server must bind");

    let cases: Vec<WireCase> = vec![
        (
            vec![("CamelRedis.Timeout", serde_json::json!(100u64))],
            vec!["k", "EX", "100"],
        ),
        (
            vec![
                ("CamelRedis.Timeout", serde_json::json!(500u64)),
                ("CamelRedis.ExpireUnit", serde_json::json!("MILLISECONDS")),
            ],
            vec!["k", "PX", "500"],
        ),
        (
            vec![("CamelRedis.Timestamp", serde_json::json!(1731000000i64))],
            vec!["k", "EXAT", "1731000000"],
        ),
        (
            vec![
                ("CamelRedis.Timestamp", serde_json::json!(1731000000000i64)),
                ("CamelRedis.ExpireUnit", serde_json::json!("MILLISECONDS")),
            ],
            vec!["k", "PXAT", "1731000000000"],
        ),
        (
            vec![("CamelRedis.Persist", serde_json::json!(true))],
            vec!["k", "PERSIST"],
        ),
        // No option header at all: the frame is exactly `GETEX k`.
        (Vec::new(), vec!["k"]),
    ];

    let mut expected = Vec::new();
    for (headers, frame) in &cases {
        call_producer(
            &server,
            "GETEX",
            "",
            keyed_with_headers(Body::Empty, "k", headers),
        )
        .await
        .expect("GETEX must succeed");
        expected.push(expect_args(frame));
    }

    assert_eq!(
        server.args_of("GETEX"),
        expected,
        "each GETEX mapping must render its own option arguments"
    );
}

#[tokio::test]
async fn setrange_frame_and_length_reply() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "SETRANGE",
            reply: resp_int(11),
        },
        ScriptedAction::Reply {
            matches_command: "SETRANGE",
            reply: resp_int(5),
        },
    ])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "SETRANGE",
        "",
        keyed_with_headers(
            Body::Text("world".to_string()),
            "k",
            &[("CamelRedis.Offset", serde_json::json!(6u64))],
        ),
    )
    .await
    .expect("SETRANGE at offset 6 must succeed");
    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!(11)),
        "SETRANGE must keep the new-length reply as JSON"
    );

    // Offset zero is legal and must be sent as the literal `0`.
    call_producer(
        &server,
        "SETRANGE",
        "",
        keyed_with_headers(
            Body::Text("hello".to_string()),
            "k",
            &[("CamelRedis.Offset", serde_json::json!(0u64))],
        ),
    )
    .await
    .expect("SETRANGE at offset 0 must succeed");

    assert_eq!(
        server.args_of("SETRANGE"),
        vec![
            expect_args(&["k", "6", "world"]),
            expect_args(&["k", "0", "hello"]),
        ],
        "SETRANGE must send the key, the offset, then the body"
    );
}

#[tokio::test]
async fn getrange_frame_and_binary_slice() {
    let slice = vec![0x00, 0xFF, 0x80, b'h'];
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "GETRANGE",
        reply: resp_bulk_bytes(&slice),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "GETRANGE",
        "",
        keyed_with_headers(
            Body::Empty,
            "k",
            &[
                ("CamelRedis.Start", serde_json::json!(0i64)),
                ("CamelRedis.End", serde_json::json!(3i64)),
            ],
        ),
    )
    .await
    .expect("GETRANGE must succeed");

    assert_eq!(
        server.args_of("GETRANGE"),
        vec![expect_args(&["k", "0", "3"])],
        "GETRANGE must send the key and both range bounds"
    );
    assert_eq!(
        outcome.input.body,
        Body::Bytes(slice.into()),
        "GETRANGE must keep the slice bytes binary-safe"
    );
}

// ── Phase 2 command frames and reply shapes (rediscmds 2.7) ─────────────────

/// Exchange carrying a `CamelRedis.Keys` list and any option headers.
fn keys_with_headers(keys: serde_json::Value, headers: &[(&str, serde_json::Value)]) -> Exchange {
    let mut exchange = body_exchange(Body::Empty);
    exchange.input.set_header("CamelRedis.Keys", keys);
    for (header, value) in headers {
        exchange.input.set_header(*header, value.clone());
    }
    exchange
}

/// `LMPOP`/`BLMPOP` exchange: keys `a`,`b`, end `LEFT`, `COUNT 2`.
fn pop_exchange(headers: &[(&str, serde_json::Value)]) -> Exchange {
    let mut all: Vec<(&str, serde_json::Value)> = vec![
        ("CamelRedis.Where", serde_json::json!("LEFT")),
        ("CamelRedis.Count", serde_json::json!(2)),
    ];
    all.extend_from_slice(headers);
    keys_with_headers(serde_json::json!(["a", "b"]), &all)
}

#[tokio::test]
async fn lmpop_frame_and_reply_shape() {
    let binary_reply = resp_array(&[resp_bulk("a"), resp_array(&[resp_bulk_bytes(b"\xff")])]);
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "LMPOP",
            reply: resp_array(&[
                resp_bulk("a"),
                resp_array(&[resp_bulk("x"), resp_bulk("y")]),
            ]),
        },
        ScriptedAction::Reply {
            matches_command: "LMPOP",
            reply: binary_reply.clone(),
        },
    ])
    .await
    .expect("fake server must bind");

    let text = call_producer(&server, "LMPOP", "", pop_exchange(&[]))
        .await
        .expect("LMPOP with an all-UTF-8 reply must succeed");
    assert_eq!(
        text.input.body,
        Body::Json(serde_json::json!(["a", ["x", "y"]])),
        "an all-UTF-8 LMPOP reply keeps the JSON [key, [elements]] shape"
    );

    let binary = call_producer(&server, "LMPOP", "", pop_exchange(&[]))
        .await
        .expect("LMPOP with a binary element must succeed");
    assert_eq!(
        binary.input.body,
        Body::Bytes(binary_reply.into()),
        "a binary element switches the whole reply to the RESP-encoded Bytes body"
    );

    assert_eq!(
        server.args_of("LMPOP"),
        vec![expect_args(&["2", "a", "b", "LEFT", "COUNT", "2"]); 2],
        "LMPOP must send numkeys, keys, the end, then COUNT n"
    );
}

#[tokio::test]
async fn blmpop_frame_with_timeout() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "BLMPOP",
            reply: resp_nil(),
        },
        ScriptedAction::Reply {
            matches_command: "BLMPOP",
            reply: resp_nil(),
        },
    ])
    .await
    .expect("fake server must bind");

    let mut expected = Vec::new();
    for (timeout, leading) in [(5u64, "5"), (0u64, "0")] {
        let outcome = call_producer(
            &server,
            "BLMPOP",
            "",
            pop_exchange(&[("CamelRedis.Timeout", serde_json::json!(timeout))]),
        )
        .await
        .expect("BLMPOP must succeed");
        assert_eq!(
            outcome.input.body,
            Body::Json(serde_json::Value::Null),
            "a nil BLMPOP reply must be JSON null"
        );
        expected.push(expect_args(&[leading, "2", "a", "b", "LEFT", "COUNT", "2"]));
    }

    assert_eq!(
        server.args_of("BLMPOP"),
        expected,
        "BLMPOP must lead with the timeout, and a missing/zero timeout sends 0"
    );
}

#[tokio::test]
async fn lmove_frame_and_element_reply() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "LMOVE",
        reply: resp_bulk("x"),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "LMOVE",
        "",
        keyed_with_headers(
            Body::Empty,
            "src",
            &[
                ("CamelRedis.Destination", serde_json::json!("dst")),
                ("CamelRedis.WhereFrom", serde_json::json!("LEFT")),
                ("CamelRedis.WhereTo", serde_json::json!("RIGHT")),
            ],
        ),
    )
    .await
    .expect("LMOVE must succeed");

    assert_eq!(
        server.args_of("LMOVE"),
        vec![expect_args(&["src", "dst", "LEFT", "RIGHT"])],
        "LMOVE must send source, destination, then both directions"
    );
    assert_eq!(
        outcome.input.body,
        Body::Text("x".to_string()),
        "the moved element follows the GET reply rule"
    );
}

#[tokio::test]
async fn blmove_frame() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "BLMOVE",
        reply: resp_bulk("y"),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "BLMOVE",
        "",
        keyed_with_headers(
            Body::Empty,
            "src",
            &[
                ("CamelRedis.Destination", serde_json::json!("dst")),
                ("CamelRedis.WhereFrom", serde_json::json!("LEFT")),
                ("CamelRedis.WhereTo", serde_json::json!("RIGHT")),
                ("CamelRedis.Timeout", serde_json::json!(1u64)),
            ],
        ),
    )
    .await
    .expect("BLMOVE must succeed");

    assert_eq!(
        server.args_of("BLMOVE"),
        vec![expect_args(&["src", "dst", "LEFT", "RIGHT", "1"])],
        "BLMOVE must append the blocking timeout after both directions"
    );
    assert_eq!(
        outcome.input.body,
        Body::Text("y".to_string()),
        "the moved element follows the GET reply rule"
    );
}

#[tokio::test]
async fn lpos_frame_from_body() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "LPOS",
            reply: resp_int(2),
        },
        ScriptedAction::Reply {
            matches_command: "LPOS",
            reply: resp_nil(),
        },
        ScriptedAction::Reply {
            matches_command: "LPOS",
            reply: resp_array(&[resp_int(0), resp_int(3)]),
        },
    ])
    .await
    .expect("fake server must bind");

    let ranked = call_producer(
        &server,
        "LPOS",
        "",
        keyed_with_headers(
            Body::Text("x".to_string()),
            "k",
            &[
                ("CamelRedis.Rank", serde_json::json!(1i64)),
                ("CamelRedis.Count", serde_json::json!(3i64)),
            ],
        ),
    )
    .await
    .expect("LPOS with RANK and COUNT must succeed");
    assert_eq!(
        ranked.input.body,
        Body::Json(serde_json::json!(2)),
        "a scalar LPOS reply maps to a JSON index"
    );

    let bare = call_producer(
        &server,
        "LPOS",
        "",
        keyed_exchange(Body::Text("x".to_string()), "k"),
    )
    .await
    .expect("LPOS with no options must succeed");
    assert_eq!(
        bare.input.body,
        Body::Json(serde_json::Value::Null),
        "a nil LPOS reply maps to JSON null"
    );

    let all_matches = call_producer(
        &server,
        "LPOS",
        "",
        keyed_with_headers(
            Body::Text("x".to_string()),
            "k",
            &[("CamelRedis.Count", serde_json::json!(0i64))],
        ),
    )
    .await
    .expect("LPOS COUNT 0 must succeed");
    assert_eq!(
        all_matches.input.body,
        Body::Json(serde_json::json!([0, 3])),
        "an array LPOS reply maps to a JSON index array"
    );

    assert_eq!(
        server.args_of("LPOS"),
        vec![
            expect_args(&["k", "x", "RANK", "1", "COUNT", "3"]),
            expect_args(&["k", "x"]),
            expect_args(&["k", "x", "COUNT", "0"]),
        ],
        "LPOS must read the element from the body and map RANK/COUNT after it"
    );
}

#[tokio::test]
async fn smismember_frame_and_bool_reply() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "SMISMEMBER",
        reply: resp_array(&[resp_int(1), resp_int(0)]),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "SMISMEMBER",
        "",
        keyed_with_headers(
            Body::Empty,
            "k",
            &[("CamelRedis.Members", serde_json::json!(["a", "b"]))],
        ),
    )
    .await
    .expect("SMISMEMBER must succeed");

    assert_eq!(
        server.args_of("SMISMEMBER"),
        vec![expect_args(&["k", "a", "b"])],
        "SMISMEMBER must send the key and every member"
    );
    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!([true, false])),
        "the 1/0 reply array must map to JSON booleans"
    );
}

#[tokio::test]
async fn sintercard_frame_with_and_without_limit() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "SINTERCARD",
            reply: resp_int(2),
        },
        ScriptedAction::Reply {
            matches_command: "SINTERCARD",
            reply: resp_int(1),
        },
    ])
    .await
    .expect("fake server must bind");

    let limited = call_producer(
        &server,
        "SINTERCARD",
        "",
        keys_with_headers(
            serde_json::json!(["a", "b"]),
            &[("CamelRedis.Limit", serde_json::json!(10i64))],
        ),
    )
    .await
    .expect("SINTERCARD with LIMIT must succeed");
    assert_eq!(
        limited.input.body,
        Body::Json(serde_json::json!(2)),
        "the SINTERCARD integer reply is the JSON body"
    );

    call_producer(
        &server,
        "SINTERCARD",
        "",
        keys_with_headers(serde_json::json!(["a", "b"]), &[]),
    )
    .await
    .expect("SINTERCARD without LIMIT must succeed");

    assert_eq!(
        server.args_of("SINTERCARD"),
        vec![
            expect_args(&["2", "a", "b", "LIMIT", "10"]),
            expect_args(&["2", "a", "b"]),
        ],
        "SINTERCARD must send numkeys and keys, then LIMIT only when configured"
    );
}

#[tokio::test]
async fn zmpop_frame_and_reply_shape() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "ZMPOP",
        reply: resp_array(&[
            resp_bulk("a"),
            resp_array(&[resp_array(&[resp_bulk("m"), resp_bulk("1.5")])]),
        ]),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "ZMPOP",
        "",
        keys_with_headers(
            serde_json::json!(["a", "b"]),
            &[
                ("CamelRedis.OrderBy", serde_json::json!("MAX")),
                ("CamelRedis.Count", serde_json::json!(2u64)),
            ],
        ),
    )
    .await
    .expect("ZMPOP must succeed");

    assert_eq!(
        server.args_of("ZMPOP"),
        vec![expect_args(&["2", "a", "b", "MAX", "COUNT", "2"])],
        "ZMPOP must send numkeys, keys, the score order, then COUNT n"
    );
    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!(["a", [["m", 1.5]]])),
        "a UTF-8 ZMPOP reply keeps the JSON [key, [[member, score]]] shape with numeric scores"
    );
}

#[tokio::test]
async fn bzmpop_frame() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "BZMPOP",
            reply: resp_nil(),
        },
        ScriptedAction::Reply {
            matches_command: "BZMPOP",
            reply: resp_nil(),
        },
    ])
    .await
    .expect("fake server must bind");

    call_producer(
        &server,
        "BZMPOP",
        "",
        keys_with_headers(
            serde_json::json!(["a", "b"]),
            &[
                ("CamelRedis.OrderBy", serde_json::json!("MIN")),
                ("CamelRedis.Timeout", serde_json::json!(0u64)),
            ],
        ),
    )
    .await
    .expect("BZMPOP with the default count must succeed");

    call_producer(
        &server,
        "BZMPOP",
        "",
        keys_with_headers(
            serde_json::json!(["a", "b"]),
            &[
                ("CamelRedis.OrderBy", serde_json::json!("MIN")),
                ("CamelRedis.Timeout", serde_json::json!(0u64)),
                ("CamelRedis.Count", serde_json::json!(2u64)),
            ],
        ),
    )
    .await
    .expect("BZMPOP with an explicit count must succeed");

    assert_eq!(
        server.args_of("BZMPOP"),
        vec![
            expect_args(&["0", "2", "a", "b", "MIN", "COUNT", "1"]),
            expect_args(&["0", "2", "a", "b", "MIN", "COUNT", "2"]),
        ],
        "BZMPOP must lead with the timeout and always carry an explicit COUNT"
    );
}

#[tokio::test]
async fn zrandmember_frames_and_replies() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "ZRANDMEMBER",
            reply: resp_bulk("m"),
        },
        ScriptedAction::Reply {
            matches_command: "ZRANDMEMBER",
            reply: resp_array(&[resp_bulk("m"), resp_bulk("n")]),
        },
    ])
    .await
    .expect("fake server must bind");

    let single = call_producer(&server, "ZRANDMEMBER", "", keyed_exchange(Body::Empty, "k"))
        .await
        .expect("ZRANDMEMBER without COUNT must succeed");
    assert_eq!(
        single.input.body,
        Body::Text("m".to_string()),
        "the single-member reply follows the GET reply rule"
    );

    let counted = call_producer(
        &server,
        "ZRANDMEMBER",
        "",
        keyed_with_headers(
            Body::Empty,
            "k",
            &[("CamelRedis.Count", serde_json::json!(-5i64))],
        ),
    )
    .await
    .expect("ZRANDMEMBER with a negative COUNT must succeed");
    assert_eq!(
        counted.input.body,
        Body::Json(serde_json::json!(["m", "n"])),
        "the counted reply follows the MGET binary rule"
    );

    assert_eq!(
        server.args_of("ZRANDMEMBER"),
        vec![expect_args(&["k"]), expect_args(&["k", "-5"])],
        "a negative COUNT must reach the wire verbatim (repeats allowed)"
    );
}

#[tokio::test]
async fn zdiff_zinter_zunion_frames() {
    let reply = resp_array(&[resp_bulk("m1"), resp_bulk("m2")]);
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "ZDIFF",
            reply: reply.clone(),
        },
        ScriptedAction::Reply {
            matches_command: "ZINTER",
            reply: reply.clone(),
        },
        ScriptedAction::Reply {
            matches_command: "ZUNION",
            reply: reply.clone(),
        },
    ])
    .await
    .expect("fake server must bind");

    for (command, keys) in [
        ("ZDIFF", serde_json::json!(["a", "b"])),
        ("ZINTER", serde_json::json!(["a", "b", "c"])),
        ("ZUNION", serde_json::json!(["a", "b"])),
    ] {
        let outcome = call_producer(&server, command, "", keys_with_headers(keys, &[]))
            .await
            .unwrap_or_else(|err| panic!("{command} must succeed: {err}"));
        assert_eq!(
            outcome.input.body,
            Body::Json(serde_json::json!(["m1", "m2"])),
            "{command} must map the member array to a JSON array"
        );
    }

    assert_eq!(
        server.args_of("ZDIFF"),
        vec![expect_args(&["2", "a", "b"])],
        "ZDIFF must send numkeys then keys"
    );
    assert_eq!(
        server.args_of("ZINTER"),
        vec![expect_args(&["3", "a", "b", "c"])],
        "ZINTER must send numkeys then keys"
    );
    assert_eq!(
        server.args_of("ZUNION"),
        vec![expect_args(&["2", "a", "b"])],
        "ZUNION must send numkeys then keys"
    );
}

#[tokio::test]
async fn zrangebylex_frame() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "ZRANGEBYLEX",
        reply: resp_array(&[resp_bulk("a"), resp_bulk("b")]),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "ZRANGEBYLEX",
        "",
        keyed_with_headers(
            Body::Empty,
            "k",
            &[
                ("CamelRedis.Min", serde_json::json!("[a")),
                ("CamelRedis.Max", serde_json::json!("+")),
            ],
        ),
    )
    .await
    .expect("ZRANGEBYLEX must succeed");

    assert_eq!(
        server.args_of("ZRANGEBYLEX"),
        vec![expect_args(&["k", "[a", "+"])],
        "ZRANGEBYLEX must send the key and both lexicographic bounds"
    );
    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!(["a", "b"])),
        "the member array must map to a JSON array per the binary rule"
    );
}

#[tokio::test]
async fn hrandfield_frame() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "HRANDFIELD",
            reply: resp_bulk("m"),
        },
        ScriptedAction::Reply {
            matches_command: "HRANDFIELD",
            reply: resp_array(&[resp_bulk("f1"), resp_bulk("f2")]),
        },
    ])
    .await
    .expect("fake server must bind");

    let single = call_producer(&server, "HRANDFIELD", "", keyed_exchange(Body::Empty, "k"))
        .await
        .expect("HRANDFIELD without COUNT must succeed");
    assert_eq!(
        single.input.body,
        Body::Text("m".to_string()),
        "the single-field reply follows the GET reply rule"
    );

    let counted = call_producer(
        &server,
        "HRANDFIELD",
        "",
        keyed_with_headers(
            Body::Empty,
            "k",
            &[("CamelRedis.Count", serde_json::json!(3i64))],
        ),
    )
    .await
    .expect("HRANDFIELD with COUNT must succeed");
    assert_eq!(
        counted.input.body,
        Body::Json(serde_json::json!(["f1", "f2"])),
        "the counted reply follows the MGET binary rule"
    );

    assert_eq!(
        server.args_of("HRANDFIELD"),
        vec![expect_args(&["k"]), expect_args(&["k", "3"])],
        "HRANDFIELD must append COUNT only when configured"
    );
}

#[tokio::test]
async fn hincrbyfloat_frame_and_float_reply() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "HINCRBYFLOAT",
        reply: resp_bulk("2.5"),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "HINCRBYFLOAT",
        "",
        keyed_with_headers(
            Body::Empty,
            "k",
            &[
                ("CamelRedis.Field", serde_json::json!("balance")),
                ("CamelRedis.Increment", serde_json::json!(1.5)),
            ],
        ),
    )
    .await
    .expect("HINCRBYFLOAT must succeed");

    assert_eq!(
        server.args_of("HINCRBYFLOAT"),
        vec![expect_args(&["k", "balance", "1.5"])],
        "HINCRBYFLOAT must send the shortest round-trip increment form"
    );
    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!("2.5")),
        "the bulk-string float reply must stay a JSON string, not a Text body"
    );
}

#[tokio::test]
async fn lmpop_empty_keys_fails_before_send() {
    let server = StreamFakeServer::start(Vec::new())
        .await
        .expect("fake server must bind");

    let err = call_producer(
        &server,
        "LMPOP",
        "",
        keys_with_headers(
            serde_json::json!([]),
            &[("CamelRedis.Where", serde_json::json!("LEFT"))],
        ),
    )
    .await
    .expect_err("LMPOP with an empty key list must fail before send");
    let text = err.to_string();
    assert!(
        text.contains("LMPOP") && text.contains("must not be empty"),
        "the error must name LMPOP and the empty key list: {text}"
    );
    assert!(
        server.args_of("LMPOP").is_empty(),
        "an empty-key LMPOP must not reach the wire: {:?}",
        server.args_of("LMPOP")
    );
    assert!(
        server.recorded().is_empty(),
        "no frame may reach the wire for an empty key list: {:?}",
        server.recorded()
    );
}

// ── 3.3: UNLINK / COPY / TOUCH wire proofs ──────────────────────────────────

#[tokio::test]
async fn unlink_frame_and_count_reply() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "UNLINK",
        reply: resp_int(1),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(&server, "UNLINK", "", keyed_exchange(Body::Empty, "k"))
        .await
        .expect("UNLINK must succeed");

    assert_eq!(
        server.args_of("UNLINK"),
        vec![expect_args(&["k"])],
        "UNLINK must mirror the single-key DEL frame"
    );
    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!(1)),
        "the UNLINK integer reply must stay a JSON number"
    );
}

#[tokio::test]
async fn copy_frames_with_and_without_options() {
    let server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "COPY",
            reply: resp_int(1),
        },
        ScriptedAction::Reply {
            matches_command: "COPY",
            reply: resp_int(1),
        },
        ScriptedAction::Reply {
            matches_command: "COPY",
            reply: resp_int(0),
        },
    ])
    .await
    .expect("fake server must bind");

    let full_headers: Vec<(&str, serde_json::Value)> = vec![
        ("CamelRedis.Destination", serde_json::json!("dst")),
        ("CamelRedis.Db", serde_json::json!(3u64)),
        ("CamelRedis.Replace", serde_json::json!(true)),
    ];
    let full = call_producer(
        &server,
        "COPY",
        "",
        keyed_with_headers(Body::Empty, "src", &full_headers),
    )
    .await
    .expect("a COPY with DB and REPLACE must succeed");

    let minimal = call_producer(
        &server,
        "COPY",
        "",
        keyed_with_headers(
            Body::Empty,
            "src",
            &[("CamelRedis.Destination", serde_json::json!("dst"))],
        ),
    )
    .await
    .expect("a COPY without optionals must succeed");

    let refused = call_producer(
        &server,
        "COPY",
        "",
        keyed_with_headers(
            Body::Empty,
            "src",
            &[("CamelRedis.Destination", serde_json::json!("dst"))],
        ),
    )
    .await
    .expect("a COPY whose source is missing must succeed with a false result");

    assert_eq!(
        server.args_of("COPY"),
        vec![
            expect_args(&["src", "dst", "DB", "3", "REPLACE"]),
            expect_args(&["src", "dst"]),
            expect_args(&["src", "dst"]),
        ],
        "COPY must append DB before REPLACE, and send neither when unset"
    );
    assert_eq!(
        full.input.body,
        Body::Json(serde_json::json!(true)),
        "a COPY 1 reply must map to a JSON boolean"
    );
    assert_eq!(
        minimal.input.body,
        Body::Json(serde_json::json!(true)),
        "a COPY 1 reply must map to a JSON boolean"
    );
    assert_eq!(
        refused.input.body,
        Body::Json(serde_json::json!(false)),
        "a COPY 0 reply must map to the false JSON boolean"
    );
}

#[tokio::test]
async fn touch_frame_and_count_reply() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "TOUCH",
        reply: resp_int(2),
    }])
    .await
    .expect("fake server must bind");

    let outcome = call_producer(
        &server,
        "TOUCH",
        "",
        keys_with_headers(serde_json::json!(["a", "b"]), &[]),
    )
    .await
    .expect("TOUCH must succeed");

    assert_eq!(
        server.args_of("TOUCH"),
        vec![expect_args(&["a", "b"])],
        "TOUCH must send each key of the CamelRedis.Keys list in order"
    );
    assert_eq!(
        outcome.input.body,
        Body::Json(serde_json::json!(2)),
        "the TOUCH integer reply must stay a JSON number"
    );
}

#[tokio::test]
async fn copy_missing_destination_fails_before_send() {
    let server = StreamFakeServer::start(Vec::new())
        .await
        .expect("fake server must bind");

    let err = call_producer(&server, "COPY", "", keyed_exchange(Body::Empty, "src"))
        .await
        .expect_err("COPY without a destination must fail before send");
    let text = err.to_string();
    assert!(
        text.contains("CamelRedis.Destination"),
        "the error must name CamelRedis.Destination: {text}"
    );
    assert!(
        server.args_of("COPY").is_empty(),
        "a destination-less COPY must not reach the wire: {:?}",
        server.args_of("COPY")
    );
}
