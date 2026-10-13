#![allow(dead_code)]

//! Reader for the Java bridge's durable attempt-journal frame format.
//!
//! Task 5.2 must prove the survivor inherits the reservation count from the
//! **durable journal**, not merely from the broker's `JMSXDeliveryCount` (both
//! happen to be 2 on a single redelivery, so the broker count alone cannot
//! distinguish a fresh journal from an inherited one). This module reproduces
//! the exact on-disk format written by
//! `bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java`:
//!
//! ```text
//! header:  'R' 'C' 'J' 'N'  <u8 format version = 1>
//! frame:   [u32 payload_len][u8 type][payload]
//! payload: [u16 id_len][id utf8][i64 last_touched_ms]
//! ```
//!
//! All integers are big endian. `payload_len` counts the payload bytes only and
//! is bounded to `[10, 10 + 0xFFFF]`. A partial trailing frame is a torn tail
//! and is discarded, exactly as replay does. Record types: `RESERVE = 1`,
//! `COMMIT = 2`, `ROLLBACK = 3`, `RETIRE = 4`. `COMMIT` and `RETIRE` remove the
//! entry; `ROLLBACK` keeps the count. This mirrors `JournalState`/`applyRecord`.
//!
//! Deriving the journal path reproduces the component's own derivation:
//! `<XDG_STATE_HOME>/rust-camel/jms-bridge/<broker-key>/<destination-key>.journal`,
//! with the broker key from [`attempt_journal_broker_key`] and the destination
//! keyed by the single `*.journal` file under the dedicated broker directory
//! (this test owns a dedicated Classic container and one queue, so exactly one
//! journal exists).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use camel_bridge::process::BrokerType;

/// On-disk magic, matching `JournalService.MAGIC`.
const MAGIC: &[u8; 4] = b"RCJN";
/// On-disk format version, matching `JournalService.FORMAT_VERSION`.
const FORMAT_VERSION: u8 = 1;
/// `MAGIC` plus the one-byte version.
const HEADER_LENGTH: usize = 5;
/// `u16 id length` plus `i64 last-touched`.
const MIN_PAYLOAD_LEN: usize = 2 + 8;
/// `u16` id length caps the frame payload.
const MAX_PAYLOAD_LEN: usize = MIN_PAYLOAD_LEN + 0xFFFF;

/// `JournalService.TYPE_RESERVE`.
pub const TYPE_RESERVE: u8 = 1;
/// `JournalService.TYPE_COMMIT`.
pub const TYPE_COMMIT: u8 = 2;
/// `JournalService.TYPE_ROLLBACK`.
pub const TYPE_ROLLBACK: u8 = 3;
/// `JournalService.TYPE_RETIRE`.
pub const TYPE_RETIRE: u8 = 4;

/// One decoded journal record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalFrame {
    /// Record type byte (one of the `TYPE_*` constants).
    pub kind: u8,
    /// The reservation key: the provider `JMSMessageID`.
    pub message_id: String,
}

/// The dedicated broker directory for `broker_name`/`broker_url` under
/// `xdg_state_home`, using the component's own broker-key derivation so the
/// test reads the exact file the bridge writes.
pub fn journal_broker_dir(
    xdg_state_home: &Path,
    broker_name: &str,
    broker_type: &BrokerType,
    broker_url: &str,
) -> PathBuf {
    let broker_key = camel_component_jms::config::attempt_journal_broker_key(
        broker_name,
        broker_type,
        broker_url,
    );
    xdg_state_home
        .join("rust-camel")
        .join("jms-bridge")
        .join(broker_key)
}

/// Returns the single `*.journal` file under `broker_dir`, or `None` while the
/// bridge has not created it yet. Panics if more than one exists, because the
/// test's dedicated broker/queue must resolve exactly one journal domain.
pub fn find_journal_file(broker_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(broker_dir).ok()?;
    let mut found: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("journal") {
            if let Some(existing) = &found {
                panic!(
                    "more than one journal under dedicated broker dir {}: {:?} and {path:?}",
                    broker_dir.display(),
                    existing,
                );
            }
            found = Some(path);
        }
    }
    found
}

