//! Deployment truststore codec for signature pinning (keypin Task 1.2)
//! and freshness floors (keypin Task 2.2).
//!
//! A truststore is a single UTF-8 text file owned by the deployment: each
//! non-empty, non-comment line pins exactly one verifying key as
//! `blake3:` + 64 lowercase hex characters — the manifest
//! `key_fingerprint` form — optionally followed by whitespace and a
//! decimal unsigned 64-bit freshness floor. Blank lines and lines whose
//! first non-space character is `#` are ignored. The validated floor
//! value is stored per pin, and the file's verbatim lines are retained on
//! the parse ([`TrustStore::lines`]) so a floor rewrite preserves every
//! comment, blank line, and pin in order.
//!
//! Parsing fails closed: an unreadable or non-UTF-8 store names the
//! path; a malformed line or a duplicate pin names the path and the
//! 1-based line. Every parse-failure message starts with the step token
//! `truststore-parse`; the lock and write failures (keypin Task 2.2)
//! start with `truststore-update`. The CLI diagnostic names the failing
//! step either way.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};

/// One pinned verifying key: the `blake3:<64 lowercase hex>` fingerprint
/// recorded in a truststore line (and in the manifest signing block),
/// plus the freshness floor recorded for it (keypin Task 2.2).
#[derive(Debug)]
pub(crate) struct PinEntry {
    fingerprint: String,
    floor: Option<u64>,
}

/// A parsed deployment truststore: the ordered set of pins the artifact
/// key fingerprints are checked against, plus the file's verbatim lines
/// (keypin Task 2.2) so a floor rewrite preserves the deployment's
/// comments, blank lines, and ordering.
#[derive(Debug)]
pub(crate) struct TrustStore {
    entries: Vec<PinEntry>,
    lines: Vec<String>,
}

impl TrustStore {
    /// Parse the truststore file at `path` (keypin Task 1.2).
    ///
    /// Fail-closed rules: an unreadable file is
    /// [`TrustError::Unreadable`]; non-UTF-8 bytes are
    /// [`TrustError::NotUtf8`] — both name only the path. Every other
    /// line must be exactly one `blake3:<64 lowercase hex>` token, or two
    /// tokens where the second is a strict decimal `u64`; any other shape
    /// is [`TrustError::MalformedEntry`] and an already-pinned
    /// fingerprint is [`TrustError::DuplicatePin`] — both name the path
    /// and the 1-based line. The validated floor is stored on the entry
    /// (keypin Task 2.2). Zero-byte and comments/blank-only files parse
    /// to a valid empty store that pins nothing.
    pub(crate) fn parse(path: &Path) -> Result<TrustStore, TrustError> {
        let bytes = std::fs::read(path).map_err(|_| TrustError::Unreadable {
            path: path.to_path_buf(),
        })?;
        let text = std::str::from_utf8(&bytes).map_err(|_| TrustError::NotUtf8 {
            path: path.to_path_buf(),
        })?;
        // The verbatim line set is retained for comment- and
        // order-preserving rewrites (keypin Task 2.2).
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let mut entries: Vec<PinEntry> = Vec::new();
        for (offset, raw) in lines.iter().enumerate() {
            let line = offset + 1;
            let trimmed = raw.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let tokens: Vec<&str> = raw.split_whitespace().collect();
            let (fingerprint, floor) = match tokens.as_slice() {
                [fingerprint] => ((*fingerprint).to_string(), None),
                [fingerprint, floor] => {
                    if !is_strict_u64(floor) {
                        return Err(TrustError::MalformedEntry {
                            path: path.to_path_buf(),
                            line,
                        });
                    }
                    let value = match floor.parse::<u64>() {
                        Ok(value) => value,
                        // Unreachable after `is_strict_u64`, kept total.
                        Err(_) => {
                            return Err(TrustError::MalformedEntry {
                                path: path.to_path_buf(),
                                line,
                            });
                        }
                    };
                    ((*fingerprint).to_string(), Some(value))
                }
                _ => {
                    return Err(TrustError::MalformedEntry {
                        path: path.to_path_buf(),
                        line,
                    });
                }
            };
            if !is_pinned_form(&fingerprint) {
                return Err(TrustError::MalformedEntry {
                    path: path.to_path_buf(),
                    line,
                });
            }
            if entries.iter().any(|entry| entry.fingerprint == fingerprint) {
                return Err(TrustError::DuplicatePin {
                    path: path.to_path_buf(),
                    line,
                });
            }
            entries.push(PinEntry { fingerprint, floor });
        }
        Ok(TrustStore { entries, lines })
    }

