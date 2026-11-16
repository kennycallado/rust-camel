use crate::config::RedisCommand;
use camel_component_api::{Body, CamelError, Exchange};
use redis::aio::MultiplexedConnection;

pub(crate) mod bitfield;

/// Largest bit offset Redis accepts: a bitstring is capped at 512 MB, so the
/// highest addressable bit is `512 * 1024 * 1024 * 8 - 1`.
pub(crate) const BIT_OFFSET_MAX: u64 = 4_294_967_295;

pub(crate) fn is_bitmap_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Setbit
            | RedisCommand::Getbit
            | RedisCommand::Bitcount
            | RedisCommand::Bitpos
            | RedisCommand::Bitop
            | RedisCommand::Bitfield
            | RedisCommand::BitfieldRo
    )
}

/// Resolves the `CamelRedis.BitOffset` bit position.
///
/// `SETBIT` and `GETBIT` have no safe default offset: writing bit `0` would
/// mutate the wrong bit, so a missing header is a named error. A present
/// header must be a `u64` within the 512 MB bitstring cap; an offset above
/// `4294967295` fails locally instead of reaching a server-side error.
pub(crate) fn resolve_bit_offset(exchange: &Exchange) -> Result<u64, CamelError> {
    const HEADER: &str = "CamelRedis.BitOffset";
    let offset = super::get_u64_header(exchange, HEADER)?
        .ok_or_else(|| CamelError::ProcessorError(format!("Missing required header: {HEADER}")))?;
    if offset > BIT_OFFSET_MAX {
        return Err(CamelError::ProcessorError(format!(
            "Invalid {HEADER}: {offset} exceeds the 512 MB bitstring cap ({BIT_OFFSET_MAX})"
        )));
    }
    Ok(offset)
}

/// Resolves the `CamelRedis.BitValue` bit to write.
///
/// Redis accepts only `0` or `1`; the bit value is an argument header, never
/// a body payload (ADR-0098). A missing header or any other value fails
/// closed with a named error.
pub(crate) fn resolve_bit_value(exchange: &Exchange) -> Result<u8, CamelError> {
    const HEADER: &str = "CamelRedis.BitValue";
    match super::get_u64_header(exchange, HEADER)? {
        Some(0) => Ok(0),
        Some(1) => Ok(1),
        Some(other) => Err(CamelError::ProcessorError(format!(
            "Invalid value for header {HEADER}: expected 0 or 1, got {other}"
        ))),
        None => Err(CamelError::ProcessorError(format!(
            "Missing required header: {HEADER}"
        ))),
    }
}

/// Which unit a `BITCOUNT`/`BITPOS` range is expressed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitRangeUnit {
    /// `BYTE`: the range counts bytes, the Redis default.
    Byte,
    /// `BIT`: the range counts bits.
    Bit,
}

impl BitRangeUnit {
    /// The wire token for this unit.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            BitRangeUnit::Byte => "BYTE",
            BitRangeUnit::Bit => "BIT",
        }
    }
}

/// A resolved `BITCOUNT`/`BITPOS` range over the bitstring.
///
/// `end` is `None` only for `BITPOS`, the single command that accepts a bare
/// start token; `BITCOUNT` always carries both endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BitRange {
    /// First index of the range; negative values count from the end.
    pub(crate) start: i64,
    /// Last index of the range; negative values count from the end.
    pub(crate) end: Option<i64>,
    /// Unit the indexes are expressed in.
    pub(crate) unit: BitRangeUnit,
}

/// `CamelRedis.Start` — first index of the range.
const START_HEADER: &str = "CamelRedis.Start";
/// `CamelRedis.End` — last index of the range.
const END_HEADER: &str = "CamelRedis.End";
/// `CamelRedis.BitRangeUnit` — `BYTE` (default) or `BIT`.
const UNIT_HEADER: &str = "CamelRedis.BitRangeUnit";

/// Named error for a present-but-malformed `CamelRedis.BitRangeUnit`.
fn invalid_bit_range_unit(value: &serde_json::Value) -> CamelError {
    CamelError::ProcessorError(format!(
        "Invalid value for header {UNIT_HEADER}: expected BYTE or BIT, got {}",
        super::invalid_header_value_preview(value)
    ))
}

