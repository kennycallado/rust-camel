//! `CamelRedis.Batch` preflight: entry parsing, allowlist, and field contract.
//!
//! The batch header is producer-side route data: it names the Redis commands
//! that run per exchange and carries their operands. Value payloads always
//! come from the body (ADR-0098), never from an entry field, so an entry is
//! validated against a per-command field contract before a single command
//! reaches Redis.

use super::invalid_header_value_preview;
use super::key;
use super::string;
use crate::config::RedisCommand;
use crate::config::is_idempotent_command;
use camel_component_api::Body;
use camel_component_api::CamelError;
use camel_component_api::Exchange;

/// Header carrying the batch entry plan.
const BATCH_HEADER: &str = "CamelRedis.Batch";

/// Entry field naming the Redis command to run.
const COMMAND_FIELD: &str = "command";

/// Upper bound on the entries of one batch.
///
/// The cap bounds the per-exchange work a producer accepts (and, with
/// `atomic=true`, the size of one `MULTI`/`EXEC` transaction) so a
/// misconfigured route cannot fan one message out into an unbounded command
/// list.
pub const MAX_BATCH_ENTRIES: usize = 256;

/// One parsed batch entry.
#[derive(Debug, Clone)]
pub struct BatchEntry {
    /// The Redis command this entry runs.
    pub command: RedisCommand,

    /// The entry's operand fields, keyed by field name.
    pub fields: serde_json::Map<String, serde_json::Value>,
}

/// The validated plan for one exchange's batch.
#[derive(Debug, Clone)]
pub struct BatchPlan {
    /// The validated entries, in header order.
    pub entries: Vec<BatchEntry>,

    /// True when every entry's command is idempotent, which is what makes an
    /// outcome-unknown batch safe to replay.
    pub idempotent: bool,
}

/// The value domain of one entry field.
#[derive(Debug, Clone, Copy)]
pub enum FieldKind {
    /// A JSON string.
    Str,

    /// An `i64`, as a JSON integer or a numeric string.
    Int,

    /// An `i64` strictly greater than zero, as a JSON integer or a numeric
    /// string.
    PositiveInt,

    /// A `u64`, as a JSON integer or a numeric string.
    UInt,

    /// A non-empty JSON array of strings.
    Strs,
}

/// One allowed entry field of a batchable command.
#[derive(Debug, Clone, Copy)]
pub struct FieldSpec {
    /// The field name, matching the `CamelRedis.*` header it mirrors.
    pub name: &'static str,

    /// Whether every entry of this command must carry the field.
    pub required: bool,

    /// The field's value domain.
    pub kind: FieldKind,
}

/// Parses and validates the `CamelRedis.Batch` header of `exchange`.
///
/// This is a pure preflight: it never touches a connection, so a rejected
/// batch cannot send a partially-applied command list. It fails closed on a
/// missing header, malformed JSON, a non-array header, a non-object entry, an
/// unknown or non-batchable `command`, an entry-count over
/// [`MAX_BATCH_ENTRIES`], a field outside [`allowed_entry_fields`], a
/// wrong-typed or non-positive field value, a missing required field, a body
/// that is not a JSON array (including an empty body: the aligned array is
/// mandatory), and a body array whose length does not match the entry count.
/// Every error names `CamelRedis.Batch` and, for an entry-level problem, its
/// index.
pub fn plan_batch(exchange: &Exchange) -> Result<BatchPlan, CamelError> {
    let Some(raw) = exchange.input.header(BATCH_HEADER) else {
        return Err(batch_error(
            "missing required header: expected a JSON array of entry objects",
        ));
    };
    let Some(raw_entries) = raw.as_array() else {
        return Err(batch_error(format!(
            "expected a JSON array of entry objects, got {}",
            invalid_header_value_preview(raw)
        )));
    };
    if raw_entries.len() > MAX_BATCH_ENTRIES {
        return Err(batch_error(format!(
            "{} entries exceed MAX_BATCH_ENTRIES ({MAX_BATCH_ENTRIES})",
            raw_entries.len()
        )));
    }

    let entries = raw_entries
        .iter()
        .enumerate()
        .map(|(index, raw_entry)| parse_entry(raw_entry, index))
        .collect::<Result<Vec<_>, _>>()?;

    validate_body_length(exchange, entries.len())?;

    // A batch is replayable only when every command in it is: one
    // non-idempotent entry makes the whole transaction unsafe to retry.
    let idempotent = entries
        .iter()
        .all(|entry| is_idempotent_command(&entry.command));

    Ok(BatchPlan {
        entries,
        idempotent,
    })
}

