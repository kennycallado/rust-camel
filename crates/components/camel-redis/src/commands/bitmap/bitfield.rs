use camel_component_api::Body;
use camel_component_api::CamelError;
use camel_component_api::Exchange;

/// Bounded rendering of an untrusted token for a diagnostic.
///
/// `CamelRedis.BitFieldOps` values are attacker-sized (ADR-0032), so every
/// parser error echoes a 64-scalar preview, never the full token.
fn bounded_entry(entry: &str) -> String {
    crate::commands::invalid_header_value_preview(&serde_json::Value::String(entry.to_string()))
}

/// Which overflow mode applies to `BITFIELD` mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitFieldOverflow {
    /// `WRAP`: wrap around on overflow.
    Wrap,
    /// `SAT`: saturate at the type's bounds.
    Sat,
    /// `FAIL`: fail the sub-command on overflow.
    Fail,
}

impl BitFieldOverflow {
    /// The wire token for this mode.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            BitFieldOverflow::Wrap => "WRAP",
            BitFieldOverflow::Sat => "SAT",
            BitFieldOverflow::Fail => "FAIL",
        }
    }

    /// Parses an overflow token, ignoring case and surrounding whitespace.
    fn parse(text: &str) -> Option<BitFieldOverflow> {
        let text = text.trim();
        [
            BitFieldOverflow::Wrap,
            BitFieldOverflow::Sat,
            BitFieldOverflow::Fail,
        ]
        .into_iter()
        .find(|mode| text.eq_ignore_ascii_case(mode.as_str()))
    }
}

/// Named error for a malformed `BITFIELD` entry.
fn bitfield_ops_error(detail: impl std::fmt::Display) -> CamelError {
    CamelError::ProcessorError(format!("CamelRedis.BitFieldOps: {detail}"))
}

/// Named error for a present-but-malformed `CamelRedis.BitFieldOverflow`.
fn invalid_bitfield_overflow(value: &serde_json::Value) -> CamelError {
    CamelError::ProcessorError(format!(
        "Invalid value for header CamelRedis.BitFieldOverflow: expected WRAP, SAT, or FAIL, got {}",
        crate::commands::invalid_header_value_preview(value)
    ))
}

/// A `BITFIELD` sub-command encoding: a signed or unsigned integer width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BitFieldType {
    /// `true` for `i<width>`, `false` for `u<width>`.
    pub(crate) signed: bool,
    /// Width in bits.
    pub(crate) width: u8,
}

impl BitFieldType {
    /// The wire token, e.g. `i16` or `u8`.
    fn as_token(&self) -> String {
        format!("{}{}", if self.signed { 'i' } else { 'u' }, self.width)
    }
}

/// Parses `[iu]<width>`: signed widths 1..=64, unsigned widths 1..=63.
///
/// `u64` is rejected with the named rule "u64 is not supported but i64 is":
/// the unsigned encoding cannot represent the sign bit Redis reserves.
pub(crate) fn parse_bitfield_type(entry: &str) -> Result<BitFieldType, CamelError> {
    let entry = entry.trim();
    let (signed, digits) = if let Some(digits) = entry.strip_prefix('i') {
        (true, digits)
    } else if let Some(digits) = entry.strip_prefix('u') {
        (false, digits)
    } else {
        return Err(bitfield_ops_error(format!(
            "invalid type {}: expected [iu]<width>",
            bounded_entry(entry)
        )));
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(bitfield_ops_error(format!(
            "invalid type {}: expected [iu]<width>",
            bounded_entry(entry)
        )));
    }
    let width: u8 = digits.parse().map_err(|_| {
        bitfield_ops_error(format!(
            "invalid type {}: width {} is out of range",
            bounded_entry(entry),
            bounded_entry(digits)
        ))
    })?;
    let valid = if signed {
        (1..=64).contains(&width)
    } else {
        (1..=63).contains(&width)
    };
    if !valid {
        if !signed && width == 64 {
            return Err(bitfield_ops_error(
                "u64 is not supported but i64 is; use i64 for 64-bit values",
            ));
        }
        return Err(bitfield_ops_error(format!(
            "invalid type {}: signed widths are 1..=64 and unsigned widths are 1..=63",
            bounded_entry(entry)
        )));
    }
    Ok(BitFieldType { signed, width })
}

