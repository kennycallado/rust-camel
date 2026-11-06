//! Document namespaces: mint, run id, and load validation (bd rc-ql8eq).
//!
//! Grammar (ADR-0069 §4): `BASE ~it~ RUN ~ ORD ~`.
//!
//! - `BASE` is `[A-Za-z0-9][A-Za-z0-9_-]{0,31}` (at most 32 bytes).
//! - `RUN` is four OS-CSPRNG bytes rendered as exactly eight lowercase hex
//!   characters, generated once per `camel test` invocation and shared by
//!   every document of that invocation. It is never derived from the clock,
//!   the pid, or the hostname.
//! - `ORD` is the 1-based zero-padded four-digit ordinal of the document in
//!   the deterministic sorted discovered-document list, assigned at
//!   discovery time; more than [`MAX_DOCUMENTS`] discovered documents is a
//!   load error.
//!
//! The mint is prefix-free across the documents of one run for two
//! independent reasons: the trailing `~` terminates every value, and every
//! segment has a fixed width (eight hex characters, four digits), so no
//! shorter ordinal is a byte prefix of a longer one.
//!
//! The namespace is a harness isolation device: bindings resolve only from
//! the harness layer ([`crate::env_layers::LayeredEnv`]), never from the
//! document `env:` map or the ambient environment.

use std::collections::BTreeMap;

/// Length of the run id in characters: four CSPRNG bytes as hex.
pub const RUN_ID_HEX_LEN: usize = 8;

/// Maximum number of discovered documents per `camel test` invocation.
pub const MAX_DOCUMENTS: usize = 9999;

/// Maximum minted namespace length in bytes (base + separators + run + ord).
pub const MAX_MINT_LEN: usize = 50;

/// Marker separating [`NamespaceBinding::base`] from the run id.
const NAMESPACE_MARKER: &str = "~it~";

/// Maximum base length in bytes: one leading character plus up to 31 more.
const BASE_MAX_LEN: usize = 32;

/// Fixed width of the zero-padded ordinal.
const ORDINAL_WIDTH: usize = 4;

/// A declared `bindVar` / `base` pair from a scenario document's
/// `namespaces:` section, already load-validated.
#[derive(Clone, Debug, PartialEq)]
pub struct NamespaceBinding {
    /// The environment variable the harness binds the minted value to.
    pub bind_var: String,
    /// The namespace base (`[A-Za-z0-9][A-Za-z0-9_-]{0,31}`).
    pub base: String,
}

/// Generates a fresh run id from the OS CSPRNG: four bytes rendered as
/// exactly eight lowercase hex characters.
///
/// The caller generates this once per `camel test` invocation and passes the
/// same value to every document of that invocation.
#[must_use]
pub fn generate_run_id() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).expect("the OS CSPRNG must provide four bytes"); // allow-unwrap

    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut run_id = String::with_capacity(RUN_ID_HEX_LEN);
    for byte in bytes {
        run_id.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        run_id.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    run_id
}

/// Mints one document's namespace: `{base}~it~{run_id}~{ordinal:04}~`.
///
/// # Errors
///
/// Returns an owned error when `ordinal` is 0 or greater than
/// [`MAX_DOCUMENTS`], when `run_id` is not exactly [`RUN_ID_HEX_LEN`]
/// lowercase hex characters, or when the minted value exceeds
/// [`MAX_MINT_LEN`] bytes.
#[must_use = "discarding the minted namespace loses the document scoping"]
pub fn mint_namespace(base: &str, run_id: &str, ordinal: usize) -> Result<String, String> {
    if ordinal == 0 || ordinal > MAX_DOCUMENTS {
        return Err(format!(
            "namespace ordinal {ordinal} is outside the 1..={MAX_DOCUMENTS} range"
        ));
    }
    if !is_valid_run_id(run_id) {
        return Err(format!(
            "namespace run id {run_id:?} must be exactly {RUN_ID_HEX_LEN} lowercase hex characters"
        ));
    }

    let minted = format!("{base}{NAMESPACE_MARKER}{run_id}~{ordinal:04}~");
    if minted.len() > MAX_MINT_LEN {
        return Err(format!(
            "namespace {minted:?} exceeds the {MAX_MINT_LEN}-byte limit"
        ));
    }
    Ok(minted)
}

/// Runtime re-check of the mint grammar: `BASE ~it~ RUN ~ ORD ~`.
///
/// Used before a namespace value scopes a destructive action, so a
/// document-declared or ambient value can never be mistaken for a mint.
#[must_use]
pub fn is_valid_namespace_value(value: &str) -> bool {
    let Some((base, rest)) = value.split_once(NAMESPACE_MARKER) else {
        return false;
    };
    let Some((run_id, rest)) = rest.split_once('~') else {
        return false;
    };
    let Some(ordinal) = rest.strip_suffix('~') else {
        return false;
    };

    is_valid_base(base) && is_valid_run_id(run_id) && is_valid_ordinal(ordinal)
}