/// Returns true only for commands with a `batch` encoder.
///
/// The allowlist is exactly the set of commands the batch encoder supports:
/// a command outside it would have no way to encode an entry, so it is
/// rejected at plan time rather than at send time. The v1 allowlist is `GET`,
/// `SET`, `GETDEL`, `GETEX`, `SETRANGE`, `GETRANGE`, `INCR`, `INCRBY`,
/// `DECR`, `DECRBY`, `APPEND`, `MGET`, `DEL`, `UNLINK`, `EXPIRE`, `PERSIST`,
/// `COPY`, and `TOUCH`. Blocking pops, the consumer commands, pub/sub,
/// streams, and the script commands are outside it.
pub fn batchable(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        RedisCommand::Get
            | RedisCommand::Set
            | RedisCommand::Getdel
            | RedisCommand::Getex
            | RedisCommand::Setrange
            | RedisCommand::Getrange
            | RedisCommand::Incr
            | RedisCommand::Incrby
            | RedisCommand::Decr
            | RedisCommand::Decrby
            | RedisCommand::Append
            | RedisCommand::Mget
            | RedisCommand::Del
            | RedisCommand::Unlink
            | RedisCommand::Expire
            | RedisCommand::Persist
            | RedisCommand::Copy
            | RedisCommand::Touch
    )
}

/// One required key field.
const KEY_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "key",
    required: true,
    kind: FieldKind::Str,
}];

/// `GETEX`: a key plus an optional relative expiry.
const GETEX_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "key",
        required: true,
        kind: FieldKind::Str,
    },
    FieldSpec {
        name: "timeout",
        required: false,
        kind: FieldKind::PositiveInt,
    },
];

/// `SETRANGE`: a key plus the byte offset to write at.
const SETRANGE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "key",
        required: true,
        kind: FieldKind::Str,
    },
    FieldSpec {
        name: "offset",
        required: true,
        kind: FieldKind::UInt,
    },
];

/// `GETRANGE`: a key plus the inclusive start/end offsets.
const GETRANGE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "key",
        required: true,
        kind: FieldKind::Str,
    },
    FieldSpec {
        name: "start",
        required: true,
        kind: FieldKind::Int,
    },
    FieldSpec {
        name: "end",
        required: true,
        kind: FieldKind::Int,
    },
];

/// `INCRBY`/`DECRBY`: a key plus the signed increment.
const INCREMENT_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "key",
        required: true,
        kind: FieldKind::Str,
    },
    FieldSpec {
        name: "increment",
        required: true,
        kind: FieldKind::Int,
    },
];

/// Multi-key commands: one non-empty key list.
const KEYS_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "keys",
    required: true,
    kind: FieldKind::Strs,
}];

/// `EXPIRE`: a key plus a strictly positive TTL.
const EXPIRE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "key",
        required: true,
        kind: FieldKind::Str,
    },
    FieldSpec {
        name: "timeout",
        required: true,
        kind: FieldKind::PositiveInt,
    },
];

/// `COPY`: a source key plus the destination key.
const COPY_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "key",
        required: true,
        kind: FieldKind::Str,
    },
    FieldSpec {
        name: "destination",
        required: true,
        kind: FieldKind::Str,
    },
];

/// Returns the field contract of `cmd`, or an empty slice for a command
/// outside the batch allowlist.
pub fn allowed_entry_fields(cmd: &RedisCommand) -> &'static [FieldSpec] {
    match cmd {
        RedisCommand::Get
        | RedisCommand::Getdel
        | RedisCommand::Incr
        | RedisCommand::Decr
        | RedisCommand::Persist
        | RedisCommand::Set
        | RedisCommand::Append => KEY_FIELDS,
        RedisCommand::Getex => GETEX_FIELDS,
        RedisCommand::Setrange => SETRANGE_FIELDS,
        RedisCommand::Getrange => GETRANGE_FIELDS,
        RedisCommand::Incrby | RedisCommand::Decrby => INCREMENT_FIELDS,
        RedisCommand::Mget | RedisCommand::Del | RedisCommand::Unlink | RedisCommand::Touch => {
            KEYS_FIELDS
        }
        RedisCommand::Expire => EXPIRE_FIELDS,
        RedisCommand::Copy => COPY_FIELDS,
        // Non-batchable commands have no contract: every field is unknown.
        _ => &[],
    }
}

// ── Entry encoders ──────────────────────────────────────────────────────────