/// A `BITFIELD` offset: an absolute bit position or a `#<n>` width multiple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitFieldOffset {
    /// Absolute bit position from the start of the bitstring.
    Absolute(u64),
    /// `#<n>`: `n × <this entry's width>` bits from the start.
    Sharp(u64),
}

impl BitFieldOffset {
    /// The wire token, sent back exactly as validated.
    fn as_token(&self) -> String {
        match self {
            BitFieldOffset::Absolute(n) => n.to_string(),
            BitFieldOffset::Sharp(n) => format!("#{n}"),
        }
    }

    /// Resolves this offset to a bit position for the given entry width.
    ///
    /// An absolute offset must fit `i64`, the Redis parsing bound. A `#<n>`
    /// offset multiplies *this entry's own width*, never a running position,
    /// so two `#1` entries of the same width address the same bits.
    pub(crate) fn bit_offset(&self, width: u8) -> Result<u64, CamelError> {
        match self {
            BitFieldOffset::Absolute(n) => {
                if *n > i64::MAX as u64 {
                    Err(bitfield_ops_error(format!(
                        "offset {n} exceeds i64::MAX ({}); Redis cannot parse it",
                        i64::MAX
                    )))
                } else {
                    Ok(*n)
                }
            }
            BitFieldOffset::Sharp(n) => {
                let product = n.checked_mul(u64::from(width)).ok_or_else(|| {
                    bitfield_ops_error(format!(
                        "offset #{n} × width {width} overflows the bit offset type"
                    ))
                })?;
                if product > i64::MAX as u64 {
                    Err(bitfield_ops_error(format!(
                        "offset #{n} × width {width} = {product} exceeds i64::MAX ({}); Redis cannot parse it",
                        i64::MAX
                    )))
                } else {
                    Ok(product)
                }
            }
        }
    }
}

/// One parsed `BITFIELD` sub-command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitFieldEntry {
    /// `OVERFLOW <mode>`: switches the effective mode for following entries.
    Overflow(BitFieldOverflow),
    /// `GET <type> <offset>`.
    Get {
        /// Encoding of the value read.
        ty: BitFieldType,
        /// Bit position read from.
        offset: BitFieldOffset,
    },
    /// `SET <type> <offset> <value>`.
    Set {
        /// Encoding of the value written.
        ty: BitFieldType,
        /// Bit position written to.
        offset: BitFieldOffset,
        /// Value written.
        value: i64,
    },
    /// `INCRBY <type> <offset> <increment>`.
    Incrby {
        /// Encoding of the value incremented.
        ty: BitFieldType,
        /// Bit position incremented.
        offset: BitFieldOffset,
        /// Signed increment applied.
        increment: i64,
    },
}

/// Parses and validates `CamelRedis.BitFieldOps` entries.
///
/// Each list element is one sub-command, tokenized on whitespace. Every
/// grammar violation is a named `CamelRedis.BitFieldOps` error before send.
/// Offsets are resolved here only to surface `#<n>` multiplication overflow
/// and absolute offsets above `i64::MAX` before any command reaches Redis.
pub(crate) fn parse_bitfield_ops(raw: &[String]) -> Result<Vec<BitFieldEntry>, CamelError> {
    raw.iter()
        .map(|entry| parse_bitfield_entry(entry))
        .collect()
}

