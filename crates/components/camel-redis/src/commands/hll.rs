use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::aio::MultiplexedConnection;

pub(crate) fn is_hll_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Pfadd | RedisCommand::Pfcount | RedisCommand::Pfmerge
    )
}

/// Resolves the `CamelRedis.Keys` source list for `PFCOUNT` and `PFMERGE`.
///
/// Both commands take one or more source keys, so a single-key command is a
/// one-element list. The strict list resolver rejects a missing header, an
/// empty list, a non-list value, and a non-string entry, so a route never
/// reads or merges fewer keys than it configured (ADR-0032). Every violation
/// is a named `CamelRedis.Keys` error before send.
pub(crate) fn resolve_pf_keys(exchange: &Exchange, verb: &str) -> Result<Vec<String>, CamelError> {
    super::require_string_list(exchange, "CamelRedis.Keys", verb, "key")
}

/// Builds `PFADD key element`.
pub(crate) fn encode_pfadd(key: &str, element: &[u8]) -> redis::Cmd {
    let mut command = redis::cmd("PFADD");
    command.arg(key).arg(element);
    command
}

/// Builds `PFCOUNT key...`.
pub(crate) fn encode_pfcount(keys: &[String]) -> redis::Cmd {
    let mut command = redis::cmd("PFCOUNT");
    for key in keys {
        command.arg(key);
    }
    command
}

/// Builds `PFMERGE dest key...`.
pub(crate) fn encode_pfmerge(dest: &str, keys: &[String]) -> redis::Cmd {
    let mut command = redis::cmd("PFMERGE");
    command.arg(dest);
    for key in keys {
        command.arg(key);
    }
    command
}

/// Dispatches the HyperLogLog commands.
///
/// `PFADD` takes its element from the exchange body and replies with the
/// integer `1` when the structure changed; `PFCOUNT` replies with the
/// estimated cardinality of its sources; `PFMERGE` replies with a status that
/// becomes the body as text. Integer replies map to the body as JSON numbers.
pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_hll_command(cmd) {
        return Err(CamelError::ProcessorError(
            "Not a HyperLogLog command".into(),
        ));
    }

    match cmd {
        RedisCommand::Pfadd => {
            let key = super::require_key(exchange)?;
            let element = super::body_value_bytes(exchange, "PFADD", &[crate::HEADER_VALUE])?;
            let changed: i64 = encode_pfadd(&key, &element)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("PFADD", e))?;
            exchange.input.body = Body::Json(serde_json::json!(changed));
        }
        RedisCommand::Pfcount => {
            let keys = resolve_pf_keys(exchange, "PFCOUNT")?;
            let cardinality: i64 = encode_pfcount(&keys)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("PFCOUNT", e))?;
            exchange.input.body = Body::Json(serde_json::json!(cardinality));
        }
        RedisCommand::Pfmerge => {
            let dest = super::require_str_header(exchange, "CamelRedis.Destination")?;
            let keys = resolve_pf_keys(exchange, "PFMERGE")?;
            let _: () = encode_pfmerge(dest, &keys)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("PFMERGE", e))?;
            exchange.input.body = Body::Text("OK".to_string());
        }
        _ => unreachable!("non-HyperLogLog commands rejected above"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use camel_component_api::{Body, Exchange, Message};

    fn packed_args(cmd: &redis::Cmd) -> Vec<String> {
        cmd.args_iter()
            .filter_map(|arg| match arg {
                redis::Arg::Simple(bytes) => String::from_utf8(bytes.to_vec()).ok(),
                _ => None,
            })
            .collect()
    }

    /// `PFADD` takes its element from the body (ADR-0098): a UTF-8 text body
    /// travels as its raw bytes, never as a header payload.
    #[test]
    fn test_pfadd_uses_body_element_wire_frame() {
        let mut msg = Message::new(Body::Text("café".into()));
        msg.set_header("CamelRedis.Key", serde_json::json!("k"));
        let exchange = Exchange::new(msg);

        let key = crate::commands::require_key(&exchange).expect("a key header must resolve");
        let element = crate::commands::body_value_bytes(&exchange, "PFADD", &[crate::HEADER_VALUE])
            .expect("a text body is a valid PFADD element");
        let command = super::encode_pfadd(&key, &element);

        assert_eq!(packed_args(&command), vec!["PFADD", "k", "café"]);
    }

    #[test]
    fn test_pfcount_multi_key_wire_frame() {
        let keys = ["h1".to_string(), "h2".to_string()];
        let command = super::encode_pfcount(&keys);

        assert_eq!(packed_args(&command), vec!["PFCOUNT", "h1", "h2"]);
    }

    #[test]
    fn test_pfmerge_wire_frame() {
        let keys = ["h1".to_string(), "h2".to_string()];
        let command = super::encode_pfmerge("h3", &keys);

        assert_eq!(packed_args(&command), vec!["PFMERGE", "h3", "h1", "h2"]);
    }
}