/// Checks the discovered-document count against [`MAX_DOCUMENTS`].
///
/// # Errors
///
/// Returns an owned error naming the count when it exceeds the cap.
#[must_use = "the discovery cap must be enforced by the caller"]
pub fn check_discovery_cap(discovered: usize) -> Result<(), String> {
    if discovered > MAX_DOCUMENTS {
        return Err(format!(
            "discovered {discovered} scenario documents exceed the {MAX_DOCUMENTS}-document limit"
        ));
    }
    Ok(())
}

/// Mints every binding into `doc_minted` (the harness layer of one
/// document's [`crate::env_layers::LayeredEnv`]).
///
/// The map is only written once every binding has minted successfully, so a
/// failure leaves the caller's map untouched.
///
/// # Errors
///
/// Returns an owned error naming the bindVar and the mint failure.
#[must_use = "the minted namespaces must reach the document's env layer"]
pub fn apply_namespaces(
    doc_minted: &mut BTreeMap<String, String>,
    bindings: &[NamespaceBinding],
    run_id: &str,
    ordinal: usize,
) -> Result<(), String> {
    let mut minted = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let value = mint_namespace(&binding.base, run_id, ordinal)
            .map_err(|err| format!("namespace bindVar {:?}: {err}", binding.bind_var))?;
        minted.push((binding.bind_var.clone(), value));
    }
    for (bind_var, value) in minted {
        doc_minted.insert(bind_var, value);
    }
    Ok(())
}

/// Validates the raw `(bindVar, base)` pairs of a document's `namespaces:`
/// section and returns the typed bindings.
///
/// `reserved` carries the keys a namespace bindVar must not shadow: the
/// document `env:` keys, the `envPassthrough` names, and the partner
/// bindVars (the document caller assembles that list).
///
/// # Errors
///
/// Returns an owned error naming the offending entry index and field when a
/// bindVar is empty, a base does not match the grammar, a bindVar collides
/// with a reserved key, or a bindVar or base repeats within the list.
#[must_use = "the validated bindings must be used by the caller"]
pub fn validate_namespace_entries(
    raw: &[(String, String)],
    reserved: &[String],
) -> Result<Vec<NamespaceBinding>, String> {
    let mut bindings: Vec<NamespaceBinding> = Vec::with_capacity(raw.len());

    for (index, (bind_var, base)) in raw.iter().enumerate() {
        if bind_var.is_empty() {
            return Err(format!("namespaces[{index}]: bindVar must not be empty"));
        }
        if !is_valid_base(base) {
            return Err(format!(
                "namespaces[{index}]: base {base:?} for bindVar {bind_var:?} must match \
                 [A-Za-z0-9][A-Za-z0-9_-]{{0,31}}"
            ));
        }
        if reserved.iter().any(|key| key == bind_var) {
            return Err(format!(
                "namespaces[{index}]: bindVar {bind_var:?} collides with a reserved key"
            ));
        }
        if bindings
            .iter()
            .any(|existing| existing.bind_var == *bind_var)
        {
            return Err(format!(
                "namespaces[{index}]: duplicate bindVar {bind_var:?} in the document"
            ));
        }
        if bindings.iter().any(|existing| existing.base == *base) {
            return Err(format!(
                "namespaces[{index}]: duplicate base {base:?} in the document"
            ));
        }

        bindings.push(NamespaceBinding {
            bind_var: bind_var.clone(),
            base: base.clone(),
        });
    }

    Ok(bindings)
}

/// `[A-Za-z0-9][A-Za-z0-9_-]{0,31}`: first char alphanumeric, at most 32
/// bytes total.
fn is_valid_base(base: &str) -> bool {
    let mut chars = base.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && base.len() <= BASE_MAX_LEN
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Exactly [`RUN_ID_HEX_LEN`] lowercase hex characters.
fn is_valid_run_id(run_id: &str) -> bool {
    run_id.len() == RUN_ID_HEX_LEN
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Four digits in the 1..=[`MAX_DOCUMENTS`] range (zero-padded, no signs).
fn is_valid_ordinal(ordinal: &str) -> bool {
    if ordinal.len() != ORDINAL_WIDTH || !ordinal.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    match ordinal.parse::<usize>() {
        Ok(parsed) => (1..=MAX_DOCUMENTS).contains(&parsed),
        Err(_) => false,
    }
}
