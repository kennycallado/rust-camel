//! Confined retrieval of file-backed JSON Schema references (ADR-0100).
//!
//! Every reference the compiler resolves is handed to
//! [`ConfinedFileRetriever`], which turns the resolved URI back into a real
//! file path and refuses anything that is remote, unparseable, outside the
//! root schema directory, or not a regular file. Confinement is checked on
//! the canonical file path, never on URI text alone, so `$id`-driven rebases
//! pass through the same rules as direct `$ref`s.
//!
//! Reads are bounded by [`MAX_FILE_BYTES`] and accounted against a
//! per-compilation [`Budget`]: one root resource plus every distinct external
//! path, up to [`MAX_RESOURCES`] resources and [`MAX_TOTAL_BYTES`] bytes.

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use jsonschema::Uri;
use noyalib::compat::serde_yaml as serde_yml;
use serde_json::Value;

use crate::CaptureSlot;
use crate::MAX_FILE_BYTES;
use crate::MAX_RESOURCES;
use crate::MAX_TOTAL_BYTES;
use crate::SchemaError;

/// A reference target that passed every confinement check.
pub(crate) struct ResolvedResource {
    /// Canonical path the reference resolved to.
    pub(crate) canonical_path: PathBuf,
    /// Resource contents, read from the already-verified file handle.
    pub(crate) bytes: Vec<u8>,
}

/// Per-compilation retrieval budget (ADR-0100).
///
/// The root document is resource 1. Each distinct canonical path retrieved
/// afterwards counts once, so repeated `$ref`s to the same file are free. The
/// budget lives inside one retriever, which is created per [`crate::compile`]
/// call, so it never spans compilations.
#[derive(Debug, Default)]
pub(crate) struct Budget {
    /// Number of distinct resources admitted so far.
    resources: usize,
    /// Total bytes admitted so far.
    total_bytes: u64,
    /// Canonical paths already counted.
    seen: HashSet<PathBuf>,
}

impl Budget {
    /// Admit one retrieved resource, counting it once per canonical path.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError::TooManyResources`] when a new canonical path
    /// would pass [`MAX_RESOURCES`], and [`SchemaError::TotalBudgetExceeded`]
    /// when its bytes would pass [`MAX_TOTAL_BYTES`].
    pub(crate) fn record(&mut self, canonical_path: &Path, bytes: u64) -> Result<(), SchemaError> {
        if !self.seen.insert(canonical_path.to_path_buf()) {
            return Ok(());
        }

        let resources = self
            .resources
            .checked_add(1)
            .ok_or(SchemaError::TooManyResources {
                limit: MAX_RESOURCES,
            })?;
        if resources > MAX_RESOURCES {
            return Err(SchemaError::TooManyResources {
                limit: MAX_RESOURCES,
            });
        }
        self.resources = resources;

        let total_bytes =
            self.total_bytes
                .checked_add(bytes)
                .ok_or(SchemaError::TotalBudgetExceeded {
                    limit: MAX_TOTAL_BYTES,
                })?;
        if total_bytes > MAX_TOTAL_BYTES {
            return Err(SchemaError::TotalBudgetExceeded {
                limit: MAX_TOTAL_BYTES,
            });
        }
        self.total_bytes = total_bytes;
        Ok(())
    }
}

/// Read at most `limit` bytes from `path`.
///
/// The read asks for one byte past the limit: a file that grew after any
/// earlier check still trips the cap, which a metadata-only length check
/// would miss.
///
/// # Errors
///
/// Returns [`SchemaError::Open`] or [`SchemaError::Read`] on I/O failure and
/// [`SchemaError::FileTooLarge`] when the file holds more than `limit` bytes.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, SchemaError> {
    let display_path = path.display().to_string();
    let file = fs::File::open(path).map_err(|error| SchemaError::Open {
        path: display_path.clone(),
        message: error.to_string(),
    })?;
    read_limited(file, limit, &display_path)
}

/// Read at most `limit` bytes from an already-open reader.
fn read_limited(reader: impl Read, limit: u64, display_path: &str) -> Result<Vec<u8>, SchemaError> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| SchemaError::Read {
            path: display_path.to_string(),
            message: error.to_string(),
        })?;
    if bytes.len() as u64 > limit {
        return Err(SchemaError::FileTooLarge {
            path: display_path.to_string(),
            limit,
        });
    }
    Ok(bytes)
}

/// Resolve `uri` inside `root_dir`, checking every confinement rule in order.
///
/// The checks run before the file is opened, so a FIFO target is rejected
/// from metadata instead of blocking in `open`.
pub(crate) fn resolve_confined(
    uri: &str,
    root_dir: &Path,
    budget: &mut Budget,
) -> Result<ResolvedResource, SchemaError> {
    let root = root_dir.display().to_string();
    let reference_path = decoded_path(uri, &root)?;
    let candidate = if reference_path.is_absolute() {
        reference_path
    } else {
        root_dir.join(reference_path)
    };
    // Name the decoded path in rejections: the URI form hides traversal
    // behind percent-encoding, and the real path is what the check rejected.
    let decoded = candidate.display().to_string();

    let canonical_path =
        fs::canonicalize(&candidate).map_err(|_| SchemaError::ExternalRefRejected {
            uri: decoded.clone(),
            root: root.clone(),
        })?;

    if !canonical_path.starts_with(root_dir) {
        return Err(SchemaError::ExternalRefRejected { uri: decoded, root });
    }

    let display_path = canonical_path.display().to_string();
    let metadata = fs::metadata(&canonical_path).map_err(|error| SchemaError::Open {
        path: display_path.clone(),
        message: error.to_string(),
    })?;
    if !metadata.file_type().is_file() {
        return Err(SchemaError::NotARegularFile { path: display_path });
    }

    let mut file = fs::File::open(&canonical_path).map_err(|error| SchemaError::Open {
        path: display_path.clone(),
        message: error.to_string(),
    })?;
    // TOCTOU hardening: the pre-open check saw the metadata of the canonical
    // path; re-check the handle that is actually being read.
    let open_metadata = file.metadata().map_err(|error| SchemaError::Read {
        path: display_path.clone(),
        message: error.to_string(),
    })?;
    if !open_metadata.file_type().is_file() {
        return Err(SchemaError::NotARegularFile { path: display_path });
    }

    let bytes = read_limited(&mut file, MAX_FILE_BYTES, &display_path)?;
    budget.record(&canonical_path, bytes.len() as u64)?;

    Ok(ResolvedResource {
        canonical_path,
        bytes,
    })
}