/// Encodes one validated batch entry into its wire command.
///
/// `value` is the entry's body-array slot: the payload for the commands that
/// take one (`SET`, `SETRANGE`, `APPEND`) and `null`/absent for the rest.
/// Operands come from the validated [`BatchEntry::fields`]; payloads come ONLY
/// from `value` (ADR-0098: an entry field can never carry the payload).
///
/// This is a pure function: it performs no I/O and is what the `Multi`
/// dispatch arm queues, so the batch commands are built by the same
/// `encode_*` helpers the single-command dispatch path uses.
pub fn encode_entry(
    entry: &BatchEntry,
    value: Option<&serde_json::Value>,
) -> Result<redis::Cmd, CamelError> {
    // A `null` body slot is the "this entry takes no value" placeholder.
    let value = value.filter(|value| !value.is_null());

    match &entry.command {
        RedisCommand::Set => {
            let key = entry_str(entry, "key")?;
            let payload = entry_value_bytes(entry, value)?;
            Ok(string::encode_set(
                key,
                &payload,
                &string::SetOptions::default(),
            ))
        }
        RedisCommand::Get => {
            expect_no_value(entry, value)?;
            Ok(string::encode_get(entry_str(entry, "key")?))
        }
        RedisCommand::Getdel => {
            expect_no_value(entry, value)?;
            Ok(string::encode_getdel(entry_str(entry, "key")?))
        }
        RedisCommand::Getex => {
            expect_no_value(entry, value)?;
            let key = entry_str(entry, "key")?;
            let options = match entry_optional_u64(entry, "timeout")? {
                Some(seconds) => string::GetexOptions::relative_expiry(seconds),
                None => string::GetexOptions::default(),
            };
            Ok(string::encode_getex(key, &options))
        }
        RedisCommand::Setrange => {
            let key = entry_str(entry, "key")?;
            let offset = entry_u64(entry, "offset")?;
            let payload = entry_value_bytes(entry, value)?;
            Ok(string::encode_setrange(key, offset, &payload))
        }
        RedisCommand::Getrange => {
            expect_no_value(entry, value)?;
            let key = entry_str(entry, "key")?;
            Ok(string::encode_getrange(
                key,
                entry_i64(entry, "start")?,
                entry_i64(entry, "end")?,
            ))
        }
        RedisCommand::Incr => {
            expect_no_value(entry, value)?;
            Ok(string::encode_incr(entry_str(entry, "key")?))
        }
        RedisCommand::Incrby => {
            expect_no_value(entry, value)?;
            let key = entry_str(entry, "key")?;
            Ok(string::encode_incrby(key, entry_i64(entry, "increment")?))
        }
        RedisCommand::Decr => {
            expect_no_value(entry, value)?;
            Ok(string::encode_decr(entry_str(entry, "key")?))
        }
        RedisCommand::Decrby => {
            expect_no_value(entry, value)?;
            let key = entry_str(entry, "key")?;
            Ok(string::encode_decrby(key, entry_i64(entry, "increment")?))
        }
        RedisCommand::Append => {
            let key = entry_str(entry, "key")?;
            let payload = entry_value_bytes(entry, value)?;
            Ok(string::encode_append(key, &payload))
        }
        RedisCommand::Mget => {
            expect_no_value(entry, value)?;
            Ok(string::encode_mget(&entry_keys(entry)?))
        }
        RedisCommand::Del => {
            expect_no_value(entry, value)?;
            Ok(key::encode_del(&entry_keys(entry)?))
        }
        RedisCommand::Unlink => {
            expect_no_value(entry, value)?;
            Ok(key::encode_unlink(&entry_keys(entry)?))
        }
        RedisCommand::Expire => {
            expect_no_value(entry, value)?;
            let key = entry_str(entry, "key")?;
            Ok(key::encode_expire(key, entry_i64(entry, "timeout")?))
        }
        RedisCommand::Persist => {
            expect_no_value(entry, value)?;
            Ok(key::encode_persist(entry_str(entry, "key")?))
        }
        RedisCommand::Copy => {
            expect_no_value(entry, value)?;
            Ok(key::encode_copy(
                entry_str(entry, "key")?,
                entry_str(entry, "destination")?,
                None,
                false,
            ))
        }
        RedisCommand::Touch => {
            expect_no_value(entry, value)?;
            Ok(key::encode_touch(&entry_keys(entry)?))
        }
        // `plan_batch` rejects every command outside the allowlist, so an
        // entry that reaches encoding is always one of the arms above.
        other => Err(batch_error(format!(
            "command {} has no batch encoder",
            other.as_str()
        ))),
    }
}

