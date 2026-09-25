//! Process-global, read-only asset registry for embedded deploy-time
//! assets (r2embed task 3.1; design "In-memory serving").
//!
//! The compiled-artifact runtime populates this registry from the decoded
//! asset entries exactly once, before boot, after verifying every entry's
//! BLAKE3 digest against the manifest-declared value. Normal `camel run`
//! never populates it, so nothing leaks ambiently: `lookup` only sees
//! bytes when an artifact runtime put them there.
//!
//! Matching is positional by contract: manifest `embedded_files` entries
//! use the same canonical order as the store index, so population pairs
//! the two lists by position and checks kind, length, and digest per pair.
//! Secret manifest entries carry a null path — the registry never sees a
//! manifest path at all, so secrets are handled by never path-matching.
//! "Duplicate" means a duplicate logical path in the store list, never a
//! duplicate digest: two distinct assets with identical bytes are
//! legitimate and both populate.
//!
//! The registry is read-only after population: there is no mutation or
//! reset API. Because the singleton is process-global, test isolation is
//! fixed by DECIDED plan-flag choice — the one populating test lives alone
//! in `tests/asset_registry_population_test.rs` (its own `cargo test`
//! binary, its own process), emptiness assertions live in binaries that
//! never populate, and global state is never reset.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

/// Store-side asset entry offered to [`AssetRegistry::populate`], in the
/// canonical store-index order.
#[derive(Debug, Clone)]
pub struct RegistryAsset {
    /// Store logical path of the asset (`assets/<normalized-relative-path>`).
    /// Secret entries keep their path here: only the manifest redacts it.
    pub logical_path: String,
    /// Canonical store-entry kind name (`StoreEntryKind::as_str`).
    pub kind: String,
    /// Embedded asset bytes, embedded verbatim by the compiler.
    pub bytes: Vec<u8>,
}

/// Manifest-side positional counterpart of one [`RegistryAsset`].
///
/// Deliberately path-free: manifest entries for secret-class assets
/// withhold the logical path, so population pairs by position only.
#[derive(Debug, Clone)]
pub struct RegistryManifestEntry {
    /// Canonical kind name declared for the entry.
    pub kind: String,
    /// Declared byte length of the embedded content.
    pub length: u64,
    /// Declared BLAKE3 digest, lowercase hex (64 characters).
    pub digest: String,
}

/// Population failure. Every variant fails closed: nothing is stored.
#[derive(Debug)]
pub enum AssetRegistryError {
    /// The registry is populated exactly once per process.
    AlreadyPopulated,
    /// A store entry has no positional manifest counterpart.
    MissingManifestCounterpart {
        /// Store position lacking a manifest pair.
        position: usize,
        /// Logical path of the unpaired store entry.
        logical_path: String,
    },
    /// A manifest entry has no positional store counterpart.
    UnpairedManifestEntry {
        /// Manifest position lacking a store pair.
        position: usize,
    },
    /// Kind mismatch between the paired entries.
    KindMismatch {
        /// Paired position.
        position: usize,
        /// Logical path of the store entry.
        logical_path: String,
        /// Kind from the store index.
        store_kind: String,
        /// Kind from the manifest.
        manifest_kind: String,
    },
    /// Declared manifest length differs from the embedded byte length.
    LengthMismatch {
        /// Paired position.
        position: usize,
        /// Logical path of the store entry.
        logical_path: String,
        /// Length declared by the manifest.
        declared: u64,
        /// Actual embedded byte length.
        actual: usize,
    },
    /// BLAKE3 of the embedded bytes differs from the manifest digest.
    DigestMismatch {
        /// Paired position.
        position: usize,
        /// Logical path of the store entry.
        logical_path: String,
        /// Digest declared by the manifest.
        declared: String,
        /// Digest computed over the embedded bytes.
        actual: String,
    },
    /// The same logical path appears twice in the store list. A duplicate
    /// digest is NOT this error: identical bytes under distinct paths are
    /// legitimate.
    DuplicateLogicalPath {
        /// Repeated logical path.
        logical_path: String,
        /// Position of the duplicate occurrence.
        duplicate_position: usize,
    },
}

impl fmt::Display for AssetRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyPopulated => write!(
                f,
                "asset registry already populated; population happens exactly once per process"
            ),
            Self::MissingManifestCounterpart {
                position,
                logical_path,
            } => write!(
                f,
                "asset registry: store entry {position} ({logical_path:?}) has no positional manifest counterpart"
            ),
            Self::UnpairedManifestEntry { position } => write!(
                f,
                "asset registry: manifest entry {position} has no positional store counterpart"
            ),
            Self::KindMismatch {
                position,
                logical_path,
                store_kind,
                manifest_kind,
            } => write!(
                f,
                "asset registry: store entry {position} ({logical_path:?}) kind {store_kind:?} does not match manifest kind {manifest_kind:?}"
            ),
            Self::LengthMismatch {
                position: _,
                logical_path,
                declared,
                actual,
            } => write!(
                f,
                "asset registry: asset {logical_path:?} declared length {declared} does not match embedded bytes ({actual})"
            ),
            Self::DigestMismatch {
                position: _,
                logical_path,
                declared,
                actual,
            } => write!(
                f,
                "asset registry: asset {logical_path:?} BLAKE3 digest mismatch (declared {declared}, computed {actual})"
            ),
            Self::DuplicateLogicalPath {
                logical_path,
                duplicate_position,
            } => write!(
                f,
                "asset registry: duplicate asset logical path {logical_path:?} at store position {duplicate_position}"
            ),
        }
    }
}