/// Parses one `BITFIELD` sub-command entry.
fn parse_bitfield_entry(raw: &str) -> Result<BitFieldEntry, CamelError> {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let Some(op) = tokens.first() else {
        return Err(bitfield_ops_error(
            "empty entry: expected GET, SET, INCRBY, or OVERFLOW",
        ));
    };
    match tokens.len() {
        2 if op.eq_ignore_ascii_case("OVERFLOW") => {
            let mode = BitFieldOverflow::parse(tokens[1]).ok_or_else(|| {
                bitfield_ops_error(format!(
                    "invalid OVERFLOW mode {}: expected WRAP, SAT, or FAIL",
                    bounded_entry(tokens[1])
                ))
            })?;
            Ok(BitFieldEntry::Overflow(mode))
        }
        3 if op.eq_ignore_ascii_case("GET") => {
            let ty = parse_bitfield_type(tokens[1])?;
            let offset = parse_bitfield_offset(tokens[2])?;
            offset.bit_offset(ty.width)?;
            Ok(BitFieldEntry::Get { ty, offset })
        }
        4 if op.eq_ignore_ascii_case("SET") => {
            let ty = parse_bitfield_type(tokens[1])?;
            let offset = parse_bitfield_offset(tokens[2])?;
            offset.bit_offset(ty.width)?;
            let value = parse_bitfield_integer(tokens[3], "value")?;
            Ok(BitFieldEntry::Set { ty, offset, value })
        }
        4 if op.eq_ignore_ascii_case("INCRBY") => {
            let ty = parse_bitfield_type(tokens[1])?;
            let offset = parse_bitfield_offset(tokens[2])?;
            offset.bit_offset(ty.width)?;
            let increment = parse_bitfield_integer(tokens[3], "increment")?;
            Ok(BitFieldEntry::Incrby {
                ty,
                offset,
                increment,
            })
        }
        _ => Err(bitfield_ops_error(format!(
            "invalid entry {}: expected \"GET <type> <offset>\", \"SET <type> <offset> <value>\", \"INCRBY <type> <offset> <increment>\", or \"OVERFLOW <WRAP|SAT|FAIL>\"",
            bounded_entry(raw)
        ))),
    }
}

/// Parses a `#<n>` or absolute bit offset.
fn parse_bitfield_offset(entry: &str) -> Result<BitFieldOffset, CamelError> {
    if let Some(digits) = entry.strip_prefix('#') {
        parse_bitfield_u64(digits, entry).map(BitFieldOffset::Sharp)
    } else {
        parse_bitfield_u64(entry, entry).map(BitFieldOffset::Absolute)
    }
}

/// Parses strict unsigned digits, rejecting signs and out-of-range values.
fn parse_bitfield_u64(digits: &str, entry: &str) -> Result<u64, CamelError> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(bitfield_ops_error(format!(
            "invalid offset {}",
            bounded_entry(entry)
        )));
    }
    digits.parse::<u64>().map_err(|_| {
        bitfield_ops_error(format!(
            "invalid offset {}: out of range",
            bounded_entry(entry)
        ))
    })
}

/// Parses a `SET` value or `INCRBY` increment as a signed 64-bit integer.
fn parse_bitfield_integer(entry: &str, role: &str) -> Result<i64, CamelError> {
    entry.parse().map_err(|_| {
        bitfield_ops_error(format!(
            "invalid {role} {}: expected a signed 64-bit integer",
            bounded_entry(entry)
        ))
    })
}