    /// Whether `fingerprint` (the manifest `key_fingerprint` form,
    /// `blake3:<hex>`) is pinned — exact string match.
    pub(crate) fn is_pinned(&self, fingerprint: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.fingerprint == fingerprint)
    }

    /// The recorded freshness floor for `fingerprint`, or `None` when the
    /// fingerprint is pinned without a floor or is not pinned at all
    /// (keypin Task 2.2).
    pub(crate) fn floor(&self, fingerprint: &str) -> Option<u64> {
        self.entries
            .iter()
            .find(|entry| entry.fingerprint == fingerprint)
            .and_then(|entry| entry.floor)
    }
}

/// Whether `pin` is exactly `blake3:` + 64 LOWERCASE hex characters —
/// the manifest `key_fingerprint` form.
fn is_pinned_form(pin: &str) -> bool {
    let Some(hex) = pin.strip_prefix("blake3:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `token` is a strict decimal `u64`: ASCII digits only (no
/// sign, no whitespace, no fraction) that fit the unsigned 64-bit range.
fn is_strict_u64(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit()) && token.parse::<u64>().is_ok()
}

/// The freshness decision of a pinned key (keypin Task 2.2): whether the
/// artifact's signed marker is acceptable against the recorded floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FreshnessVerdict {
    /// Marker at the floor (or both absent): proceed, no floor update.
    Accept,
    /// Marker above the floor, or the first sight of the key: proceed and
    /// record the marker as the new floor (boot only).
    AcceptRecord,
    /// Marker below the floor, or a schema-4 artifact with no marker while
    /// a floor is recorded: fail closed.
    Rollback,
}

/// The Step-3 policy table (keypin Task 2.2): a missing marker (legacy
/// schema 4) is below any recorded floor; a first-sight marker records;
/// equal markers accept; a higher marker records; a lower marker rolls
/// back.
pub(crate) fn decide(marker: Option<u64>, floor: Option<u64>) -> FreshnessVerdict {
    use FreshnessVerdict::{Accept, AcceptRecord, Rollback};
    match (marker, floor) {
        (None, None) => Accept,
        // A schema-4 (no-marker) artifact below a recorded floor is a
        // rollback once freshness protection has engaged for its key.
        (None, Some(_)) => Rollback,
        (Some(_), None) => AcceptRecord,
        (Some(m), Some(f)) if m < f => Rollback,
        (Some(m), Some(f)) if m == f => Accept,
        (Some(_), Some(_)) => AcceptRecord,
    }
}

/// The boot lock deadline: a boot that cannot acquire the truststore lock
/// within this window fails closed with `truststore-update` (keypin Task
/// 2.2).
pub(crate) const TRUSTSTORE_LOCK_DEADLINE: Duration = Duration::from_secs(10);

/// Retry interval of the bounded exclusive-lock acquisition loop.
const TRUSTSTORE_LOCK_RETRY: Duration = Duration::from_millis(50);

/// The sibling advisory-lock path of a truststore: `<truststore>.lock`
/// (keypin Task 2.2).
pub(crate) fn lock_path_for(truststore: &Path) -> PathBuf {
    let mut name = truststore.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// Run `f` inside the truststore's exclusive advisory lock (keypin Task
/// 2.2): open-or-create the lock file at `lock_path`, then `try_lock` in a
/// bounded retry loop (50 ms interval) until `deadline`; the lock releases
/// when this function returns. A failure to open the lock file is
/// [`TrustError::Io`]; an IO failure or the deadline while acquiring it is
/// [`TrustError::LockFailure`] — both step `truststore-update`, so the
/// caller fails closed rather than degrading to no freshness.
pub(crate) fn with_truststore_lock<T>(
    lock_path: &Path,
    deadline: Duration,
    f: impl FnOnce() -> Result<T, TrustError>,
) -> Result<T, TrustError> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        // The lock file's bytes are irrelevant: never truncate (and never
        // write) it; the flock is the whole contract.
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|_| TrustError::Io {
            path: lock_path.to_path_buf(),
        })?;
    let started = Instant::now();
    loop {
        // UFCS pins the call to the fs4 trait: std's newer inherent
        // `File::try_lock` would otherwise shadow it.
        match FileExt::try_lock(&file) {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => {
                if started.elapsed() >= deadline {
                    return Err(TrustError::LockFailure {
                        path: lock_path.to_path_buf(),
                    });
                }
                std::thread::sleep(TRUSTSTORE_LOCK_RETRY);
            }
            Err(TryLockError::Error(_)) => {
                return Err(TrustError::LockFailure {
                    path: lock_path.to_path_buf(),
                });
            }
        }
    }
    // The exclusive flock releases when `file` drops after `f` returns
    // (or unwinds).
    let result = f();
    drop(file);
    result
}