impl std::error::Error for AssetRegistryError {}

/// Process-global, read-only map of `assets/<logical-path>` to bytes.
///
/// Accessed exclusively through [`AssetRegistry::global()`] (the
/// `TlsReloadRegistry::global()` singleton pattern). Population is
/// all-or-nothing: a verification failure leaves the registry unpopulated,
/// and a successful population is immutable for the rest of the process.
pub struct AssetRegistry {
    assets: OnceLock<HashMap<String, Vec<u8>>>,
}

impl AssetRegistry {
    /// Returns the process-global singleton.
    pub fn global() -> &'static AssetRegistry {
        static INSTANCE: OnceLock<AssetRegistry> = OnceLock::new();
        INSTANCE.get_or_init(|| AssetRegistry {
            assets: OnceLock::new(),
        })
    }

    /// Populates the registry from positionally paired store and manifest
    /// entries, verifying kind, length, and BLAKE3 digest for every pair.
    /// Takes the store entries by value so their embedded bytes move into
    /// the registry without copying.
    ///
    /// All-or-nothing: on any [`AssetRegistryError`] nothing is stored and
    /// the registry stays unpopulated. A second call — even with identical
    /// input — is rejected with [`AssetRegistryError::AlreadyPopulated`].
    /// Population is an explicit call: only the compiled-artifact runtime
    /// invokes it; normal `camel run` never does.
    pub fn populate(
        &self,
        store: Vec<RegistryAsset>,
        manifest: &[RegistryManifestEntry],
    ) -> Result<(), AssetRegistryError> {
        if self.assets.get().is_some() {
            return Err(AssetRegistryError::AlreadyPopulated);
        }
        let map = Self::verify(store, manifest)?;
        match self.assets.set(map) {
            Ok(()) => Ok(()),
            // Lost a concurrent population race; the winner populated the
            // singleton, so this call is still exactly the once-only rule.
            Err(_) => Err(AssetRegistryError::AlreadyPopulated),
        }
    }

    /// Verifies every positional pair and builds the read-only map. Static
    /// so a failed verification cannot half-touch the singleton: the map is
    /// built locally and only handed to the [`OnceLock`] after it is whole.
    fn verify(
        store: Vec<RegistryAsset>,
        manifest: &[RegistryManifestEntry],
    ) -> Result<HashMap<String, Vec<u8>>, AssetRegistryError> {
        let store_len = store.len();
        let mut assets = HashMap::with_capacity(store_len);
        for (position, entry) in store.into_iter().enumerate() {
            let Some(declared) = manifest.get(position) else {
                return Err(AssetRegistryError::MissingManifestCounterpart {
                    position,
                    logical_path: entry.logical_path,
                });
            };
            if entry.kind != declared.kind {
                return Err(AssetRegistryError::KindMismatch {
                    position,
                    logical_path: entry.logical_path,
                    store_kind: entry.kind,
                    manifest_kind: declared.kind.clone(),
                });
            }
            if declared.length as usize != entry.bytes.len() {
                return Err(AssetRegistryError::LengthMismatch {
                    position,
                    logical_path: entry.logical_path,
                    declared: declared.length,
                    actual: entry.bytes.len(),
                });
            }
            let computed = blake3::hash(&entry.bytes).to_hex();
            if computed.as_str() != declared.digest {
                return Err(AssetRegistryError::DigestMismatch {
                    position,
                    logical_path: entry.logical_path,
                    declared: declared.digest.clone(),
                    actual: computed.to_string(),
                });
            }
            // `insert` returning the previous value IS the duplicate check:
            // it pays for the map build already required, so no second map
            // or second pass is needed. The bytes move in, never cloned.
            if assets
                .insert(entry.logical_path.clone(), entry.bytes)
                .is_some()
            {
                return Err(AssetRegistryError::DuplicateLogicalPath {
                    logical_path: entry.logical_path,
                    duplicate_position: position,
                });
            }
        }
        if manifest.len() > store_len {
            return Err(AssetRegistryError::UnpairedManifestEntry {
                position: store_len,
            });
        }
        Ok(assets)
    }

    /// Whether an artifact runtime populated the registry in this process.
    pub fn is_populated(&self) -> bool {
        self.assets.get().is_some()
    }

    /// Looks an asset up by its store logical path
    /// (`assets/<normalized-relative-path>`).
    ///
    /// Returns a borrowed view of the embedded bytes; never a copy. Always
    /// `None` in processes that never populated the registry.
    pub fn lookup(&self, logical_path: &str) -> Option<&[u8]> {
        self.assets
            .get()
            .and_then(|assets| assets.get(logical_path))
            .map(|bytes| bytes.as_slice())
    }
}

#[cfg(test)]
#[test]
fn asset_registry_stays_empty_outside_artifacts() {
    // Placement note: this test lives at module top level (no inner
    // `mod tests`) so the task 3.1 filter
    // `asset_registry::asset_registry_stays_empty_outside_artifacts`
    // matches the full test path. The `--lib` binary never populates the
    // registry; the populating test is confined to
    // `tests/asset_registry_population_test.rs`, its own process.
    let registry = AssetRegistry::global();
    assert!(!registry.is_populated());
    assert_eq!(registry.lookup("assets/anything/at/all"), None);
}