/// Resolves `CamelRedis.BitRangeUnit`, keeping absent and malformed distinct.
///
/// `get_str_header` collapses a non-string value to `None`, which would let a
/// present-but-malformed unit fall back to the documented `BYTE` default
/// silently. Here an absent header is `Ok(None)` (the caller applies
/// `BYTE`), while a non-string or unknown token is a named error before send
/// (ADR-0032).
fn resolve_bit_range_unit(exchange: &Exchange) -> Result<Option<BitRangeUnit>, CamelError> {
    let Some(value) = exchange.input.header(UNIT_HEADER) else {
        return Ok(None);
    };
    let serde_json::Value::String(token) = value else {
        return Err(invalid_bit_range_unit(value));
    };
    let token = token.trim();
    if token.eq_ignore_ascii_case(BitRangeUnit::Byte.as_str()) {
        Ok(Some(BitRangeUnit::Byte))
    } else if token.eq_ignore_ascii_case(BitRangeUnit::Bit.as_str()) {
        Ok(Some(BitRangeUnit::Bit))
    } else {
        Err(invalid_bit_range_unit(value))
    }
}

/// Named error for a one-sided range.
fn bit_range_endpoints_error(verb: &str, present: &str) -> CamelError {
    CamelError::ProcessorError(format!(
        "{verb} requires both {START_HEADER} and {END_HEADER} together: a range needs both endpoints, got only {present}"
    ))
}

/// Named error for a unit header without a complete range.
fn bit_range_unit_without_full_range_error(verb: &str) -> CamelError {
    CamelError::ProcessorError(format!(
        "{verb}: {UNIT_HEADER} requires both endpoints ({START_HEADER} and {END_HEADER}); a unit applies only to a complete range"
    ))
}

/// Resolves the optional `BITCOUNT`/`BITPOS` range from the argument headers.
///
/// `CamelRedis.Start`/`CamelRedis.End` are signed (`i64`): a negative index
/// counts from the end of the bitstring, matching Redis. The range grammar is
/// Redis 7:
///
/// - `BITCOUNT` requires both endpoints together;
/// - `BITPOS` accepts a bare start, but `CamelRedis.BitRangeUnit` is
///   meaningful only for a complete range, and `End` always requires `Start`.
///
/// A missing `CamelRedis.BitRangeUnit` uses the documented `BYTE` default; a
/// present-but-malformed value fails closed in
/// [`resolve_bit_range_unit`]. Every grammar violation is a named error
/// before send (ADR-0032).
pub(crate) fn resolve_bit_range(
    exchange: &Exchange,
    verb: &str,
) -> Result<Option<BitRange>, CamelError> {
    let start = super::get_i64_header(exchange, START_HEADER)?;
    let end = super::get_i64_header(exchange, END_HEADER)?;
    let unit = resolve_bit_range_unit(exchange)?;

    match (start, end) {
        (Some(start), Some(end)) => Ok(Some(BitRange {
            start,
            end: Some(end),
            unit: unit.unwrap_or(BitRangeUnit::Byte),
        })),
        // BITPOS is the only range command that accepts a bare start token.
        (Some(start), None) if verb == "BITPOS" => {
            if unit.is_some() {
                Err(bit_range_unit_without_full_range_error(verb))
            } else {
                Ok(Some(BitRange {
                    start,
                    end: None,
                    unit: BitRangeUnit::Byte,
                }))
            }
        }
        (Some(_), None) | (None, Some(_)) => {
            let present = if start.is_some() {
                START_HEADER
            } else {
                END_HEADER
            };
            Err(bit_range_endpoints_error(verb, present))
        }
        (None, None) => match unit {
            None => Ok(None),
            Some(_) => Err(bit_range_unit_without_full_range_error(verb)),
        },
    }
}

/// Resolves the `CamelRedis.BitValue` bit `BITPOS` searches for (`0` or `1`).
///
/// Same rule as [`resolve_bit_value`]: the header is required and accepts only
/// a binary value.
pub(crate) fn resolve_bit_pos_value(exchange: &Exchange) -> Result<u8, CamelError> {
    resolve_bit_value(exchange)
}