/// Decodes every complete frame, discarding a torn trailing frame. Fails closed
/// on a bad magic/version, an out-of-range length, a mismatched id length, or a
/// non-UTF-8 id — mirroring `replayBytes`/`applyRecord`.
pub fn read_frames(path: &Path) -> Result<Vec<JournalFrame>, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("cannot read journal {}: {e}", path.display()))?;
    if bytes.len() < HEADER_LENGTH {
        return Ok(Vec::new());
    }
    if &bytes[0..4] != MAGIC {
        return Err(format!("journal magic mismatch in {}", path.display()));
    }
    if bytes[4] != FORMAT_VERSION {
        return Err(format!(
            "unsupported journal format version {} in {}",
            bytes[4],
            path.display()
        ));
    }

    let mut frames = Vec::new();
    let mut pos = HEADER_LENGTH;
    while pos < bytes.len() {
        let frame_start = pos;
        if bytes.len() - frame_start < 4 {
            break; // torn tail: partial length field
        }
        let payload_len = u32::from_be_bytes([
            bytes[frame_start],
            bytes[frame_start + 1],
            bytes[frame_start + 2],
            bytes[frame_start + 3],
        ]) as usize;
        if !(MIN_PAYLOAD_LEN..=MAX_PAYLOAD_LEN).contains(&payload_len) {
            return Err(format!(
                "journal frame length {payload_len} out of range in {}",
                path.display()
            ));
        }
        let frame_end = frame_start + 4 + 1 + payload_len;
        if frame_end > bytes.len() {
            break; // torn tail: partial record body
        }
        let kind = bytes[frame_start + 4];
        if !matches!(
            kind,
            TYPE_RESERVE | TYPE_COMMIT | TYPE_ROLLBACK | TYPE_RETIRE
        ) {
            // Production `applyRecord` fails closed on an unsupported type; an
            // unknown kind is corruption, never silently ignored.
            return Err(format!(
                "unsupported journal record type {kind} in {}",
                path.display()
            ));
        }
        let payload = &bytes[frame_start + 5..frame_end];
        let id_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
        if id_len != payload_len - MIN_PAYLOAD_LEN {
            return Err(format!(
                "journal record id length {id_len} out of range in {}",
                path.display()
            ));
        }
        if id_len == 0 {
            // Production `decodeMessageId` rejects a blank id as corruption.
            return Err(format!(
                "journal record message id is blank in {}",
                path.display()
            ));
        }
        let message_id = std::str::from_utf8(&payload[2..2 + id_len])
            .map_err(|e| format!("journal id is not valid UTF-8 in {}: {e}", path.display()))?
            .to_string();
        frames.push(JournalFrame { kind, message_id });
        pos = frame_end;
    }
    Ok(frames)
}

/// Replays `frames` with the Java `JournalState` semantics and returns the
/// durable reservation count for `message_id`. `RESERVE` increments; `COMMIT`
/// and `RETIRE` remove the entry; `ROLLBACK` keeps the count.
pub fn reservation_count(frames: &[JournalFrame], message_id: &str) -> u64 {
    let mut counts: HashMap<&str, u64> = HashMap::new();
    for frame in frames {
        match frame.kind {
            TYPE_RESERVE => *counts.entry(frame.message_id.as_str()).or_insert(0) += 1,
            TYPE_COMMIT | TYPE_RETIRE => {
                counts.remove(frame.message_id.as_str());
            }
            TYPE_ROLLBACK => {}
            _ => {}
        }
    }
    counts.get(message_id).copied().unwrap_or(0)
}

/// True when a `COMMIT` frame for `message_id` was ever written. The crash
/// proof requires this to be false: no settlement may have been observed.
pub fn commit_observed(frames: &[JournalFrame], message_id: &str) -> bool {
    frames
        .iter()
        .any(|f| f.kind == TYPE_COMMIT && f.message_id == message_id)
}

/// Counts every frame of `kind` recorded for `message_id`. The threshold proof
/// reads the durable frames directly (not the broker's `JMSXDeliveryCount`):
/// the bridge records exactly one `RESERVE` per handoff and one `COMMIT` for
/// the terminal acknowledgement.
pub fn count_kind(frames: &[JournalFrame], kind: u8, message_id: &str) -> usize {
    frames
        .iter()
        .filter(|f| f.kind == kind && f.message_id == message_id)
        .count()
}

/// The `.lock` file beside a destination journal (same stem).
pub fn lock_path(journal: &Path) -> PathBuf {
    journal.with_extension("lock")
}