/// Record `marker` as `fingerprint`'s freshness floor in the truststore at
/// `path`, rewriting from the snapshot's verbatim `lines` (keypin Task
/// 2.2): every comment, blank line, and pin is preserved in order, and the
/// pin's own line gets a floor token of `max(existing_floor, marker)`. A
/// fingerprint with no line in `store` returns [`TrustError::Unwritable`]
/// as a defensive unreachable — the caller re-checks the pin under the
/// lock first. The rewrite goes through a sibling `.tmp` file plus rename
/// (the envelope-write precedent); any failure removes the tmp file first.
/// Callers MUST pass the snapshot parsed UNDER the lock. The rewrite is a
/// normalization: comment and blank lines, pin order, and floors are
/// preserved, while the pin's line is rewritten canonically (no leading
/// indentation) and the file always ends with a single trailing newline.
pub(crate) fn record_floor(
    store: &TrustStore,
    path: &Path,
    fingerprint: &str,
    marker: u64,
) -> Result<(), TrustError> {
    let unwritable = || TrustError::Unwritable {
        path: path.to_path_buf(),
    };
    let line_index = store
        .lines
        .iter()
        .position(|line| line.split_whitespace().next() == Some(fingerprint))
        .ok_or_else(unwritable)?;
    let new_floor = match store.floor(fingerprint) {
        Some(existing) => existing.max(marker),
        None => marker,
    };
    let mut lines = store.lines.clone();
    lines[line_index] = format!("{fingerprint} {new_floor}");
    let body = format!("{}\n", lines.join("\n"));

    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    let write = || -> std::io::Result<()> {
        std::fs::write(&tmp, body.as_bytes())?;
        std::fs::rename(&tmp, path)
    };
    if write().is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err(unwritable());
    }
    Ok(())
}

/// Truststore failure. Every parse-time variant names the failing step
/// (`truststore-parse`); the lock and write-side variants name
/// `truststore-update` (keypin Task 2.2). The CLI surfaces each as a
/// precise exit-2 diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrustError {
    /// The truststore file could not be read.
    Unreadable {
        /// The rejected truststore path.
        path: PathBuf,
    },
    /// The truststore file is not valid UTF-8.
    NotUtf8 {
        /// The rejected truststore path.
        path: PathBuf,
    },
    /// A truststore line is neither one pin token nor pin + decimal
    /// floor (or the pin is not `blake3:` + 64 lowercase hex).
    MalformedEntry {
        /// The rejected truststore path.
        path: PathBuf,
        /// The 1-based number of the offending line.
        line: usize,
    },
    /// A truststore line pins a fingerprint already pinned above it.
    DuplicatePin {
        /// The rejected truststore path.
        path: PathBuf,
        /// The 1-based number of the offending line.
        line: usize,
    },
    /// The advisory lock could not be acquired before the deadline.
    LockFailure {
        /// The lock path that could not be acquired.
        path: PathBuf,
    },
    /// The truststore could not be rewritten atomically.
    Unwritable {
        /// The truststore path that could not be updated.
        path: PathBuf,
    },
    /// The truststore lock file could not be created or opened.
    Io {
        /// The lock file path that could not be created or opened.
        path: PathBuf,
    },
}