/// Named error for one entry field that cannot be read.
fn entry_field_error(
    entry: &BatchEntry,
    field: &str,
    detail: impl std::fmt::Display,
) -> CamelError {
    batch_error(format!(
        "command {} field {field:?} {detail}",
        entry.command.as_str()
    ))
}

/// Reads a required string field.
fn entry_str<'a>(entry: &'a BatchEntry, field: &str) -> Result<&'a str, CamelError> {
    match entry.fields.get(field) {
        Some(serde_json::Value::String(text)) => Ok(text),
        Some(other) => Err(entry_field_error(
            entry,
            field,
            format!(
                "must be a string, got {}",
                invalid_header_value_preview(other)
            ),
        )),
        None => Err(entry_field_error(entry, field, "is required")),
    }
}

/// Reads a required `i64` field (JSON integer or numeric string).
fn entry_i64(entry: &BatchEntry, field: &str) -> Result<i64, CamelError> {
    entry
        .fields
        .get(field)
        .and_then(json_i64)
        .ok_or_else(|| entry_field_error(entry, field, "must be an integer (i64)"))
}

/// Reads a required `u64` field (JSON integer or numeric string).
fn entry_u64(entry: &BatchEntry, field: &str) -> Result<u64, CamelError> {
    entry
        .fields
        .get(field)
        .and_then(json_u64)
        .ok_or_else(|| entry_field_error(entry, field, "must be a non-negative integer (u64)"))
}

/// Reads an optional `u64` field.
fn entry_optional_u64(entry: &BatchEntry, field: &str) -> Result<Option<u64>, CamelError> {
    match entry.fields.get(field) {
        Some(value) => json_u64(value)
            .map(Some)
            .ok_or_else(|| entry_field_error(entry, field, "must be a non-negative integer (u64)")),
        None => Ok(None),
    }
}

/// Reads the required non-empty `keys` array.
fn entry_keys(entry: &BatchEntry) -> Result<Vec<String>, CamelError> {
    match entry.fields.get("keys") {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                serde_json::Value::String(text) => Ok(text.clone()),
                other => Err(entry_field_error(
                    entry,
                    "keys",
                    format!(
                        "must contain strings only, got {}",
                        invalid_header_value_preview(other)
                    ),
                )),
            })
            .collect(),
        Some(other) => Err(entry_field_error(
            entry,
            "keys",
            format!(
                "must be a non-empty JSON array of strings, got {}",
                invalid_header_value_preview(other)
            ),
        )),
        None => Err(entry_field_error(entry, "keys", "is required")),
    }
}