/// Builds `SETBIT key offset value`.
pub(crate) fn encode_setbit(key: &str, offset: u64, value: u8) -> redis::Cmd {
    let mut command = redis::cmd("SETBIT");
    command.arg(key).arg(offset).arg(value);
    command
}

/// Builds `GETBIT key offset`.
pub(crate) fn encode_getbit(key: &str, offset: u64) -> redis::Cmd {
    let mut command = redis::cmd("GETBIT");
    command.arg(key).arg(offset);
    command
}

/// Builds `BITCOUNT key [start end unit]`.
///
/// A resolved range always carries both endpoints, and the unit token is
/// emitted explicitly even for the Redis default `BYTE`, so the wire frame
/// states the unit it counted in.
pub(crate) fn encode_bitcount(key: &str, range: Option<&BitRange>) -> redis::Cmd {
    let mut command = redis::cmd("BITCOUNT");
    command.arg(key);
    if let Some(range) = range
        && let Some(end) = range.end
    {
        // `resolve_bit_range` rejects a start-only range for BITCOUNT, so this
        // arm always has both endpoints; the guard keeps the encoder panic-free.
        command.arg(range.start).arg(end).arg(range.unit.as_str());
    }
    command
}

/// Builds `BITPOS key value [start [end unit]]`.
///
/// A start-only range emits no unit token: Redis reads a unit only after a
/// complete range, and [`resolve_bit_range`] already rejects that combination.
pub(crate) fn encode_bitpos(key: &str, value: u8, range: Option<&BitRange>) -> redis::Cmd {
    let mut command = redis::cmd("BITPOS");
    command.arg(key).arg(value);
    if let Some(range) = range {
        command.arg(range.start);
        if let Some(end) = range.end {
            command.arg(end).arg(range.unit.as_str());
        }
    }
    command
}

/// Builds `BITOP <AND|OR|XOR|NOT> <dest> <key>...`.
pub(crate) fn encode_bitop(op: BitOpOperation, dest: &str, keys: &[String]) -> redis::Cmd {
    let mut command = redis::cmd("BITOP");
    command.arg(op.as_str()).arg(dest);
    for key in keys {
        command.arg(key);
    }
    command
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitOpOperation {
    /// `AND`: bitwise AND of all sources.
    And,
    /// `OR`: bitwise OR of all sources.
    Or,
    /// `XOR`: bitwise XOR of all sources.
    Xor,
    /// `NOT`: bitwise complement of the single source.
    Not,
}

impl BitOpOperation {
    /// The wire token for this operation.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            BitOpOperation::And => "AND",
            BitOpOperation::Or => "OR",
            BitOpOperation::Xor => "XOR",
            BitOpOperation::Not => "NOT",
        }
    }

    /// Parses a `CamelRedis.BitOperation` value, ignoring case and surrounding
    /// whitespace. Returns `None` for any value outside the four operations.
    fn parse(token: &str) -> Option<BitOpOperation> {
        let token = token.trim();
        [
            BitOpOperation::And,
            BitOpOperation::Or,
            BitOpOperation::Xor,
            BitOpOperation::Not,
        ]
        .into_iter()
        .find(|op| token.eq_ignore_ascii_case(op.as_str()))
    }
}

/// Named error for a present-but-malformed `CamelRedis.BitOperation`.
fn invalid_bit_operation(value: &serde_json::Value) -> CamelError {
    CamelError::ProcessorError(format!(
        "Invalid value for header CamelRedis.BitOperation: expected AND, OR, XOR, or NOT, got {}",
        super::invalid_header_value_preview(value)
    ))
}

/// Resolves the required `CamelRedis.BitOperation` aggregation.
///
/// A missing header is a named missing-header error. A present-but-malformed
/// value (an unknown token such as `NAND`, or a non-string such as `42`) is a
/// named error listing the four valid operations; it is never treated as
/// absent (ADR-0032).
pub(crate) fn resolve_bit_operation(exchange: &Exchange) -> Result<BitOpOperation, CamelError> {
    const HEADER: &str = "CamelRedis.BitOperation";
    let Some(value) = exchange.input.header(HEADER) else {
        return Err(CamelError::ProcessorError(format!(
            "Missing required header: {HEADER}"
        )));
    };
    let serde_json::Value::String(token) = value else {
        return Err(invalid_bit_operation(value));
    };
    BitOpOperation::parse(token).ok_or_else(|| invalid_bit_operation(value))
}