impl fmt::Display for TrustError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path } => write!(
                f,
                "truststore-parse: cannot read the truststore at {}",
                path.display()
            ),
            Self::NotUtf8 { path } => write!(
                f,
                "truststore-parse: the truststore at {} is not valid UTF-8",
                path.display()
            ),
            Self::MalformedEntry { path, line } => write!(
                f,
                "truststore-parse: malformed truststore entry at {}:line {line}",
                path.display()
            ),
            Self::DuplicatePin { path, line } => write!(
                f,
                "truststore-parse: duplicate pin at {}:line {line}",
                path.display()
            ),
            Self::LockFailure { path } => write!(
                f,
                "truststore-update: cannot acquire the truststore lock at {}",
                path.display()
            ),
            Self::Unwritable { path } => write!(
                f,
                "truststore-update: cannot update the truststore at {}",
                path.display()
            ),
            Self::Io { path } => write!(
                f,
                "truststore-update: truststore IO failed at {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for TrustError {}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        FreshnessVerdict, TRUSTSTORE_LOCK_DEADLINE, TrustError, TrustStore, decide, lock_path_for,
        record_floor, with_truststore_lock,
    };

    /// Two 64-char lowercase hex bodies used as distinct valid pins.
    const HEX_A: &str = concat!(
        "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0",
        "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0"
    );
    const HEX_B: &str = concat!(
        "beefbeefbeefbeefbeefbeefbeefbeef",
        "beefbeefbeefbeefbeefbeefbeefbeef"
    );

    fn pin(hex: &str) -> String {
        format!("blake3:{hex}")
    }

    fn write_store(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write truststore fixture");
        path
    }

    /// Pins, comments, blank lines, and a valid floor column parse Ok in
    /// file order; the floor value is stored on the entry (keypin
    /// Task 2.2).
    #[test]
    fn truststore_parse_accepts_pins_comments_and_valid_floor_column() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = format!(
            "# deployment pins\n# maintained by ops\n\n{}\n{} 42\n",
            pin(HEX_A),
            pin(HEX_B)
        );
        let path = write_store(dir.path(), "pins.keys", body.as_bytes());

        let store = TrustStore::parse(&path).expect("pins and floor parse");

        assert_eq!(store.entries.len(), 2, "two pins in file order");
        assert_eq!(store.entries[0].fingerprint, pin(HEX_A));
        assert_eq!(store.entries[1].fingerprint, pin(HEX_B));
        assert_eq!(store.entries[0].floor, None, "the first pin has no floor");
        assert_eq!(
            store.entries[1].floor,
            Some(42),
            "the second pin stores its floor"
        );
    }

    /// A line that is neither a pin nor pin + floor is rejected with the
    /// path and the 1-based line number in the Display message.
    #[test]
    fn truststore_parse_rejects_malformed_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = format!("{}\n{}\nnot-a-pin\n", pin(HEX_A), pin(HEX_B));
        let path = write_store(dir.path(), "bad.keys", body.as_bytes());

        let err = TrustStore::parse(&path).expect_err("malformed line must fail");
        assert_eq!(
            err,
            TrustError::MalformedEntry {
                path: path.clone(),
                line: 3
            }
        );
        let message = err.to_string();
        assert!(
            message.contains(path.display().to_string().as_str()),
            "{message}"
        );
        assert!(message.contains("line 3"), "{message}");
    }

    /// The same fingerprint pinned twice is rejected, naming the SECOND
    /// (duplicate) line.
    #[test]
    fn truststore_parse_rejects_duplicate_pin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = format!("{}\n# note\n\n{}\n", pin(HEX_A), pin(HEX_A));
        let path = write_store(dir.path(), "dup.keys", body.as_bytes());

        let err = TrustStore::parse(&path).expect_err("duplicate pin must fail");
        assert_eq!(
            err,
            TrustError::DuplicatePin {
                path: path.clone(),
                line: 4
            }
        );
        let message = err.to_string();
        assert!(
            message.contains(path.display().to_string().as_str()),
            "{message}"
        );
        assert!(message.contains("line 4"), "{message}");
    }

    /// Non-UTF-8 bytes (`"blake3:"` + 0xff) are rejected naming the path.
    #[test]
    fn truststore_parse_rejects_non_utf8() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_store(
            dir.path(),
            "binary.keys",
            &[0x62, 0x6c, 0x61, 0x6b, 0x65, 0x33, 0x3a, 0xff],
        );

        let err = TrustStore::parse(&path).expect_err("non-UTF-8 must fail");
        assert_eq!(err, TrustError::NotUtf8 { path: path.clone() });
        assert!(
            err.to_string()
                .contains(path.display().to_string().as_str()),
            "{err}"
        );
    }

    /// A missing file is rejected as unreadable, naming the path.
    #[test]
    fn truststore_parse_unreadable_names_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing.keys");

        let err = TrustStore::parse(&path).expect_err("missing file must fail");
        assert_eq!(err, TrustError::Unreadable { path: path.clone() });
        assert!(
            err.to_string()
                .contains(path.display().to_string().as_str()),
            "{err}"
        );
    }

    /// Zero-byte and comments/blank-only files are valid and pin nothing.
    #[test]
    fn truststore_empty_and_comments_only_files_pin_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let empty = write_store(dir.path(), "empty.keys", b"");
        let comments = write_store(
            dir.path(),
            "comments.keys",
            b"# no pins yet\n\n# still none\n",
        );

        let empty = TrustStore::parse(&empty).expect("zero-byte store parses");
        let comments = TrustStore::parse(&comments).expect("comments-only store parses");

        assert!(store_pins_none(&empty));
        assert!(store_pins_none(&comments));
    }

    fn store_pins_none(store: &TrustStore) -> bool {
        !store.is_pinned(&pin(HEX_A)) && !store.is_pinned(&pin(HEX_B))
    }

    /// Fingerprint shapes other than exactly `blake3:` + 64 LOWERCASE
    /// hex are malformed: uppercase hex, 63 chars, missing prefix, 65
    /// chars.
    #[test]
    fn truststore_rejects_bad_fingerprint_shapes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let uppercase = format!("blake3:{}\n", HEX_A.to_uppercase());
        let short63 = format!("blake3:{}\n", &HEX_A[..63]);
        let no_prefix = format!("{HEX_A}\n");
        let long65 = format!("blake3:{HEX_A}a\n");
        for (name, body) in [
            ("uppercase.keys", uppercase),
            ("short.keys", short63),
            ("noprefix.keys", no_prefix),
            ("long.keys", long65),
        ] {
            let path = write_store(dir.path(), name, body.as_bytes());
            let err = TrustStore::parse(&path).expect_err("bad fingerprint shape must fail");
            assert_eq!(
                err,
                TrustError::MalformedEntry {
                    path: path.clone(),
                    line: 1
                },
                "{name}"
            );
        }
    }

    /// The optional floor column must be a strict decimal u64: `abc`,
    /// `-1`, `+1`, and `1.5` are all malformed.
    #[test]
    fn truststore_floor_column_must_be_decimal_u64() {
        let dir = tempfile::tempdir().expect("tempdir");
        for (idx, floor) in ["abc", "-1", "+1", "1.5"].iter().enumerate() {
            let path = write_store(
                dir.path(),
                &format!("floor{idx}.keys"),
                format!("{} {floor}\n", pin(HEX_A)).as_bytes(),
            );
            let err = TrustStore::parse(&path).expect_err("bad floor must fail");
            assert_eq!(
                err,
                TrustError::MalformedEntry {
                    path: path.clone(),
                    line: 1
                },
                "floor token {floor:?}"
            );
        }
    }

    /// The parsed floor column is stored per pin: a pin with floor `42`
    /// reports `Some(42)`, a pin without a floor reports `None`, and an
    /// unpinned fingerprint reports `None` (keypin Task 2.2).
    #[test]
    fn truststore_parse_stores_floors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = format!("{}\n{} 42\n", pin(HEX_A), pin(HEX_B));
        let path = write_store(dir.path(), "floors.keys", body.as_bytes());

        let store = TrustStore::parse(&path).expect("floors parse");

        assert_eq!(
            store.floor(&pin(HEX_A)),
            None,
            "a pin without a floor has none"
        );
        assert_eq!(
            store.floor(&pin(HEX_B)),
            Some(42),
            "the floor column is stored"
        );
        let unpinned = format!("blake3:{}", "f".repeat(64));
        assert_eq!(
            store.floor(&unpinned),
            None,
            "an unpinned fingerprint has no floor"
        );
    }

    /// The freshness decision covers the full Step-3 policy table,
    /// including the equal-marker `Accept` row (keypin Task 2.2).
    #[test]
    fn decide_covers_the_policy_table() {
        use super::FreshnessVerdict::{Accept, AcceptRecord, Rollback};

        assert!(
            matches!(decide(None, None), Accept),
            "(None, None) -> Accept"
        );
        assert!(
            matches!(decide(None, Some(42)), Rollback),
            "(None, Some(_)) -> Rollback (legacy schema-4 below a floor)"
        );
        assert!(
            matches!(decide(Some(42), None), AcceptRecord),
            "(Some(m), None) -> AcceptRecord"
        );
        assert!(
            matches!(decide(Some(41), Some(42)), Rollback),
            "(Some(m), Some(f)) with m < f -> Rollback"
        );
        assert!(
            matches!(decide(Some(42), Some(42)), Accept),
            "(Some(m), Some(m)) -> Accept"
        );
        assert!(
            matches!(decide(Some(43), Some(42)), AcceptRecord),
            "(Some(m), Some(f)) with m > f -> AcceptRecord"
        );
    }

    /// A held flock on `<ts>.lock` makes the bounded retry loop fail
    /// closed at the deadline: `LockFailure` naming the lock path
    /// (keypin Task 2.2).
    #[test]
    fn lock_deadline_fails_closed() {
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("tempdir");
        let ts = dir.path().join("pins.keys");
        std::fs::write(&ts, format!("{}\n", pin(HEX_A))).expect("write truststore");
        let lock_path = lock_path_for(&ts);

        // Hold the flock from the test via fs4.
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("open the lock file");
        use fs4::FileExt;
        FileExt::lock(&lock_file).expect("hold the lock");

        let err = with_truststore_lock(&lock_path, Duration::from_millis(150), || Ok(()))
            .expect_err("the deadline must fail closed");
        assert_eq!(
            err,
            TrustError::LockFailure {
                path: lock_path.clone()
            }
        );
        assert!(
            err.to_string()
                .contains(lock_path.display().to_string().as_str()),
            "the diagnostic names the lock path: {err}"
        );
    }

    /// Two threads run the boot-path lock-guarded sequence — lock,
    /// re-parse, re-check the pin, decide, record on accept — with
    /// markers 100 and 200. The recorded floor settles on the max, no
    /// accepting thread observed a stale floor above its own marker, and
    /// a follow-up decide at the lower marker against the settled floor
    /// is a rollback (keypin Task 2.2).
    #[test]
    fn concurrent_floor_writes_settle_on_the_max() {
        use std::path::Path;

        let dir = tempfile::tempdir().expect("tempdir");
        let ts = dir.path().join("pins.keys");
        let fingerprint = pin(HEX_A);
        std::fs::write(&ts, format!("{fingerprint}\n")).expect("write truststore");
        let lock_path = lock_path_for(&ts);

        /// One boot-path pass: the under-lock re-parse, the pin
        /// re-check, the decision, and the record on accept. Reports the
        /// floor observed at decision time and the verdict.
        fn boot_pass(
            ts: &Path,
            lock_path: &Path,
            fingerprint: &str,
            marker: u64,
        ) -> (Option<u64>, FreshnessVerdict) {
            with_truststore_lock(lock_path, TRUSTSTORE_LOCK_DEADLINE, || {
                let fresh = TrustStore::parse(ts).expect("re-parse under the lock");
                assert!(fresh.is_pinned(fingerprint), "the pin re-check passes");
                let floor = fresh.floor(fingerprint);
                let verdict = decide(Some(marker), floor);
                if matches!(verdict, FreshnessVerdict::AcceptRecord) {
                    record_floor(&fresh, ts, fingerprint, marker).expect("record the floor");
                }
                Ok((floor, verdict))
            })
            .expect("the lock-guarded sequence completes")
        }

        let ts_a = ts.clone();
        let lock_a = lock_path.clone();
        let fp_a = fingerprint.clone();
        let a = std::thread::spawn(move || boot_pass(&ts_a, &lock_a, &fp_a, 100));
        let ts_b = ts.clone();
        let lock_b = lock_path.clone();
        let fp_b = fingerprint.clone();
        let b = std::thread::spawn(move || boot_pass(&ts_b, &lock_b, &fp_b, 200));
        let (floor_a, verdict_a) = a.join().expect("thread a joins");
        let (floor_b, verdict_b) = b.join().expect("thread b joins");

        // The recorded floor is the max marker.
        let final_store = TrustStore::parse(&ts).expect("the final store parses");
        assert_eq!(
            final_store.floor(&fingerprint),
            Some(200),
            "the recorded floor settles on the max marker"
        );

        // Every accepting thread observed a floor <= its own marker: no
        // stale-accept on a floor above the marker.
        for (floor, verdict, marker) in [(floor_a, verdict_a, 100), (floor_b, verdict_b, 200)] {
            if !matches!(verdict, FreshnessVerdict::Rollback) {
                assert!(
                    floor.is_none_or(|f| f <= marker),
                    "the thread at marker {marker} accepted on floor {floor:?}"
                );
            }
        }

        // A follow-up decide at the lower marker against the settled
        // floor is a rollback.
        assert!(
            matches!(decide(Some(100), Some(200)), FreshnessVerdict::Rollback),
            "the lower marker must roll back against the settled floor"
        );
    }
}