/// Validates the overflow contract for parsed `BITFIELD` entries.
///
/// Every mutating entry (`SET`/`INCRBY`) needs an effective mode in force
/// before it runs: the `initial` header mode or an earlier `OVERFLOW` entry.
/// There is no default, because the server default `WRAP` silently wraps
/// counters, and a trailing `OVERFLOW` never covers a preceding mutation.
///
/// `read_only` is `BITFIELD_RO`: only `GET` entries are accepted, and the
/// overflow header itself is rejected because the read-only form accepts no
/// `OVERFLOW` directive.
pub(crate) fn validate_bitfield_overflow(
    entries: &[BitFieldEntry],
    initial: Option<BitFieldOverflow>,
    read_only: bool,
) -> Result<(), CamelError> {
    if read_only {
        if initial.is_some() {
            return Err(CamelError::ProcessorError(
                "Invalid CamelRedis.BitFieldOverflow: BITFIELD_RO is read-only and accepts no overflow mode".into(),
            ));
        }
        for entry in entries {
            match entry {
                BitFieldEntry::Get { .. } => {}
                BitFieldEntry::Overflow(_) => {
                    return Err(bitfield_ops_error(
                        "BITFIELD_RO accepts only GET entries: OVERFLOW is not allowed",
                    ));
                }
                BitFieldEntry::Set { .. } | BitFieldEntry::Incrby { .. } => {
                    return Err(bitfield_ops_error(
                        "BITFIELD_RO accepts only GET entries: SET and INCRBY mutate bits",
                    ));
                }
            }
        }
        return Ok(());
    }

    let mut effective = initial;
    for (index, entry) in entries.iter().enumerate() {
        match entry {
            BitFieldEntry::Overflow(mode) => effective = Some(*mode),
            BitFieldEntry::Get { .. } => {}
            BitFieldEntry::Set { .. } | BitFieldEntry::Incrby { .. } => {
                if effective.is_none() {
                    return Err(CamelError::ProcessorError(format!(
                        "Missing CamelRedis.BitFieldOverflow: BITFIELD entry at index {index} mutates bits but no overflow mode is in force; set CamelRedis.BitFieldOverflow or precede the entry with OVERFLOW WRAP|SAT|FAIL"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Resolves the optional `CamelRedis.BitFieldOverflow` initial mode.
///
/// Absent is `Ok(None)`; the dispatcher then requires an inline `OVERFLOW`
/// entry before any mutation. A present-but-malformed value (a non-string such
/// as `42`, or an unknown token such as `SOMETIMES`) is a named error before
/// send, even when an inline `OVERFLOW` entry exists: a present header is
/// never treated as absent (ADR-0032).
pub(crate) fn resolve_bitfield_overflow(
    exchange: &Exchange,
) -> Result<Option<BitFieldOverflow>, CamelError> {
    const HEADER: &str = "CamelRedis.BitFieldOverflow";
    let Some(value) = exchange.input.header(HEADER) else {
        return Ok(None);
    };
    let serde_json::Value::String(text) = value else {
        return Err(invalid_bitfield_overflow(value));
    };
    BitFieldOverflow::parse(text)
        .map(Some)
        .ok_or_else(|| invalid_bitfield_overflow(value))
}

/// Builds `BITFIELD`/`BITFIELD_RO <key> [OVERFLOW <mode>] <ops...>`.
///
/// Offset tokens are sent back exactly as validated (`#<n>` stays `#<n>`), so
/// the server applies the same independent `n × width` rule the validator
/// checked.
pub(crate) fn encode_bitfield(
    key: &str,
    initial: Option<BitFieldOverflow>,
    entries: &[BitFieldEntry],
    read_only: bool,
) -> redis::Cmd {
    let mut command = if read_only {
        redis::cmd("BITFIELD_RO")
    } else {
        redis::cmd("BITFIELD")
    };
    command.arg(key);
    if let Some(mode) = initial {
        command.arg("OVERFLOW").arg(mode.as_str());
    }
    for entry in entries {
        match entry {
            BitFieldEntry::Overflow(mode) => {
                command.arg("OVERFLOW").arg(mode.as_str());
            }
            BitFieldEntry::Get { ty, offset } => {
                command.arg("GET").arg(ty.as_token()).arg(offset.as_token());
            }
            BitFieldEntry::Set { ty, offset, value } => {
                command
                    .arg("SET")
                    .arg(ty.as_token())
                    .arg(offset.as_token())
                    .arg(*value);
            }
            BitFieldEntry::Incrby {
                ty,
                offset,
                increment,
            } => {
                command
                    .arg("INCRBY")
                    .arg(ty.as_token())
                    .arg(offset.as_token())
                    .arg(*increment);
            }
        }
    }
    command
}

/// Maps a `BITFIELD` reply to the exchange body as a JSON array of numbers and
/// nulls.
pub(crate) fn bitfield_reply_to_body(values: Vec<Option<i64>>) -> Body {
    Body::Json(serde_json::json!(values))
}

#[cfg(test)]
mod tests {
    use camel_component_api::Body;

    fn packed_args(cmd: &redis::Cmd) -> Vec<String> {
        cmd.args_iter()
            .filter_map(|arg| match arg {
                redis::Arg::Simple(bytes) => String::from_utf8(bytes.to_vec()).ok(),
                _ => None,
            })
            .collect()
    }

    fn get_entry_offsets(entries: &[super::BitFieldEntry]) -> Vec<u64> {
        entries
            .iter()
            .map(|entry| match entry {
                super::BitFieldEntry::Get { ty, offset } => offset
                    .bit_offset(ty.width)
                    .expect("test offsets fit the offset type"),
                other => panic!("expected a GET entry, got {other:?}"),
            })
            .collect()
    }

    fn parse(raw: &[&str]) -> Vec<super::BitFieldEntry> {
        let raw: Vec<String> = raw.iter().map(|entry| (*entry).to_string()).collect();
        super::parse_bitfield_ops(&raw).expect("test entries must parse")
    }

    #[test]
    fn test_parse_bitfield_type_accepts_i64_and_u63_rejects_u64() {
        assert_eq!(
            super::parse_bitfield_type("i64").expect("i64 must parse"),
            super::BitFieldType {
                signed: true,
                width: 64
            }
        );
        assert_eq!(
            super::parse_bitfield_type("u63").expect("u63 must parse"),
            super::BitFieldType {
                signed: false,
                width: 63
            }
        );

        let err = super::parse_bitfield_type("u64").expect_err("u64 must be rejected");
        assert!(
            err.to_string().contains("u64 is not supported but i64 is"),
            "u64 rejection must name the rule: {err}"
        );

        for text in ["x8", "u0", "u65", "i65"] {
            let err = super::parse_bitfield_type(text)
                .expect_err("an invalid bitfield type must be rejected");
            assert!(
                err.to_string().contains("CamelRedis.BitFieldOps"),
                "{text} rejection must name CamelRedis.BitFieldOps: {err}"
            );
        }
    }

    #[test]
    fn test_bitfield_sharp_offset_multiplies_entry_width() {
        let entries = parse(&["GET u8 #1", "GET i16 #1"]);
        assert_eq!(
            get_entry_offsets(&entries),
            vec![8, 16],
            "#n must multiply each entry's own width"
        );
    }

    #[test]
    fn test_bitfield_repeated_sharp_offsets_address_same_bits() {
        let entries = parse(&["GET u8 #1", "GET u8 #1"]);
        assert_eq!(
            get_entry_offsets(&entries),
            vec![8, 8],
            "each #n is evaluated independently, never accumulated"
        );
    }

    #[test]
    fn test_bitfield_rejects_offset_multiplication_overflow() {
        let raw = ["INCRBY i64 #9223372036854775807 1".to_string()];
        let err = super::parse_bitfield_ops(&raw)
            .expect_err("an #n offset multiplication overflow must be refused before send");
        assert!(
            err.to_string().contains("CamelRedis.BitFieldOps"),
            "offset overflow must name CamelRedis.BitFieldOps: {err}"
        );
    }

    #[test]
    fn test_bitfield_requires_explicit_overflow_before_mutation() {
        let entries = parse(&["INCRBY u8 #0 1"]);
        let err = super::validate_bitfield_overflow(&entries, None, false)
            .expect_err("a mutation without an effective overflow mode must be refused");
        assert!(
            err.to_string().contains("CamelRedis.BitFieldOverflow"),
            "the error must name CamelRedis.BitFieldOverflow: {err}"
        );
    }

    #[test]
    fn test_bitfield_trailing_overflow_does_not_cover_mutation() {
        let entries = parse(&["INCRBY u8 0 1", "OVERFLOW SAT"]);
        let err = super::validate_bitfield_overflow(&entries, None, false)
            .expect_err("a trailing OVERFLOW must not cover a preceding mutation");
        assert!(
            err.to_string().contains("CamelRedis.BitFieldOverflow"),
            "the error must name CamelRedis.BitFieldOverflow: {err}"
        );
    }

    #[test]
    fn test_bitfield_ro_rejects_mutations_and_overflow_header() {
        let set = parse(&["SET u8 0 1"]);
        let err = super::validate_bitfield_overflow(&set, None, true)
            .expect_err("BITFIELD_RO must reject a SET entry");
        assert!(
            err.to_string().contains("CamelRedis.BitFieldOps"),
            "a SET entry in BITFIELD_RO must name CamelRedis.BitFieldOps: {err}"
        );

        let overflow = parse(&["OVERFLOW SAT"]);
        let err = super::validate_bitfield_overflow(&overflow, None, true)
            .expect_err("BITFIELD_RO must reject an OVERFLOW entry");
        assert!(
            err.to_string().contains("CamelRedis.BitFieldOps"),
            "an OVERFLOW entry in BITFIELD_RO must name CamelRedis.BitFieldOps: {err}"
        );

        let get = parse(&["GET u8 0"]);
        let err =
            super::validate_bitfield_overflow(&get, Some(super::BitFieldOverflow::Wrap), true)
                .expect_err("BITFIELD_RO must reject a present overflow header");
        assert!(
            err.to_string().contains("CamelRedis.BitFieldOverflow"),
            "an overflow header in BITFIELD_RO must name CamelRedis.BitFieldOverflow: {err}"
        );
    }

    #[test]
    fn test_bitfield_overflow_validation_accepts_effective_modes() {
        // (a) An initial header mode covers every mutation in the list.
        let entries = parse(&["SET u8 0 1", "INCRBY u8 #0 1", "GET u8 0"]);
        super::validate_bitfield_overflow(&entries, Some(super::BitFieldOverflow::Sat), false)
            .expect("an initial overflow header must cover all mutations");

        // (b) A preceding inline OVERFLOW entry covers a later mutation.
        let entries = parse(&["OVERFLOW SAT", "INCRBY u8 #0 1"]);
        super::validate_bitfield_overflow(&entries, None, false)
            .expect("a preceding OVERFLOW SAT must cover the INCRBY");

        // (c) Inline entries switch the mode for their following mutation.
        let entries = parse(&[
            "OVERFLOW WRAP",
            "SET u8 0 1",
            "OVERFLOW FAIL",
            "INCRBY u8 #0 1",
        ]);
        super::validate_bitfield_overflow(&entries, None, false)
            .expect("each inline OVERFLOW must cover the mutation that follows it");

        // (d) GET-only entries need no mode at all.
        let entries = parse(&["GET u8 0", "GET i16 #1"]);
        super::validate_bitfield_overflow(&entries, None, false)
            .expect("GET entries need no overflow mode");
    }

    #[test]
    fn test_parse_bitfield_rejects_malformed_entries() {
        for entry in ["FOO u8 0", "GET u8", "GET u8 abc", "SET u8 0 notanint"] {
            let raw = [entry.to_string()];
            let err = super::parse_bitfield_ops(&raw)
                .expect_err("a malformed BITFIELD entry must be rejected");
            assert!(
                err.to_string().contains("CamelRedis.BitFieldOps"),
                "{entry} rejection must name CamelRedis.BitFieldOps: {err}"
            );
        }
    }

    #[test]
    fn test_bitfield_error_bounds_oversized_token() {
        let huge = "9".repeat(4096);
        let raw = [format!("GET u8 {huge}")];
        let err =
            super::parse_bitfield_ops(&raw).expect_err("an oversized offset must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("CamelRedis.BitFieldOps"),
            "an oversized text must still be a named error: {msg}"
        );
        assert!(
            !msg.contains(&huge),
            "the full untrusted text must not appear in the diagnostic"
        );
        assert!(
            msg.len() < 512,
            "the diagnostic must stay bounded, got {} chars",
            msg.len()
        );
    }

    #[test]
    fn test_bitfield_error_handles_unicode_token() {
        let raw = [format!("FÖÖ {} u8", "é".repeat(200))];
        let err = super::parse_bitfield_ops(&raw)
            .expect_err("a malformed Unicode entry must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("CamelRedis.BitFieldOps"),
            "a Unicode text must still be a named error: {msg}"
        );
    }

    #[test]
    fn test_bitfield_wire_frame_emits_header_overflow_first() {
        let entries = parse(&["GET u8 0", "SET i16 #8 -1", "INCRBY u4 #0 3"]);
        let command =
            super::encode_bitfield("k", Some(super::BitFieldOverflow::Wrap), &entries, false);
        assert_eq!(
            packed_args(&command),
            vec![
                "BITFIELD", "k", "OVERFLOW", "WRAP", "GET", "u8", "0", "SET", "i16", "#8", "-1",
                "INCRBY", "u4", "#0", "3",
            ]
        );
    }

    #[test]
    fn test_bitfield_reply_maps_ints_and_nils_to_json_array() {
        let body = super::bitfield_reply_to_body(vec![Some(1), None, Some(-3)]);
        assert_eq!(body, Body::Json(serde_json::json!([1, null, -3])));
    }

    #[test]
    fn test_bitfield_ro_wire_frame() {
        let entries = parse(&["GET u8 #1"]);
        let command = super::encode_bitfield("k", None, &entries, true);
        assert_eq!(
            packed_args(&command),
            vec!["BITFIELD_RO", "k", "GET", "u8", "#1"]
        );
    }
}