/// Resolves `CamelRedis.Destination`, the key `BITOP` writes.
///
/// A non-string value is a named error rather than a missing header: treating
/// it as absent would report the wrong cause and hide the misconfigured route
/// (ADR-0032).
fn resolve_bitop_destination(exchange: &Exchange) -> Result<String, CamelError> {
    const HEADER: &str = "CamelRedis.Destination";
    match exchange.input.header(HEADER) {
        None => Err(CamelError::ProcessorError(format!(
            "Missing required header: {HEADER}"
        ))),
        Some(serde_json::Value::String(dest)) => Ok(dest.clone()),
        Some(other) => Err(CamelError::ProcessorError(format!(
            "Invalid value for header {HEADER}: expected a string, got {}",
            super::invalid_header_value_preview(other)
        ))),
    }
}

/// Resolves the `CamelRedis.Keys` sources and enforces the operation arity.
///
/// The SINTERSTORE convention applies: `NOT` takes exactly one source and
/// `AND`/`OR`/`XOR` take one or more. The strict list resolver rejects a
/// missing header, an empty list, a non-list value, and a non-string entry, so
/// the command never runs against fewer sources than the route configured
/// (ADR-0032). Every violation is a named error before send.
fn resolve_bitop_keys(exchange: &Exchange, op: BitOpOperation) -> Result<Vec<String>, CamelError> {
    let keys = super::require_string_list(exchange, "CamelRedis.Keys", "BITOP", "key")?;
    if op == BitOpOperation::Not && keys.len() != 1 {
        return Err(CamelError::ProcessorError(format!(
            "BITOP: invalid CamelRedis.Keys: NOT requires exactly one source key, got {}",
            keys.len()
        )));
    }
    Ok(keys)
}

/// Dispatches the bitmap commands.
///
/// `SETBIT` replies with the previous bit and `GETBIT` with the current bit;
/// `BITCOUNT` counts the set bits and `BITPOS` reports the first matching bit
/// (`-1` when it is never set). `BITOP` replies with the destination's new
/// length in bytes. `BITFIELD` and `BITFIELD_RO` reply with an array of
/// integers and nils. Every reply maps to the exchange body as a JSON value.
pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    if !is_bitmap_command(cmd) {
        return Err(CamelError::ProcessorError("Not a bitmap command".into()));
    }

    let result: serde_json::Value = match cmd {
        RedisCommand::Setbit => {
            let key = super::require_key(exchange)?;
            let offset = resolve_bit_offset(exchange)?;
            let value = resolve_bit_value(exchange)?;
            let previous: i64 = encode_setbit(&key, offset, value)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("SETBIT", e))?;
            serde_json::json!(previous)
        }
        RedisCommand::Getbit => {
            let key = super::require_key(exchange)?;
            let offset = resolve_bit_offset(exchange)?;
            let bit: i64 = encode_getbit(&key, offset)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("GETBIT", e))?;
            serde_json::json!(bit)
        }
        RedisCommand::Bitcount => {
            let key = super::require_key(exchange)?;
            let range = resolve_bit_range(exchange, "BITCOUNT")?;
            let count: i64 = encode_bitcount(&key, range.as_ref())
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("BITCOUNT", e))?;
            serde_json::json!(count)
        }
        RedisCommand::Bitpos => {
            let key = super::require_key(exchange)?;
            let value = resolve_bit_pos_value(exchange)?;
            let range = resolve_bit_range(exchange, "BITPOS")?;
            let position: i64 = encode_bitpos(&key, value, range.as_ref())
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("BITPOS", e))?;
            serde_json::json!(position)
        }
        RedisCommand::Bitop => {
            let op = resolve_bit_operation(exchange)?;
            let dest = resolve_bitop_destination(exchange)?;
            let keys = resolve_bitop_keys(exchange, op)?;
            let length: i64 = encode_bitop(op, &dest, &keys)
                .query_async(conn)
                .await
                .map_err(|e| crate::transport_error::redis_error_to_camel("BITOP", e))?;
            serde_json::json!(length)
        }
        RedisCommand::Bitfield | RedisCommand::BitfieldRo => {
            let read_only = matches!(cmd, RedisCommand::BitfieldRo);
            let verb = if read_only { "BITFIELD_RO" } else { "BITFIELD" };
            let key = super::require_key(exchange)?;
            let initial = bitfield::resolve_bitfield_overflow(exchange)?;
            let raw =
                super::require_string_list(exchange, "CamelRedis.BitFieldOps", verb, "operation")?;
            let entries = bitfield::parse_bitfield_ops(&raw)?;
            bitfield::validate_bitfield_overflow(&entries, initial, read_only)?;
            let reply: Vec<Option<i64>> =
                bitfield::encode_bitfield(&key, initial, &entries, read_only)
                    .query_async(conn)
                    .await
                    .map_err(|e| crate::transport_error::redis_error_to_camel(verb, e))?;
            exchange.input.body = bitfield::bitfield_reply_to_body(reply);
            return Ok(());
        }
        _ => unreachable!("non-bitmap commands rejected above"),
    };
    exchange.input.body = Body::Json(result);
    Ok(())
}