/// Extract the filesystem path from the scheme-checked reference `uri`.
///
/// Upstream resolves every reference against the base URI before retrieval,
/// so `uri` is normally absolute. Scheme-less input is still handled
/// fail-closed by resolving it against `root_dir`.
fn decoded_path(uri: &str, root: &str) -> Result<PathBuf, SchemaError> {
    let encoded = match Uri::parse(uri.to_string()) {
        Ok(absolute) => match absolute.scheme().as_str() {
            "file" | "" => absolute.path().as_str().to_string(),
            _ => {
                return Err(SchemaError::RemoteRefRejected {
                    uri: uri.to_string(),
                });
            }
        },
        // Not an absolute URI. A reference that still carries a non-`file`
        // scheme is remote; one with a `file` scheme but broken syntax, or
        // no scheme at all, can only escape the root and is rejected below.
        Err(_) => match scheme_of(uri) {
            Some("file") => {
                return Err(SchemaError::ExternalRefRejected {
                    uri: uri.to_string(),
                    root: root.to_string(),
                });
            }
            Some(_) => {
                return Err(SchemaError::RemoteRefRejected {
                    uri: uri.to_string(),
                });
            }
            None => relative_path(uri).to_string(),
        },
    };

    let decoded = percent_encoding::percent_decode_str(&encoded)
        .decode_utf8()
        .map_err(|_| SchemaError::ExternalRefRejected {
            uri: uri.to_string(),
            root: root.to_string(),
        })?;
    Ok(PathBuf::from(decoded.as_ref()))
}

/// RFC 3986 scheme prefix of `reference`, if it has one.
fn scheme_of(reference: &str) -> Option<&str> {
    let (candidate, _) = reference.split_once(':')?;
    let mut characters = candidate.chars();
    let first = characters.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.'))
        .then_some(candidate)
}

/// Path component of a relative reference, without query or fragment.
fn relative_path(reference: &str) -> &str {
    reference.split(['#', '?']).next().unwrap_or(reference)
}

/// Parse a retrieved resource by the target file extension.
fn parse_resource(resource: &ResolvedResource) -> Result<Value, SchemaError> {
    let path = resource.canonical_path.display().to_string();
    let yaml = matches!(
        resource
            .canonical_path
            .extension()
            .and_then(std::ffi::OsStr::to_str),
        Some("yaml" | "yml")
    );

    if yaml {
        let text = std::str::from_utf8(&resource.bytes).map_err(|error| SchemaError::Utf8 {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let document: serde_yml::Value =
            serde_yml::from_str(text).map_err(|error| SchemaError::Parse {
                message: format!("invalid YAML schema '{path}': {error}"),
                path: path.clone(),
            })?;
        serde_json::to_value(&document).map_err(|error| SchemaError::Parse {
            message: format!("failed to convert YAML schema to JSON '{path}': {error}"),
            path,
        })
    } else {
        serde_json::from_slice(&resource.bytes).map_err(|error| SchemaError::Parse {
            message: format!("invalid JSON schema '{path}': {error}"),
            path,
        })
    }
}

/// File retriever confined to the root schema directory.
pub(crate) struct ConfinedFileRetriever {
    /// Canonical directory every reference must stay inside.
    root_dir: PathBuf,
    /// Shared slot that keeps the typed failure before it is boxed.
    capture: CaptureSlot,
    /// Per-compilation resource accounting.
    budget: Mutex<Budget>,
}

impl ConfinedFileRetriever {
    /// Build a retriever rooted at `root_dir`, reporting into `capture` and
    /// accounting every retrieved resource against `budget`.
    pub(crate) fn new(root_dir: PathBuf, capture: CaptureSlot, budget: Budget) -> Self {
        Self {
            root_dir,
            capture,
            budget: Mutex::new(budget),
        }
    }

    /// Resolve and parse one reference, keeping the typed error.
    fn retrieve_confined(&self, uri: &str) -> Result<Value, SchemaError> {
        let mut budget = self.budget.lock().map_err(|_| SchemaError::Compile {
            message: "schema retrieval budget lock poisoned".to_string(),
        })?;
        let resource = resolve_confined(uri, &self.root_dir, &mut budget)?;
        drop(budget);
        parse_resource(&resource)
    }
}

impl jsonschema::Retrieve for ConfinedFileRetriever {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        match self.retrieve_confined(uri.as_str()) {
            Ok(value) => Ok(value),
            Err(error) => {
                if let Ok(mut captured) = self.capture.lock() {
                    *captured = Some(error.clone());
                }
                Err(Box::new(error))
            }
        }
    }
}