/// Encodes the required payload slot of a value-taking command.
///
/// Mirrors [`crate::commands::encode_body_value`]'s scalar codec: a string
/// element contributes its raw UTF-8 bytes, a number or boolean its compact
/// JSON form, and a `null`/absent slot is a named error (those entries must
/// name a value).
///
/// The batch slot contract admits a scalar payload only. A JSON object or
/// array is rejected before send: the v1 allowlist has no multi-value
/// command, so a container slot has no meaning (the `redis-commands` spec's
/// "array of scalars for multi-value commands" has no v1 member). Rejecting
/// it here also keeps a route from silently writing a serialized object into
/// a Redis value.
fn entry_value_bytes(
    entry: &BatchEntry,
    value: Option<&serde_json::Value>,
) -> Result<Vec<u8>, CamelError> {
    match value {
        Some(serde_json::Value::String(text)) => Ok(text.as_bytes().to_vec()),
        Some(value @ (serde_json::Value::Number(_) | serde_json::Value::Bool(_))) => {
            serde_json::to_string(value)
                .map(String::into_bytes)
                .map_err(|err| {
                    entry_field_error(
                        entry,
                        "body value",
                        format!("could not be serialized: {err}"),
                    )
                })
        }
        Some(value @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => {
            Err(entry_field_error(
                entry,
                "body value",
                format!(
                    "must be a scalar payload (string, number, or boolean) or null, got {}",
                    batch_slot_shape(value)
                ),
            ))
        }
        // `encode_entry` filters `null` before this call; the arm keeps the
        // "value required" verdict for a direct caller.
        Some(serde_json::Value::Null) => Err(entry_field_error(
            entry,
            "body value",
            "is required: the aligned body array must carry a payload",
        )),
        None => Err(entry_field_error(
            entry,
            "body value",
            "is required: the aligned body array must carry a payload",
        )),
    }
}

/// Names a rejected body-slot shape for the batch scalar-slot contract.
fn batch_slot_shape(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Object(_) => "a JSON object",
        serde_json::Value::Array(_) => "a JSON array",
        serde_json::Value::Null => "null",
        serde_json::Value::String(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::Bool(_) => "a scalar value",
    }
}

/// Rejects a payload on an entry whose command takes no body value.
fn expect_no_value(
    entry: &BatchEntry,
    value: Option<&serde_json::Value>,
) -> Result<(), CamelError> {
    match value {
        None => Ok(()),
        Some(other) => Err(entry_field_error(
            entry,
            "body value",
            format!(
                "must be null: command {} takes no body value, got {}",
                entry.command.as_str(),
                invalid_header_value_preview(other)
            ),
        )),
    }
}

// ── Preflight internals ─────────────────────────────────────────────────────

/// Named error for any `CamelRedis.Batch` preflight rejection.
fn batch_error(detail: impl std::fmt::Display) -> CamelError {
    CamelError::ProcessorError(format!("Invalid {BATCH_HEADER}: {detail}"))
}

/// Named preflight error for one entry.
fn entry_error(index: usize, detail: impl std::fmt::Display) -> CamelError {
    batch_error(format!("entry {index} {detail}"))
}

/// Parses and validates one batch entry.
fn parse_entry(raw: &serde_json::Value, index: usize) -> Result<BatchEntry, CamelError> {
    let Some(object) = raw.as_object() else {
        return Err(entry_error(
            index,
            format!(
                "must be a JSON object, got {}",
                invalid_header_value_preview(raw)
            ),
        ));
    };

    let command_name = match object.get(COMMAND_FIELD) {
        Some(serde_json::Value::String(name)) => name,
        Some(other) => {
            return Err(entry_error(
                index,
                format!(
                    "field \"{COMMAND_FIELD}\" must be a string, got {}",
                    invalid_header_value_preview(other)
                ),
            ));
        }
        None => {
            return Err(entry_error(
                index,
                format!("is missing the required \"{COMMAND_FIELD}\" string"),
            ));
        }
    };

    let command = command_name.parse::<RedisCommand>().map_err(|_| {
        entry_error(
            index,
            format!("has an unknown Redis command {command_name:?}"),
        )
    })?;

    if !batchable(&command) {
        return Err(entry_error(
            index,
            format!(
                "command {command_name:?} is not batchable: the batch allowlist is GET, SET, GETDEL, GETEX, SETRANGE, GETRANGE, INCR, INCRBY, DECR, DECRBY, APPEND, MGET, DEL, UNLINK, EXPIRE, PERSIST, COPY, TOUCH"
            ),
        ));
    }

    validate_entry_fields(&command, object, index)?;

    // `fields` carries the operands only: `command` has its own typed home.
    let mut fields = object.clone();
    fields.remove(COMMAND_FIELD);

    Ok(BatchEntry { command, fields })
}

/// Validates one entry's fields against its command's contract.
fn validate_entry_fields(
    command: &RedisCommand,
    fields: &serde_json::Map<String, serde_json::Value>,
    index: usize,
) -> Result<(), CamelError> {
    let specs = allowed_entry_fields(command);

    for name in fields.keys() {
        if name == COMMAND_FIELD {
            continue;
        }
        if !specs.iter().any(|spec| spec.name == name) {
            return Err(entry_error(
                index,
                format!(
                    "field {name:?} is not valid for command {} (allowed: {})",
                    command.as_str(),
                    allowed_field_names(specs)
                ),
            ));
        }
    }

    for spec in specs {
        match fields.get(spec.name) {
            Some(value) => validate_field_value(command, spec, value, index)?,
            None if spec.required => {
                return Err(entry_error(
                    index,
                    format!(
                        "is missing the required field {:?} for command {}",
                        spec.name,
                        command.as_str()
                    ),
                ));
            }
            None => {}
        }
    }

    Ok(())
}

/// Validates one field value against its [`FieldKind`].
fn validate_field_value(
    command: &RedisCommand,
    spec: &FieldSpec,
    value: &serde_json::Value,
    index: usize,
) -> Result<(), CamelError> {
    let valid = match spec.kind {
        FieldKind::Str => value.is_string(),
        FieldKind::Int => json_i64(value).is_some(),
        FieldKind::PositiveInt => json_i64(value).is_some_and(|number| number > 0),
        FieldKind::UInt => json_u64(value).is_some(),
        FieldKind::Strs => match value {
            serde_json::Value::Array(items) => {
                !items.is_empty() && items.iter().all(serde_json::Value::is_string)
            }
            _ => false,
        },
    };

    if valid {
        return Ok(());
    }

    Err(entry_error(
        index,
        format!(
            "field {:?} of command {} must be {}, got {}",
            spec.name,
            command.as_str(),
            field_kind_description(spec.kind),
            invalid_header_value_preview(value)
        ),
    ))
}

/// Renders the allowed field names of a contract for a diagnostic.
fn allowed_field_names(specs: &[FieldSpec]) -> String {
    specs
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders a [`FieldKind`] as the requirement text of a diagnostic.
fn field_kind_description(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Str => "a string",
        FieldKind::Int => "an integer (i64)",
        FieldKind::PositiveInt => "a positive integer (i64 > 0)",
        FieldKind::UInt => "a non-negative integer (u64)",
        FieldKind::Strs => "a non-empty JSON array of strings",
    }
}

/// Reads an `i64` from a JSON integer or a trimmed numeric string.
///
/// Mirrors [`crate::commands::get_i64_header`]: a float, a non-numeric value,
/// or a number outside `i64` range yields `None`.
fn json_i64(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Reads a `u64` from a JSON integer or a trimmed numeric string.
///
/// Mirrors [`crate::commands::get_u64_header`]: a negative, oversized, or
/// non-numeric value yields `None`.
fn json_u64(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
}

/// Validates the body shape of a batch.
///
/// The body carries the per-entry value payloads in entry order, so it must be
/// a `Body::Json` array with exactly one element per entry; an entry that takes
/// no value still needs its `null` placeholder to keep the array aligned. Any
/// other body form (including an empty body) is rejected before send rather
/// than guessed at. Element type checking belongs to the batch encoders, not to
/// the plan.
fn validate_body_length(exchange: &Exchange, entry_count: usize) -> Result<(), CamelError> {
    let elements = match &exchange.input.body {
        Body::Json(serde_json::Value::Array(elements)) => elements,
        other => {
            return Err(batch_error(format!(
                "the body must be a JSON array with one element per entry (use null for entries that take no value), got a {} body",
                batch_body_shape(other)
            )));
        }
    };

    if elements.len() != entry_count {
        return Err(batch_error(format!(
            "the body array carries {} element(s), expected {entry_count} (exactly one body element per entry)",
            elements.len()
        )));
    }

    Ok(())
}

/// Renders the body variant for a batch body-shape diagnostic.
fn batch_body_shape(body: &Body) -> &'static str {
    match body {
        Body::Empty => "empty",
        Body::Bytes(_) => "binary",
        Body::Text(_) => "text",
        Body::Json(_) => "non-array JSON",
        Body::Xml(_) => "XML",
        Body::Stream(_) => "streaming",
        _ => "unexpected",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::Message;

    /// Builds an exchange carrying `entries` as `CamelRedis.Batch` and `body`.
    fn batch_exchange(entries: serde_json::Value, body: Body) -> Exchange {
        let mut message = Message::new(body);
        message.set_header(BATCH_HEADER, entries);
        Exchange::new(message)
    }

    #[test]
    fn batch_parses_allowlisted_entries() {
        let exchange = batch_exchange(
            serde_json::json!([
                {"command": "set", "key": "a"},
                {"command": "incr", "key": "b"},
            ]),
            // SET takes `"v"`; INCR takes no value, so its slot is `null`.
            Body::Json(serde_json::json!(["v", null])),
        );

        let plan = plan_batch(&exchange).expect("a well-formed allowlisted batch must plan");

        assert_eq!(plan.entries.len(), 2, "both entries must survive planning");
        assert_eq!(plan.entries[0].command, RedisCommand::Set);
        assert_eq!(plan.entries[1].command, RedisCommand::Incr);
        assert!(batchable(&plan.entries[0].command), "SET is batchable");
        assert!(batchable(&plan.entries[1].command), "INCR is batchable");
        assert!(
            !plan.idempotent,
            "SET and INCR are not idempotent, so the batch is not"
        );
    }

    #[test]
    fn batch_unknown_command_fails_closed_with_index() {
        let exchange = batch_exchange(
            serde_json::json!([
                {"command": "set", "key": "a"},
                {"command": "nosuchcmd"},
            ]),
            Body::Empty,
        );

        // `plan_batch` is a pure preflight over the exchange: rejecting it
        // here means no command bytes were ever handed to a connection.
        let err = plan_batch(&exchange).expect_err("an unknown command must fail closed");
        let message = err.to_string();

        assert!(
            message.contains("CamelRedis.Batch"),
            "the error must name the header: {message}"
        );
        assert!(
            message.contains("entry 1"),
            "the error must name the failing entry index: {message}"
        );
    }

    #[test]
    fn batch_non_batchable_command_rejected_with_index() {
        let exchange = batch_exchange(
            serde_json::json!([{"command": "blpop", "key": "k"}]),
            Body::Empty,
        );

        assert!(
            !batchable(&RedisCommand::Blpop),
            "blocking pops are outside the batch allowlist"
        );

        let err = plan_batch(&exchange).expect_err("a non-batchable command must fail closed");
        let message = err.to_string();

        assert!(
            message.contains("entry 0"),
            "the error must name the failing entry index: {message}"
        );
        assert!(
            message.contains("blpop"),
            "the error must name the rejected command: {message}"
        );
    }

    #[test]
    fn batch_entry_scan_is_named_rejection() {
        let exchange = batch_exchange(serde_json::json!([{"command": "SCAN"}]), Body::Empty);

        assert!(
            !batchable(&RedisCommand::Scan),
            "the SCAN family is outside the batch allowlist"
        );

        let err = plan_batch(&exchange).expect_err("a SCAN entry must fail closed");
        let message = err.to_string();

        assert!(
            message.contains("CamelRedis.Batch"),
            "the error must name the header: {message}"
        );
        assert!(
            message.contains("entry 0"),
            "the error must name the failing entry index: {message}"
        );
        assert!(
            message.contains("SCAN"),
            "the error must name the rejected command: {message}"
        );
    }

    #[test]
    fn batch_unknown_field_or_bad_type_fails_with_index() {
        let cases = [
            (
                serde_json::json!([{"command": "expire", "key": "k", "timeout": "soon"}]),
                "timeout",
            ),
            (
                serde_json::json!([{"command": "set", "key": "a", "bogus": 1}]),
                "bogus",
            ),
        ];

        for (entries, field) in cases {
            let exchange = batch_exchange(entries, Body::Empty);
            let err = plan_batch(&exchange).expect_err("an invalid field must fail closed");
            let message = err.to_string();

            assert!(
                message.contains("entry 0"),
                "the error must name the failing entry index for {field}: {message}"
            );
            assert!(
                message.contains(field),
                "the error must name the failing field {field}: {message}"
            );
        }
    }

    #[test]
    fn batch_missing_required_field_fails_with_index() {
        let exchange = batch_exchange(
            serde_json::json!([{"command": "copy", "key": "a"}]),
            Body::Empty,
        );

        let err = plan_batch(&exchange).expect_err("a missing required field must fail closed");
        let message = err.to_string();

        assert!(
            message.contains("entry 0"),
            "the error must name the failing entry index: {message}"
        );
        assert!(
            message.contains("destination"),
            "the error must name the missing field: {message}"
        );
    }

    #[test]
    fn batch_timeout_must_be_positive() {
        // A JSON number above `i64::MAX` arrives as an f64 (serde_json keeps
        // it as a float when it cannot fit an integer); it must be rejected
        // named, never narrowed into a negative value.
        let overflow: serde_json::Value =
            serde_json::from_str("99999999999999999999").expect("the literal is valid JSON");

        for timeout in [serde_json::json!(0), serde_json::json!(-5), overflow] {
            let exchange = batch_exchange(
                serde_json::json!([{"command": "expire", "key": "k", "timeout": timeout}]),
                Body::Empty,
            );

            let err = plan_batch(&exchange).expect_err("a non-positive timeout must fail closed");
            let message = err.to_string();

            assert!(
                message.contains("entry 0"),
                "the error must name the failing entry index: {message}"
            );
            assert!(
                message.contains("timeout"),
                "the error must name the failing field: {message}"
            );
        }
    }

    #[test]
    fn batch_body_length_mismatch_fails_named_error() {
        let exchange = batch_exchange(
            serde_json::json!([
                {"command": "incr", "key": "a"},
                {"command": "incr", "key": "b"},
                {"command": "incr", "key": "c"},
            ]),
            Body::Json(serde_json::json!(["v1", "v2"])),
        );

        let err = plan_batch(&exchange).expect_err("a body shorter than the entries must fail");
        let message = err.to_string();

        assert!(
            message.contains("CamelRedis.Batch"),
            "the error must name the header: {message}"
        );
        assert!(
            message.contains("expected 3"),
            "the error must name the expected element count: {message}"
        );
        assert!(
            message.contains("2 element"),
            "the error must name the actual element count: {message}"
        );
    }

    #[test]
    fn batch_empty_body_is_rejected() {
        let exchange = batch_exchange(
            serde_json::json!([{"command": "set", "key": "a"}]),
            Body::Empty,
        );

        // `plan_batch` is a pure preflight over the exchange: rejecting it
        // here means no command bytes were ever handed to a connection.
        let err = plan_batch(&exchange).expect_err("a batch without a body array must fail closed");
        let message = err.to_string();

        assert!(
            message.contains("CamelRedis.Batch"),
            "the error must name the header: {message}"
        );
        assert!(
            message.contains("JSON array"),
            "the error must name the expected body form: {message}"
        );
        assert!(
            message.contains("empty"),
            "the error must name the actual body shape: {message}"
        );
    }

    #[test]
    fn batch_empty_body_rejected_even_without_value_entries() {
        // GET takes no body value, but the aligned array is still mandatory:
        // the batch contract is index-aligned, not bodyless-when-unused.
        let exchange = batch_exchange(
            serde_json::json!([{"command": "get", "key": "a"}]),
            Body::Empty,
        );

        let err = plan_batch(&exchange)
            .expect_err("a value-less entry still needs its null placeholder slot");
        let message = err.to_string();

        assert!(
            message.contains("CamelRedis.Batch"),
            "the error must name the header: {message}"
        );
        assert!(
            message.contains("JSON array"),
            "the error must name the expected body form: {message}"
        );
    }

    #[test]
    fn batch_non_array_body_is_rejected() {
        let exchange = batch_exchange(
            serde_json::json!([{"command": "set", "key": "a"}]),
            Body::Text("v".to_owned()),
        );

        let err = plan_batch(&exchange).expect_err("a non-array body must fail closed");
        let message = err.to_string();

        assert!(
            message.contains("CamelRedis.Batch"),
            "the error must name the header: {message}"
        );
        assert!(
            message.contains("JSON array"),
            "the error must name the expected body form: {message}"
        );
        assert!(
            message.contains("text"),
            "the error must name the actual body shape: {message}"
        );
    }

    #[test]
    fn batch_idempotence_aggregates_entries() {
        // GET is idempotent, GETDEL is not: one non-idempotent entry makes the
        // whole batch unsafe to replay.
        let mixed = batch_exchange(
            serde_json::json!([
                {"command": "get", "key": "a"},
                {"command": "getdel", "key": "b"},
            ]),
            Body::Json(serde_json::json!([null, null])),
        );
        let all_reads = batch_exchange(
            serde_json::json!([
                {"command": "get", "key": "a"},
                {"command": "get", "key": "b"},
            ]),
            Body::Json(serde_json::json!([null, null])),
        );

        assert!(
            !plan_batch(&mixed).expect("a mixed batch plans").idempotent,
            "GET + GETDEL is not idempotent"
        );
        assert!(
            plan_batch(&all_reads)
                .expect("an all-read batch plans")
                .idempotent,
            "GET + GET is idempotent because every entry is"
        );
    }

    #[test]
    fn encode_entry_matches_dispatch_encoding() {
        let mut fields = serde_json::Map::new();
        fields.insert("key".to_string(), serde_json::json!("a"));
        let entry = BatchEntry {
            command: RedisCommand::Set,
            fields,
        };
        let value = serde_json::json!("v");

        let batch_cmd = encode_entry(&entry, Some(&value))
            .expect("a validated SET entry with a string value must encode");

        // The single-command dispatch path resolves the same operands from
        // headers and the body, then calls the same encoder.
        let mut message = Message::new(Body::Json(value));
        message.set_header("CamelRedis.Key", serde_json::json!("a"));
        let single = Exchange::new(message);
        let key = crate::commands::require_key(&single).expect("the header supplies the key");
        let payload = crate::commands::body_value_bytes(&single, "SET", &[crate::HEADER_VALUE])
            .expect("the body supplies the value");
        let options =
            crate::commands::string::resolve_set_options(&single).expect("no SET options are set");
        let single_cmd = crate::commands::string::encode_set(&key, &payload, &options);

        assert_eq!(
            format!("{batch_cmd:?}"),
            format!("{single_cmd:?}"),
            "the batch encoder must build the same SET command as the dispatch path"
        );
    }

    #[test]
    fn batch_entry_count_cap_fails_named_error() {
        let entries: Vec<serde_json::Value> = (0..=MAX_BATCH_ENTRIES)
            .map(|index| serde_json::json!({"command": "incr", "key": format!("k{index}")}))
            .collect();
        assert_eq!(entries.len(), MAX_BATCH_ENTRIES + 1, "one over the cap");

        let exchange = batch_exchange(serde_json::Value::Array(entries), Body::Empty);

        let err = plan_batch(&exchange).expect_err("an over-cap batch must fail closed");
        let message = err.to_string();

        assert!(
            message.contains("MAX_BATCH_ENTRIES"),
            "the error must name the cap: {message}"
        );
    }
}