/// Fallible `F_GETLK` evidence of which process owns the destination record
/// lock.
///
/// Returns `Ok(Some(pid))` when another process holds a conflicting lock,
/// `Ok(None)` when no conflicting lock exists, and `Err` when the lock file
/// cannot be opened or `F_GETLK` fails. An apparatus error must never be read
/// as "unlocked": that would let the crash proof claim a release that was never
/// observed. `None` additionally requires an `F_UNLCK` report, so a
/// non-positive `l_pid` on a held lock is an error, not an owner.
#[cfg(unix)]
pub fn lock_owner_pid(path: &Path) -> Result<Option<u32>, String> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::File::open(path)
        .map_err(|e| format!("cannot open lock file {}: {e}", path.display()))?;
    // SAFETY: `flock` is a plain C struct; zeroing then setting the fields is
    // the documented way to prepare an F_GETLK request.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as libc::c_short;
    lock.l_whence = libc::SEEK_SET as libc::c_short;
    lock.l_start = 0;
    lock.l_len = 0;
    // SAFETY: `file` owns a valid fd for the duration of the call; `&mut lock`
    // is a valid pointer to an initialized `flock`.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut lock) };
    if rc != 0 {
        return Err(format!(
            "F_GETLK failed on {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    if lock.l_type == libc::F_UNLCK as libc::c_short {
        return Ok(None);
    }
    if lock.l_pid > 0 {
        return Ok(Some(lock.l_pid as u32));
    }
    Err(format!(
        "F_GETLK reported a lock on {} with non-positive l_pid {}",
        path.display(),
        lock.l_pid
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes one frame exactly as `JournalService.encodeFrame` does, so the
    /// reader is exercised against the real byte layout.
    fn encode_raw(type_byte: u8, id_bytes: &[u8]) -> Vec<u8> {
        let payload_len = MIN_PAYLOAD_LEN + id_bytes.len();
        let mut frame = Vec::with_capacity(4 + 1 + payload_len);
        frame.extend_from_slice(&(payload_len as u32).to_be_bytes());
        frame.push(type_byte);
        frame.extend_from_slice(&(id_bytes.len() as u16).to_be_bytes());
        frame.extend_from_slice(id_bytes);
        frame.extend_from_slice(&0i64.to_be_bytes());
        frame
    }

    fn encode(type_byte: u8, id: &str) -> Vec<u8> {
        encode_raw(type_byte, id.as_bytes())
    }

    fn journal_from(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.push(FORMAT_VERSION);
        for frame in frames {
            bytes.extend_from_slice(frame);
        }
        bytes
    }

    fn write_temp(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().expect("temp journal dir");
        let path = dir.path().join("test.journal");
        std::fs::write(&path, bytes).expect("write temp journal");
        (dir, path)
    }

    #[test]
    fn reserve_then_commit_replays_zero() {
        let bytes = journal_from(&[encode(TYPE_RESERVE, "ID:1"), encode(TYPE_COMMIT, "ID:1")]);
        let (_dir, path) = write_temp(&bytes);
        let frames = read_frames(&path).expect("read");
        assert_eq!(reservation_count(&frames, "ID:1"), 0);
        assert!(commit_observed(&frames, "ID:1"));
    }

    #[test]
    fn two_reserves_with_rollback_replay_two() {
        let bytes = journal_from(&[
            encode(TYPE_RESERVE, "ID:1"),
            encode(TYPE_ROLLBACK, "ID:1"),
            encode(TYPE_RESERVE, "ID:1"),
        ]);
        let (_dir, path) = write_temp(&bytes);
        let frames = read_frames(&path).expect("read");
        assert_eq!(reservation_count(&frames, "ID:1"), 2);
        assert!(!commit_observed(&frames, "ID:1"));
    }

    #[test]
    fn torn_trailing_frame_is_discarded() {
        let mut bytes = journal_from(&[encode(TYPE_RESERVE, "ID:1")]);
        let partial = encode(TYPE_RESERVE, "ID:2");
        bytes.extend_from_slice(&partial[..partial.len() - 3]);
        let (_dir, path) = write_temp(&bytes);
        let frames = read_frames(&path).expect("read");
        assert_eq!(reservation_count(&frames, "ID:1"), 1);
        assert_eq!(reservation_count(&frames, "ID:2"), 0);
    }

    #[test]
    fn empty_id_is_corrupt() {
        // Production `decodeMessageId` rejects a blank id; the reader must too,
        // not manufacture a key the writer never recorded.
        let bytes = journal_from(&[encode_raw(TYPE_RESERVE, b"")]);
        let (_dir, path) = write_temp(&bytes);
        assert!(read_frames(&path).is_err(), "empty id must fail closed");
    }

    #[test]
    fn unsupported_record_type_is_corrupt() {
        // Production `applyRecord` throws on an unknown type; an unknown kind is
        // corruption, never silently ignored.
        let bytes = journal_from(&[encode(9, "ID:1")]);
        let (_dir, path) = write_temp(&bytes);
        assert!(
            read_frames(&path).is_err(),
            "unsupported record type must fail closed"
        );
    }

    #[test]
    fn invalid_utf8_id_is_corrupt() {
        let bytes = journal_from(&[encode_raw(TYPE_RESERVE, &[0xff, 0xfe])]);
        let (_dir, path) = write_temp(&bytes);
        assert!(read_frames(&path).is_err(), "non-UTF-8 id must fail closed");
    }
}