#[cfg(test)]
mod tests {
    fn packed_args(cmd: &redis::Cmd) -> Vec<String> {
        cmd.args_iter()
            .filter_map(|arg| match arg {
                redis::Arg::Simple(bytes) => String::from_utf8(bytes.to_vec()).ok(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_setbit_encodes_wire_frame() {
        let command = super::encode_setbit("k", 7, 1);
        assert_eq!(packed_args(&command), vec!["SETBIT", "k", "7", "1"]);
    }

    #[test]
    fn test_getbit_encodes_wire_frame() {
        let command = super::encode_getbit("k", 7);
        assert_eq!(packed_args(&command), vec!["GETBIT", "k", "7"]);
    }

    // ── redisbmp task 1.2: BITCOUNT / BITPOS wire frames ─────────────────────

    #[test]
    fn test_bitcount_encodes_range_with_explicit_byte_unit() {
        let command = super::encode_bitcount(
            "k",
            Some(&super::BitRange {
                start: 0,
                end: Some(2),
                unit: super::BitRangeUnit::Byte,
            }),
        );
        assert_eq!(
            packed_args(&command),
            vec!["BITCOUNT", "k", "0", "2", "BYTE"]
        );
    }

    #[test]
    fn test_bitcount_encodes_bit_unit() {
        let command = super::encode_bitcount(
            "k",
            Some(&super::BitRange {
                start: 0,
                end: Some(2),
                unit: super::BitRangeUnit::Bit,
            }),
        );
        assert_eq!(
            packed_args(&command),
            vec!["BITCOUNT", "k", "0", "2", "BIT"]
        );
    }

    #[test]
    fn test_bitpos_encodes_start_only_without_unit() {
        let command = super::encode_bitpos(
            "k",
            1,
            Some(&super::BitRange {
                start: 5,
                end: None,
                unit: super::BitRangeUnit::Byte,
            }),
        );
        assert_eq!(packed_args(&command), vec!["BITPOS", "k", "1", "5"]);
    }

    #[test]
    fn test_bitpos_negative_range_indexes_allowed() {
        let command = super::encode_bitpos(
            "k",
            1,
            Some(&super::BitRange {
                start: -1,
                end: Some(-1),
                unit: super::BitRangeUnit::Byte,
            }),
        );
        assert_eq!(
            packed_args(&command),
            vec!["BITPOS", "k", "1", "-1", "-1", "BYTE"]
        );
    }

    // ── redisbmp task 1.3: BITOP wire frame ─────────────────────────────────

    #[test]
    fn test_bitop_or_encodes_wire_frame() {
        let keys = ["a".to_string(), "b".to_string(), "c".to_string()];
        let command = super::encode_bitop(super::BitOpOperation::Or, "d", &keys);
        assert_eq!(
            packed_args(&command),
            vec!["BITOP", "OR", "d", "a", "b", "c"]
        );
    }
}
