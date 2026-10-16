//! Confined virtual-root XSD schema-set discovery.
//!
//! Discovers the transitive `xs:import`/`xs:include` closure reachable from one
//! root schema file and packages it as a [`SchemaSet`]: root-relative virtual
//! lookup keys to schema bytes, plus the supplied absolute-URI alias map. The
//! Java resolver serves supplied keys only, so every distinct reachable virtual
//! key is transmitted even when two keys share one canonical file.
//!
//! Security posture (ADR-0032): schema locations and alias values are
//! untrusted input. Reads are confined to the canonicalized schema root, every
//! candidate is canonicalized and containment-checked before any read,
//! non-regular files are refused (no pipe hangs), no network access exists, no
//! external DTD/entity is loaded, and the 256-key / 8 MiB / 128-alias /
//! 32-level caps fail closed as keys are discovered.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use quick_xml::NsReader;
use quick_xml::XmlVersion;
use quick_xml::events::BytesStart;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use sha2::{Digest, Sha256};

use crate::error::ValidatorError;

/// Maximum number of transmitted [`SchemaSet::resources`] entries.
pub const MAX_SCHEMA_RESOURCES: usize = 256;

/// Maximum sum of transmitted byte values across all `resources` entries.
///
/// This is the schema-set total budget: an alias and a relative path that
/// denote the same file are two entries and both byte values count, so the
/// same bytes can legitimately consume the budget twice.
pub const MAX_SCHEMA_BYTES: usize = 8 * 1024 * 1024;

/// Maximum number of supplied absolute-URI alias mappings.
pub const MAX_SCHEMA_ALIASES: usize = 128;

/// Maximum number of newly expanded discovery states on one traversal path.
pub const MAX_IMPORT_DEPTH: usize = 32;

/// The XML Schema namespace. Only `import`/`include` elements in this
/// namespace are reference edges; any other namespace binding is ignored.
const XML_SCHEMA_NS: &str = "http://www.w3.org/2001/XMLSchema";

/// A discovered schema set, keyed for transmission to the XML bridge.
///
/// Fields are public and the struct is exhaustive: `camel-component-validator`
/// is not a contract crate (ADR-0049), matching `ValidatorConfig` and
/// `ValidatorError`. A constructed set that omits the root key is a caller
/// error that [`SchemaSet::root_bytes`] handles without panicking.
#[derive(Debug, Clone)]
pub struct SchemaSet {
    /// Root file's normalized relative basename; the entry schema's systemId.
    pub root_system_id: String,
    /// Transmitted keys to schema bytes, sorted for determinism: every distinct
    /// root-relative virtual lookup key, plus a redundant entry keyed by each
    /// supplied alias URI carrying the same bytes (wire compatibility). Two
    /// keys MAY share one canonical file (for example a symlink loop back to
    /// the root), and each key counts toward the caps.
    pub resources: BTreeMap<String, Vec<u8>>,
    /// Supplied absolute-URI alias -> normalized root-relative virtual path.
    /// Transmitted as the authoritative aliased-schema virtual base (the
    /// redundant `resources` alias-URI entry supplies bytes only); retained so
    /// replay re-supplies the exact map and the cache key covers it.
    pub aliases: BTreeMap<String, String>,
}

impl SchemaSet {
    /// Stable cache key: `xsd-<64 hex>`.
    ///
    /// Covers the root systemId, the sorted resource keys, each entry's bytes
    /// digest, and the sorted alias map. Any changed child or alias rotates it.
    ///
    /// The digest is length-prefixed and domain-separated, and it hashes a
    /// per-entry digest rather than raw concatenated bytes, so no two distinct
    /// sets can collide by concatenation ambiguity.
    pub fn cache_key(&self) -> String {
        fn put(hasher: &mut Sha256, bytes: &[u8]) {
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
        let mut hasher = Sha256::new();
        hasher.update(b"rust-camel/schema-set/v1\0");
        put(&mut hasher, self.root_system_id.as_bytes());
        hasher.update((self.resources.len() as u64).to_be_bytes());
        for (key, bytes) in &self.resources {
            put(&mut hasher, key.as_bytes());
            let mut entry = Sha256::new();
            entry.update(bytes);
            hasher.update(entry.finalize());
        }
        hasher.update((self.aliases.len() as u64).to_be_bytes());
        for (key, value) in &self.aliases {
            put(&mut hasher, key.as_bytes());
            put(&mut hasher, value.as_bytes());
        }
        format!("xsd-{}", hex::encode(hasher.finalize()))
    }

    /// Bytes of the root schema.
    ///
    /// Returns an empty slice rather than panicking when an externally
    /// constructed set does not carry the root key; discovery always populates
    /// it.
    pub fn root_bytes(&self) -> &[u8] {
        self.resources
            .get(&self.root_system_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// Discovers the schema set reachable from `root` under the supplied aliases.
///
/// `root` is the root schema file; the virtual root is that file's parent
/// directory. `aliases` maps absolute URIs to root-relative virtual paths;
/// every supplied alias is materialized as a redundant URI-keyed resource
/// entry and its target participates in reachable discovery even when the root
/// never references it.
pub fn discover_schema_set(
    root: &Path,
    aliases: &BTreeMap<String, String>,
) -> Result<SchemaSet, ValidatorError> {
    discover_schema_set_with(root, aliases, &mut read_file_bounded)
}

/// Reader-injection seam for discovery.
///
/// The injected reader performs the actual filesystem read. Discovery caches
/// the result per canonical target, so a canonical file is read exactly once
/// no matter how many virtual keys reach it. Production passes
/// [`read_file_bounded`]; tests inject a counting reader.
fn discover_schema_set_with(
    root: &Path,
    aliases: &BTreeMap<String, String>,
    read: &mut dyn FnMut(&Path) -> Result<Vec<u8>, ValidatorError>,
) -> Result<SchemaSet, ValidatorError> {
    if aliases.len() > MAX_SCHEMA_ALIASES {
        return Err(ValidatorError::endpoint(format!(
            "schema_aliases exceeds the {MAX_SCHEMA_ALIASES}-entry cap"
        )));
    }

    // Bare `root.xsd` has an empty parent; treat it as `.` without touching the
    // process CWD.
    let parent = match root.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let canonical_root = std::fs::canonicalize(parent).map_err(|err| {
        ValidatorError::endpoint(format!(
            "schema root directory '{}' cannot be canonicalized: {err}",
            parent.display()
        ))
    })?;
    let root_name = root.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        ValidatorError::endpoint("schema root path has no UTF-8 file name".to_string())
    })?;
    // The ORIGINAL basename is preserved even when the root is a symlink: the
    // root systemId is the operator-facing name, not the symlink target's name.
    let root_system_id = normalize_virtual(root_name)?;

    let mut normalized_aliases: BTreeMap<String, String> = BTreeMap::new();
    for (uri, value) in aliases {
        if !is_absolute_uri(uri) {
            return Err(ValidatorError::endpoint(format!(
                "schema_aliases key '{uri}' is not an absolute URI"
            )));
        }
        if is_absolute_uri(value) {
            return Err(ValidatorError::endpoint(format!(
                "schema_aliases value '{value}' for '{uri}' must be a root-relative virtual path"
            )));
        }
        let decoded = decode_uri_path(value)?;
        if is_absolute_uri(&decoded) {
            return Err(ValidatorError::endpoint(format!(
                "schema_aliases value '{value}' for '{uri}' percent-decodes to an absolute URI"
            )));
        }
        normalized_aliases.insert(uri.clone(), normalize_virtual(&decoded)?);
    }

    let mut discoverer = Discoverer {
        virtual_root: parent.to_path_buf(),
        canonical_root,
        aliases: normalized_aliases,
        cache: HashMap::new(),
        seen: HashSet::new(),
        resources: BTreeMap::new(),
        total_bytes: 0,
        queue: VecDeque::new(),
        read,
    };
    discoverer.run(&root_system_id)?;

    Ok(SchemaSet {
        root_system_id,
        resources: discoverer.resources,
        aliases: discoverer.aliases,
    })
}

/// One canonical file's bytes and its parsed `xs:import`/`xs:include` targets.
struct CachedFile {
    bytes: Vec<u8>,
    refs: Vec<String>,
}

/// BFS discovery state: iterative so the resource cap is reached before the
/// depth cap on branching symlink growth.
struct Discoverer<'a> {
    virtual_root: PathBuf,
    canonical_root: PathBuf,
    aliases: BTreeMap<String, String>,
    /// Read-once cache keyed by canonical target.
    cache: HashMap<PathBuf, CachedFile>,
    /// Expansion states already offered, keyed by `(canonical target,
    /// normalized virtual path)` so two virtual bases over one canonical file
    /// each expand their own relative children.
    seen: HashSet<(PathBuf, String)>,
    resources: BTreeMap<String, Vec<u8>>,
    total_bytes: usize,
    queue: VecDeque<(PathBuf, String, usize)>,
    read: &'a mut dyn FnMut(&Path) -> Result<Vec<u8>, ValidatorError>,
}

impl Discoverer<'_> {
    fn run(&mut self, root_virtual: &str) -> Result<(), ValidatorError> {
        self.offer(root_virtual, 0)?;

        // Alias seeds: the relative target and the redundant URI-keyed entry
        // both count. Cap checks run before any clone of the aliased bytes.
        for (uri, norm) in self.aliases.clone() {
            self.offer(&norm, 0)?;
            let canonical =
                canonicalize_candidate(&self.virtual_root, &self.canonical_root, &norm)?;
            let cached = self.cache.get(&canonical).ok_or_else(|| {
                ValidatorError::endpoint(format!(
                    "schema '{norm}' was not loaded into the discovery cache"
                ))
            })?;
            checked_insert(
                &mut self.resources,
                &mut self.total_bytes,
                &uri,
                &cached.bytes,
            )?;
        }

        while let Some((canonical, vpath, depth)) = self.queue.pop_front() {
            let refs = self
                .cache
                .get(&canonical)
                .map(|c| c.refs.clone())
                .ok_or_else(|| {
                    ValidatorError::endpoint(format!(
                        "schema '{vpath}' was not loaded into the discovery cache"
                    ))
                })?;
            for location in refs {
                let child = resolve_ref(&vpath, &location, &self.aliases)?;
                self.offer(&child, depth + 1)?;
            }
        }
        Ok(())
    }

    /// Offers one expansion state: resolve, containment-check, read once,
    /// transmit under its virtual key, and enqueue for reference expansion.
    fn offer(&mut self, vpath: &str, depth: usize) -> Result<(), ValidatorError> {
        let canonical = canonicalize_candidate(&self.virtual_root, &self.canonical_root, vpath)?;
        // Skip a revisited pair before the depth check so cycles terminate
        // without a depth error while every distinct virtual key is kept.
        if !self.seen.insert((canonical.clone(), vpath.to_string())) {
            return Ok(());
        }
        if depth > MAX_IMPORT_DEPTH {
            return Err(ValidatorError::endpoint(format!(
                "schema import/include depth exceeds the {MAX_IMPORT_DEPTH}-level cap at '{vpath}'"
            )));
        }
        if !self.cache.contains_key(&canonical) {
            let bytes = (self.read)(&canonical)?;
            let refs = parse_schema_refs(&bytes)?;
            self.cache
                .insert(canonical.clone(), CachedFile { bytes, refs });
        }
        let cached = self.cache.get(&canonical).ok_or_else(|| {
            ValidatorError::endpoint(format!(
                "schema '{vpath}' was not loaded into the discovery cache"
            ))
        })?;
        checked_insert(
            &mut self.resources,
            &mut self.total_bytes,
            vpath,
            &cached.bytes,
        )?;
        self.queue.push_back((canonical, vpath.to_string(), depth));
        Ok(())
    }
}

/// Insert-or-conflict helper for the transmitted resource map.
///
/// The same normalized virtual key reached twice with identical bytes is a
/// no-op; different bytes for one key is a bounded conflict error. Resource
/// and byte caps are checked before the bytes are cloned into the map.
fn checked_insert(
    resources: &mut BTreeMap<String, Vec<u8>>,
    total_bytes: &mut usize,
    key: &str,
    bytes: &[u8],
) -> Result<(), ValidatorError> {
    if let Some(existing) = resources.get(key) {
        if existing.as_slice() != bytes {
            return Err(ValidatorError::endpoint(format!(
                "conflicting schema definitions for virtual path '{key}': \
                 the same normalized key was supplied with different bytes"
            )));
        }
        return Ok(());
    }
    if resources.len() + 1 > MAX_SCHEMA_RESOURCES {
        return Err(ValidatorError::endpoint(format!(
            "schema set exceeds the {MAX_SCHEMA_RESOURCES}-entry resource cap at '{key}'"
        )));
    }
    if total_bytes.saturating_add(bytes.len()) > MAX_SCHEMA_BYTES {
        return Err(ValidatorError::endpoint(format!(
            "schema set exceeds the 8 MiB ({} byte) total schema-set cap at '{key}'",
            MAX_SCHEMA_BYTES
        )));
    }
    *total_bytes += bytes.len();
    resources.insert(key.to_string(), bytes.to_vec());
    Ok(())
}

/// Bounded read of a regular file.
///
/// Non-regular files are refused before any read (a FIFO would otherwise
/// hang), and the read is bounded at the schema-set byte cap plus one so an
/// oversized file cannot force an unbounded allocation.
fn read_file_bounded(path: &Path) -> Result<Vec<u8>, ValidatorError> {
    let file = File::open(path).map_err(|err| {
        ValidatorError::endpoint(format!("cannot read schema '{}': {err}", path.display()))
    })?;
    let meta = file.metadata().map_err(|err| {
        ValidatorError::endpoint(format!("cannot stat schema '{}': {err}", path.display()))
    })?;
    if !meta.file_type().is_file() {
        return Err(ValidatorError::endpoint(format!(
            "schema '{}' is not a regular file and is refused",
            path.display()
        )));
    }
    let mut buf = Vec::new();
    let mut limited = file.take(MAX_SCHEMA_BYTES as u64 + 1);
    limited.read_to_end(&mut buf).map_err(|err| {
        ValidatorError::endpoint(format!("cannot read schema '{}': {err}", path.display()))
    })?;
    if buf.len() > MAX_SCHEMA_BYTES {
        return Err(ValidatorError::endpoint(format!(
            "schema '{}' exceeds the 8 MiB ({} byte) schema-set cap",
            path.display(),
            MAX_SCHEMA_BYTES
        )));
    }
    Ok(buf)
}

/// Resolves a `schemaLocation` against the importing schema's virtual base.
///
/// The absolute-URI branch matches the RAW location before any decoding, so a
/// supplied alias is an exact mapping and an encoded absolute URI is refused
/// with no fetch. Relative locations are percent-decoded once and then
/// confined: an absolute filesystem path is refused BEFORE it is joined onto
/// the importing base, because `/child.xsd` from `a/parent.xsd` would otherwise
/// collapse to the in-root `a/child.xsd`. Only a root-relative virtual path
/// survives to normalization.
fn resolve_ref(
    importer_virtual: &str,
    location: &str,
    aliases: &BTreeMap<String, String>,
) -> Result<String, ValidatorError> {
    if is_absolute_uri(location) {
        return aliases.get(location).cloned().ok_or_else(|| {
            ValidatorError::endpoint(format!(
                "schema location '{location}' is an absolute URI with no supplied alias; \
                 external resolution is refused"
            ))
        });
    }
    let decoded = decode_uri_path(location)?;
    if is_absolute_uri(&decoded) {
        return Err(ValidatorError::endpoint(format!(
            "schema location '{location}' percent-decodes to an absolute URI; \
             external resolution is refused"
        )));
    }
    // Refuse absolute filesystem paths before the base join. Covers the
    // leading `/` and any platform-absolute form (drive prefix, UNC).
    if decoded.starts_with('/') || Path::new(&decoded).is_absolute() {
        return Err(ValidatorError::endpoint(format!(
            "schema location '{location}' is an absolute filesystem path; \
             only root-relative virtual paths and supplied aliases are accepted"
        )));
    }
    let dir = match importer_virtual.rfind('/') {
        Some(idx) => &importer_virtual[..idx],
        None => "",
    };
    let joined = if dir.is_empty() {
        decoded
    } else {
        format!("{dir}/{decoded}")
    };
    normalize_virtual(&joined)
}

/// Normalizes a root-relative virtual path: drops `.`/empty segments, applies
/// `..`, and refuses anything that escapes the virtual root or names no file.
fn normalize_virtual(raw: &str) -> Result<String, ValidatorError> {
    if raw.is_empty() {
        return Err(ValidatorError::endpoint("schema path is empty".to_string()));
    }
    if raw.starts_with('/') {
        return Err(ValidatorError::endpoint(format!(
            "schema path '{raw}' is absolute; only root-relative virtual paths are accepted"
        )));
    }
    let mut out: Vec<&str> = Vec::new();
    for segment in raw.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if out.pop().is_none() {
                    return Err(ValidatorError::endpoint(format!(
                        "schema path '{raw}' escapes the schema root and is refused"
                    )));
                }
            }
            other => out.push(other),
        }
    }
    if out.is_empty() {
        return Err(ValidatorError::endpoint(format!(
            "schema path '{raw}' does not name a file"
        )));
    }
    Ok(out.join("/"))
}

/// Percent-decodes a URI reference exactly once, following the workspace
/// `percent-encoding` convention used by `ValidatorConfig::from_uri`.
fn decode_uri_path(raw: &str) -> Result<String, ValidatorError> {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map(|s| s.into_owned())
        .map_err(|err| {
            ValidatorError::endpoint(format!(
                "schema location '{raw}' is not valid UTF-8 after percent-decoding: {err}"
            ))
        })
}

/// RFC 3986 absolute-URI test: `scheme ":" ...` with a well-formed scheme.
///
/// Used to fail closed on any URI-like location that has no supplied alias.
fn is_absolute_uri(s: &str) -> bool {
    let Some(colon) = s.find(':') else {
        return false;
    };
    if colon == 0 {
        return false;
    }
    let mut chars = s[..colon].chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
}

/// Canonicalizes a virtual path and confines it to the schema root.
///
/// Symlinks are followed (a config-map root is supported) but the canonical
/// target must stay inside `canonical_root` and must be a regular file.
fn canonicalize_candidate(
    virtual_root: &Path,
    canonical_root: &Path,
    virtual_path: &str,
) -> Result<PathBuf, ValidatorError> {
    let joined = virtual_root.join(virtual_path);
    let canonical = std::fs::canonicalize(&joined).map_err(|err| {
        ValidatorError::endpoint(format!(
            "schema '{virtual_path}' cannot be resolved within the schema root: {err}"
        ))
    })?;
    if !canonical.starts_with(canonical_root) {
        return Err(ValidatorError::endpoint(format!(
            "schema '{virtual_path}' resolves outside the schema root and is refused"
        )));
    }
    let meta = std::fs::metadata(&canonical).map_err(|err| {
        ValidatorError::endpoint(format!("cannot stat schema '{virtual_path}': {err}"))
    })?;
    if !meta.file_type().is_file() {
        return Err(ValidatorError::endpoint(format!(
            "schema '{virtual_path}' is not a regular file and is refused"
        )));
    }
    Ok(canonical)
}

/// Parses the `xs:import`/`xs:include` `schemaLocation` edges of one schema.
///
/// Namespace-aware (`quick_xml::NsReader`): only elements resolved into the
/// XML Schema namespace count, regardless of prefix or default-namespace form.
/// No external DTD or entity is loaded and nothing is fetched.
fn parse_schema_refs(bytes: &[u8]) -> Result<Vec<String>, ValidatorError> {
    let mut reader = NsReader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut refs = Vec::new();
    loop {
        match reader.read_resolved_event_into(&mut buf) {
            Ok((resolve, Event::Start(start))) => collect_ref(&resolve, &start, &mut refs)?,
            Ok((resolve, Event::Empty(start))) => collect_ref(&resolve, &start, &mut refs)?,
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(err) => {
                return Err(ValidatorError::endpoint(format!(
                    "schema document is not well-formed XML: {err}"
                )));
            }
        }
    }
    Ok(refs)
}

fn collect_ref(
    resolve: &ResolveResult<'_>,
    start: &BytesStart<'_>,
    refs: &mut Vec<String>,
) -> Result<(), ValidatorError> {
    let namespace = match resolve {
        ResolveResult::Bound(ns) => ns.0,
        _ => return Ok(()),
    };
    if namespace != XML_SCHEMA_NS {
        return Ok(());
    }
    let local_name = start.local_name();
    let local: &str = local_name.as_ref();
    if local != "import" && local != "include" {
        return Ok(());
    }
    for attr in start.attributes() {
        let attr = attr.map_err(|err| {
            ValidatorError::endpoint(format!("schema document has an invalid attribute: {err}"))
        })?;
        // XML attributes are unqualified unless prefixed: the default
        // namespace does NOT apply to attribute names. Match the raw QName so
        // only a literal unprefixed `schemaLocation` is a reference edge and a
        // foreign `meta:schemaLocation` is metadata, never a filesystem or
        // network edge.
        if attr.key.as_ref() != "schemaLocation" {
            continue;
        }
        // XML attribute-value normalization: decodes character references and
        // predefined entities only. No external DTD or entity is ever loaded.
        let value = attr
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|err| {
                ValidatorError::endpoint(format!(
                    "schemaLocation attribute is not valid XML text: {err}"
                ))
            })?;
        refs.push(value.into_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashMap};
    use std::path::{Path, PathBuf};

    use super::{
        MAX_IMPORT_DEPTH, MAX_SCHEMA_BYTES, MAX_SCHEMA_RESOURCES, SchemaSet, checked_insert,
        discover_schema_set, discover_schema_set_with, read_file_bounded,
    };

    const XS: &str = "http://www.w3.org/2001/XMLSchema";

    // ── fixtures ────────────────────────────────────────────────────────

    /// Builds an XSD document whose elements use `prefix` (empty string selects
    /// the default namespace) and which carries one `kind` reference per
    /// `refs` entry. Namespace-aware parsing must see every one of these forms
    /// as an XML Schema `import`/`include`, never as a text match.
    fn xsd_with_refs(prefix: &str, refs: &[(&str, &str)]) -> String {
        let xmlns = if prefix.is_empty() {
            format!("xmlns=\"{XS}\"")
        } else {
            format!("xmlns:{prefix}=\"{XS}\"")
        };
        let p = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}:")
        };
        let mut body = String::new();
        for (kind, loc) in refs {
            body.push_str(&format!("  <{p}{kind} schemaLocation=\"{loc}\"/>\n"));
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<{p}schema {xmlns}>\n{body}</{p}schema>\n"
        )
    }

    /// Empty schema: a schema with no global component, so including it twice
    /// cannot produce a duplicate declaration.
    fn xsd_empty() -> String {
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xs:schema xmlns:xs=\"{XS}\"/>\n")
    }

    /// Root schema declaring exactly one global element plus `xs:`-prefixed
    /// `refs`.
    fn xsd_root_with_element(refs: &[(&str, &str)]) -> String {
        let mut body = String::from("  <xs:element name=\"root\" type=\"xs:string\"/>\n");
        for (kind, loc) in refs {
            body.push_str(&format!("  <xs:{kind} schemaLocation=\"{loc}\"/>\n"));
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xs:schema xmlns:xs=\"{XS}\">\n{body}</xs:schema>\n"
        )
    }

    fn write_file(dir: &Path, rel: &str, contents: &str) -> PathBuf {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn symlink(target: &Path, link: &Path) {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    /// Extends `path` to `len` bytes without materializing content.
    fn set_len(path: &Path, len: u64) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(len)
            .unwrap();
    }

    fn empty_aliases() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    // ── reachable imports ───────────────────────────────────────────────

    #[test]
    fn reachable_imports_are_included() {
        // Namespace-form table: the same import graph is discovered whether the
        // XML Schema namespace is bound to `xsd:`, to the default namespace, or
        // to an arbitrary prefix. The `custom` row also pins `.`/`..`
        // normalization inside the virtual root.
        let cases: [(&str, &str, &str); 3] = [
            ("xsd", "xsd", "deep/grandchild.xsd"),
            ("default", "", "deep/grandchild.xsd"),
            ("custom", "s", "./deep/../deep/grandchild.xsd"),
        ];
        for (label, prefix, grand_loc) in cases {
            let tmp = tempfile::tempdir().unwrap();
            let root_path = write_file(
                tmp.path(),
                "root.xsd",
                &xsd_with_refs(prefix, &[("import", "child.xsd")]),
            );
            write_file(
                tmp.path(),
                "child.xsd",
                &xsd_with_refs(prefix, &[("include", grand_loc)]),
            );
            write_file(tmp.path(), "deep/grandchild.xsd", &xsd_empty());

            let set = discover_schema_set(&root_path, &empty_aliases())
                .unwrap_or_else(|e| panic!("{label}: discovery failed: {e}"));

            for key in ["root.xsd", "child.xsd", "deep/grandchild.xsd"] {
                assert!(
                    set.resources.contains_key(key),
                    "{label}: missing normalized entry '{key}' in {:?}",
                    set.resources.keys().collect::<Vec<_>>()
                );
            }
            assert_eq!(set.resources.len(), 3, "{label}: exactly three entries");
            assert_eq!(set.root_system_id, "root.xsd", "{label}: root systemId");
        }

        // Foreign-attribute row: XML attributes are unqualified unless
        // prefixed, so the default namespace never qualifies them and only the
        // raw QName `schemaLocation` is a reference edge. `meta:schemaLocation`
        // and `alt:schemaLocation` are metadata from other namespaces and MUST
        // not create a filesystem edge (`decoy.xsd` exists on disk) or a
        // network edge (`https://…` has no supplied alias).
        {
            let tmp = tempfile::tempdir().unwrap();
            let root_xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <xs:schema xmlns:xs=\"{XS}\" xmlns:meta=\"urn:metadata\" xmlns:alt=\"urn:alt\">\n\
                 \x20 <xs:include schemaLocation=\"child.xsd\" meta:schemaLocation=\"decoy.xsd\"/>\n\
                 \x20 <xs:include meta:schemaLocation=\"https://example.invalid/x.xsd\"/>\n\
                 </xs:schema>\n"
            );
            let root_path = write_file(tmp.path(), "root.xsd", &root_xml);
            write_file(tmp.path(), "child.xsd", &xsd_empty());
            write_file(tmp.path(), "decoy.xsd", &xsd_empty());

            let set = discover_schema_set(&root_path, &empty_aliases())
                .expect("foreign-namespace schemaLocation attributes are not reference edges");

            for key in ["root.xsd", "child.xsd"] {
                assert!(
                    set.resources.contains_key(key),
                    "foreign-attrs: missing actual entry '{key}' in {:?}",
                    set.resources.keys().collect::<Vec<_>>()
                );
            }
            assert!(
                !set.resources.contains_key("decoy.xsd"),
                "foreign `meta:schemaLocation` must not create a filesystem edge: {:?}",
                set.resources.keys().collect::<Vec<_>>()
            );
            assert_eq!(
                set.resources.len(),
                2,
                "only the real child is discovered: {:?}",
                set.resources.keys().collect::<Vec<_>>()
            );
        }
    }

    // Native Windows filesystems do not support literal '?' filename data.
    #[cfg(unix)]
    #[test]
    fn encoded_fragment_filename_is_discovered_once() {
        assert_encoded_filename_discovered_once("child%23v1.xsd", "child#v1.xsd");
    }

    #[cfg(unix)]
    #[test]
    fn encoded_query_filename_is_discovered_once() {
        assert_encoded_filename_discovered_once("child%3Fv1.xsd", "child?v1.xsd");
    }

    /// Read-only override selects the fixture filesystem; Unix defaults to `/tmp` rather than
    /// a potentially NTFS-backed TMPDIR. The selected directory must be writable and support
    /// literal '#' and '?' filenames. Set CAMEL_TEST_SCHEMA_FIXTURE_DIR if `/tmp` does not.
    #[cfg(unix)]
    fn assert_encoded_filename_discovered_once(reference: &str, child_name: &str) {
        let fixture_dir = std::env::var_os("CAMEL_TEST_SCHEMA_FIXTURE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let setup_error = |error: std::io::Error| {
            format!(
                "CAMEL_TEST_SCHEMA_FIXTURE_DIR must name a writable directory on a filesystem \
                 supporting literal '#' and '?' filenames (selected '{}'): {error}",
                fixture_dir.display()
            )
        };
        let tmp = tempfile::tempdir_in(&fixture_dir)
            .unwrap_or_else(|error| panic!("{}", setup_error(error)));
        std::fs::create_dir(tmp.path().join("nested"))
            .unwrap_or_else(|error| panic!("{}", setup_error(error)));
        let child_key = format!("nested/{child_name}");
        for (key, contents) in [
            (
                "root.xsd",
                xsd_with_refs("xs", &[("include", &format!("nested/{reference}"))]),
            ),
            (
                &child_key,
                xsd_with_refs("xs", &[("include", "grandchild.xsd")]),
            ),
            ("nested/grandchild.xsd", xsd_empty()),
            ("grandchild.xsd", xsd_empty()),
        ] {
            std::fs::write(tmp.path().join(key), contents)
                .unwrap_or_else(|error| panic!("{} (fixture '{key}')", setup_error(error)));
        }
        let root = tmp.path().join("root.xsd");

        let mut reads = HashMap::<PathBuf, usize>::new();
        let set = discover_schema_set_with(&root, &empty_aliases(), &mut |path| {
            *reads.entry(path.to_path_buf()).or_default() += 1;
            read_file_bounded(path)
        })
        .expect("encoded filename and its relative descendant must be discovered");

        assert_eq!(set.root_system_id, "root.xsd");
        assert_eq!(set.resources.len(), 3);
        for key in ["root.xsd", &child_key, "nested/grandchild.xsd"] {
            assert!(set.resources.contains_key(key), "missing literal key {key}");
            let canonical = std::fs::canonicalize(tmp.path().join(key))
                .unwrap_or_else(|error| panic!("{} (fixture '{key}')", setup_error(error)));
            assert_eq!(reads.get(&canonical), Some(&1), "read once: {key}");
        }
        assert_eq!(reads.len(), 3, "only the expected closure is read");
    }

    #[test]
    fn unrelated_files_are_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let root_path = write_file(
            tmp.path(),
            "root.xsd",
            &xsd_with_refs("xs", &[("import", "child.xsd")]),
        );
        write_file(tmp.path(), "child.xsd", &xsd_empty());
        write_file(tmp.path(), "unrelated.xsd", &xsd_empty());

        let set = discover_schema_set(&root_path, &empty_aliases()).unwrap();

        assert!(
            !set.resources.contains_key("unrelated.xsd"),
            "unreferenced sibling must not be discovered: {:?}",
            set.resources.keys().collect::<Vec<_>>()
        );
        assert_eq!(set.resources.len(), 2);
    }

    #[test]
    fn diamond_included_once_without_duplicate_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root_path = write_file(
            tmp.path(),
            "a.xsd",
            &xsd_with_refs("xs", &[("import", "b.xsd"), ("include", "c.xsd")]),
        );
        write_file(
            tmp.path(),
            "b.xsd",
            &xsd_with_refs("xs", &[("include", "d.xsd")]),
        );
        write_file(
            tmp.path(),
            "c.xsd",
            &xsd_with_refs("xs", &[("include", "d.xsd")]),
        );
        write_file(tmp.path(), "d.xsd", &xsd_empty());

        let set = discover_schema_set(&root_path, &empty_aliases())
            .expect("diamond must not raise a duplicate error");

        assert!(set.resources.contains_key("d.xsd"));
        assert_eq!(
            set.resources.len(),
            4,
            "D is transmitted exactly once: {:?}",
            set.resources.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn cyclic_include_terminates() {
        let tmp = tempfile::tempdir().unwrap();
        let root_path = write_file(
            tmp.path(),
            "a.xsd",
            &xsd_with_refs("xs", &[("include", "b.xsd")]),
        );
        write_file(
            tmp.path(),
            "b.xsd",
            &xsd_with_refs("xs", &[("include", "a.xsd")]),
        );

        let set = discover_schema_set(&root_path, &empty_aliases())
            .expect("include cycle must terminate without a depth error");

        assert!(set.resources.contains_key("a.xsd"));
        assert!(set.resources.contains_key("b.xsd"));
    }

    // ── per-virtual-base expansion ──────────────────────────────────────

    #[test]
    fn symlinked_cycle_preserves_loop_key() {
        // Root declares exactly one global element and `xs:include`s an EMPTY
        // `loop.xsd`; `loop.xsd` `xs:include`s `loop-link.xsd`, a symlink back
        // to the empty `loop.xsd` (NOT to the root, so the root's global
        // element is not duplicated). `xs:include`, not a same-namespace
        // `xs:import`, which is invalid for a same-namespace self target.
        let tmp = tempfile::tempdir().unwrap();
        let root_path = write_file(
            tmp.path(),
            "root.xsd",
            &xsd_root_with_element(&[("include", "loop.xsd")]),
        );
        write_file(
            tmp.path(),
            "loop.xsd",
            &xsd_with_refs("xs", &[("include", "loop-link.xsd")]),
        );
        symlink(Path::new("loop.xsd"), &tmp.path().join("loop-link.xsd"));

        let reads = RefCell::new(HashMap::<PathBuf, usize>::new());
        let mut read = |p: &Path| {
            *reads.borrow_mut().entry(p.to_path_buf()).or_insert(0) += 1;
            read_file_bounded(p)
        };
        let set = discover_schema_set_with(&root_path, &empty_aliases(), &mut read)
            .expect("symlinked include cycle must terminate with no depth error");

        // Both loop virtual keys are transmitted: the supplied-keys-only
        // resolver must be able to serve `loop-link.xsd` as well as `loop.xsd`.
        for key in ["root.xsd", "loop.xsd", "loop-link.xsd"] {
            assert!(
                set.resources.contains_key(key),
                "missing virtual key '{key}' in {:?}",
                set.resources.keys().collect::<Vec<_>>()
            );
        }
        assert_eq!(
            set.resources.len(),
            3,
            "{:?}",
            set.resources.keys().collect::<Vec<_>>()
        );

        let loop_canonical = std::fs::canonicalize(tmp.path().join("loop.xsd")).unwrap();
        assert_eq!(
            reads.borrow().get(&loop_canonical),
            Some(&1),
            "the canonical empty file's bytes must be read exactly once"
        );
    }

    #[test]
    fn per_virtual_base_expands_distinct_children() {
        let tmp = tempfile::tempdir().unwrap();
        let root_path = write_file(
            tmp.path(),
            "root.xsd",
            &xsd_with_refs(
                "xs",
                &[("include", "a/shared.xsd"), ("include", "b/shared.xsd")],
            ),
        );
        // One canonical shared file, reached through two virtual keys, each of
        // which expands `child.xsd` against its own virtual base.
        write_file(
            tmp.path(),
            "shared.xsd",
            &xsd_with_refs("xs", &[("include", "child.xsd")]),
        );
        symlink(Path::new("../shared.xsd"), &tmp.path().join("a/shared.xsd"));
        symlink(Path::new("../shared.xsd"), &tmp.path().join("b/shared.xsd"));
        write_file(tmp.path(), "a/child.xsd", &xsd_empty());
        write_file(tmp.path(), "b/child.xsd", &xsd_empty());

        let reads = RefCell::new(HashMap::<PathBuf, usize>::new());
        let mut read = |p: &Path| {
            *reads.borrow_mut().entry(p.to_path_buf()).or_insert(0) += 1;
            read_file_bounded(p)
        };
        let set = discover_schema_set_with(&root_path, &empty_aliases(), &mut read)
            .expect("per-virtual-base expansion must succeed");

        for key in ["a/shared.xsd", "b/shared.xsd", "a/child.xsd", "b/child.xsd"] {
            assert!(
                set.resources.contains_key(key),
                "missing virtual key '{key}' in {:?}",
                set.resources.keys().collect::<Vec<_>>()
            );
        }
        assert_eq!(
            set.resources.len(),
            5,
            "{:?}",
            set.resources.keys().collect::<Vec<_>>()
        );

        let shared_canonical = std::fs::canonicalize(tmp.path().join("shared.xsd")).unwrap();
        assert_eq!(
            reads.borrow().get(&shared_canonical),
            Some(&1),
            "the canonical shared file's bytes must be read exactly once"
        );
    }

    // ── caps ────────────────────────────────────────────────────────────

    #[test]
    fn unbounded_symlink_alias_growth_fails_closed() {
        // Branching symlink-directory fixture. The root `xs:include`s
        // `b0a/n1.xsd` and `b0b/n1.xsd` where `b0a`/`b0b` are symlink
        // directories to `D1`; each `D_i/n_i.xsd` includes `b_ia/n_{i+1}.xsd`
        // and `b_ib/n_{i+1}.xsd` where `b_ia`/`b_ib` are symlink directories to
        // `D_{i+1}`. Distinct virtual lookup keys therefore double per level
        // while each level shares one canonical file.
        //
        // Both caps are distinct constraints: this fixture branches, so BFS
        // reaches the 256-entry resource cap at level 8 (depth 8), well before
        // the 32-level depth cap could fire. A linear chain of the same length
        // would instead hit the depth cap — see `depth_limit_is_enforced`.
        const LEVELS: usize = 8;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_file(
            dir,
            "root.xsd",
            &xsd_with_refs(
                "xs",
                &[("include", "b0a/n1.xsd"), ("include", "b0b/n1.xsd")],
            ),
        );
        symlink(&dir.join("D1"), &dir.join("b0a"));
        symlink(&dir.join("D1"), &dir.join("b0b"));
        for i in 1..=LEVELS {
            write_file(dir, &format!("D{i}/n{i}.xsd"), &xsd_empty());
            if i < LEVELS {
                let next = format!("D{}", i + 1);
                symlink(&dir.join(&next), &dir.join(format!("D{i}/b{i}a")));
                symlink(&dir.join(&next), &dir.join(format!("D{i}/b{i}b")));
            }
        }
        for i in 1..LEVELS {
            let next = i + 1;
            write_file(
                dir,
                &format!("D{i}/n{i}.xsd"),
                &xsd_with_refs(
                    "xs",
                    &[
                        ("include", &format!("b{i}a/n{next}.xsd")),
                        ("include", &format!("b{i}b/n{next}.xsd")),
                    ],
                ),
            );
        }

        let err = discover_schema_set(&dir.join("root.xsd"), &empty_aliases())
            .expect_err("unbounded virtual-key growth must fail closed");
        let msg = err.to_string();
        assert!(
            msg.contains("256"),
            "error must name the 256-entry cap, got: {msg}"
        );
    }

    #[test]
    fn depth_limit_is_enforced() {
        // Table: the boundary is 32 expansion levels below the root; the
        // over-limit row is a chain one level deeper.
        for (hops, expect_ok) in [(MAX_IMPORT_DEPTH, true), (MAX_IMPORT_DEPTH + 1, false)] {
            let tmp = tempfile::tempdir().unwrap();
            let root_path = write_file(
                tmp.path(),
                "root.xsd",
                &xsd_with_refs("xs", &[("include", "c1.xsd")]),
            );
            // root -> c1 -> ... -> c{hops}; c{hops} is a leaf.
            for i in 1..hops {
                let loc = format!("c{}.xsd", i + 1);
                let refs = [("include", loc.as_str())];
                write_file(
                    tmp.path(),
                    &format!("c{i}.xsd"),
                    &xsd_with_refs("xs", &refs),
                );
            }
            write_file(tmp.path(), &format!("c{hops}.xsd"), &xsd_empty());

            let result = discover_schema_set(&root_path, &empty_aliases());
            if expect_ok {
                assert!(
                    result.is_ok(),
                    "chain of {hops} hops must stay within the depth cap: {:?}",
                    result.err().map(|e| e.to_string())
                );
            } else {
                let err = result.expect_err("chain past the depth cap must fail");
                let msg = err.to_string();
                assert!(
                    msg.contains("32"),
                    "error must name the 32-level depth cap, got: {msg}"
                );
            }
        }
    }

    #[test]
    fn more_than_256_entries_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let locations: Vec<String> = (0..MAX_SCHEMA_RESOURCES)
            .map(|i| format!("c{i:03}.xsd"))
            .collect();
        let refs: Vec<(&str, &str)> = locations
            .iter()
            .map(|loc| ("include", loc.as_str()))
            .collect();
        let root_path = write_file(tmp.path(), "root.xsd", &xsd_with_refs("xs", &refs));
        for loc in &locations {
            write_file(tmp.path(), loc, &xsd_empty());
        }

        let err = discover_schema_set(&root_path, &empty_aliases())
            .expect_err("root + 256 children exceeds the 256-entry cap");
        let msg = err.to_string();
        assert!(
            msg.contains("256"),
            "error must name the 256-entry cap, got: {msg}"
        );
    }

    #[test]
    fn more_than_8_mib_is_rejected() {
        // Table: a single huge file, and an alias that duplicates the byte
        // weight of one file into a second transmitted entry.
        for label in ["huge file", "alias duplicate byte weight"] {
            let tmp = tempfile::tempdir().unwrap();
            let (root_path, aliases) = if label == "huge file" {
                let root_path = write_file(
                    tmp.path(),
                    "root.xsd",
                    &xsd_with_refs("xs", &[("include", "huge.xsd")]),
                );
                let huge = write_file(tmp.path(), "huge.xsd", &xsd_empty());
                set_len(&huge, MAX_SCHEMA_BYTES as u64 + 1);
                (root_path, BTreeMap::new())
            } else {
                let root_path = write_file(
                    tmp.path(),
                    "root.xsd",
                    &xsd_with_refs("xs", &[("include", "big.xsd")]),
                );
                let big = write_file(tmp.path(), "big.xsd", &xsd_empty());
                set_len(&big, (MAX_SCHEMA_BYTES as u64 / 2) + 1024);
                let mut aliases = BTreeMap::new();
                aliases.insert(
                    "http://example.com/big.xsd".to_string(),
                    "big.xsd".to_string(),
                );
                (root_path, aliases)
            };

            let err = discover_schema_set(&root_path, &aliases)
                .expect_err(&format!("{label}: expected the 8 MiB cap error"));
            let msg = err.to_string();
            assert!(
                msg.contains("8 MiB"),
                "{label}: error must name the 8 MiB cap, got: {msg}"
            );
        }
    }

    // ── conflicts ───────────────────────────────────────────────────────

    #[test]
    fn conflicting_definitions_are_rejected() {
        // Unit test of the checked inserter: the map itself cannot hold two
        // definitions of one key, so the conflict surface is the helper.
        let mut resources: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut total = 0usize;
        checked_insert(&mut resources, &mut total, "a.xsd", b"one").unwrap();
        checked_insert(&mut resources, &mut total, "a.xsd", b"one")
            .expect("repeated reach of identical bytes is not a conflict");
        assert_eq!(resources.len(), 1);

        let err = checked_insert(&mut resources, &mut total, "a.xsd", b"two")
            .expect_err("different bytes for one normalized key must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("conflicting"),
            "error must name the conflict, got: {msg}"
        );
        assert!(
            !msg.contains("cap"),
            "a conflict must not be reported as a cap error, got: {msg}"
        );
    }

    // ── confinement ─────────────────────────────────────────────────────

    #[test]
    fn in_root_symlink_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let root_path = write_file(
            tmp.path(),
            "root.xsd",
            &xsd_with_refs("xs", &[("include", "link.xsd")]),
        );
        write_file(tmp.path(), "real.xsd", &xsd_empty());
        symlink(Path::new("real.xsd"), &tmp.path().join("link.xsd"));

        let set = discover_schema_set(&root_path, &empty_aliases())
            .expect("symlink whose canonical target is inside the root must be included");

        assert!(set.resources.contains_key("link.xsd"));
        assert_eq!(set.resources.len(), 2);
    }

    #[test]
    fn escaping_symlink_is_rejected() {
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_file(outside.path(), "evil.xsd", &xsd_empty());
        let root_path = write_file(
            inside.path(),
            "root.xsd",
            &xsd_with_refs("xs", &[("include", "escape.xsd")]),
        );
        symlink(
            &outside.path().join("evil.xsd"),
            &inside.path().join("escape.xsd"),
        );

        let err = discover_schema_set(&root_path, &empty_aliases())
            .expect_err("symlink escaping the canonical root must be refused");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn traversal_outside_root_is_rejected() {
        // Table: a `..` escape, a `..` escape hidden behind an in-root prefix,
        // an alias value that escapes, and absolute filesystem paths — raw and
        // percent-encoded, from the root importer and from a nested importer.
        // An absolute location must be refused BEFORE it is joined onto the
        // importing base: `/child.xsd` from `a/parent.xsd` would otherwise
        // collapse to the in-root decoy `a/child.xsd`. In-root `..` stays valid
        // (see the `custom` row of `reachable_imports_are_included`).
        for (label, nested, location, alias_value) in [
            ("dotdot", false, "../outside.xsd", None),
            ("nested dotdot", false, "sub/../../outside.xsd", None),
            ("alias escape", false, "child.xsd", Some("../outside.xsd")),
            ("absolute from root", false, "/child.xsd", None),
            ("encoded absolute from root", false, "%2Fchild.xsd", None),
            ("absolute from nested", true, "/child.xsd", None),
            ("encoded absolute from nested", true, "%2Fchild.xsd", None),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let root_path = if nested {
                let root = write_file(
                    tmp.path(),
                    "root.xsd",
                    &xsd_with_refs("xs", &[("include", "a/parent.xsd")]),
                );
                write_file(
                    tmp.path(),
                    "a/parent.xsd",
                    &xsd_with_refs("xs", &[("include", location)]),
                );
                root
            } else {
                write_file(
                    tmp.path(),
                    "root.xsd",
                    &xsd_with_refs("xs", &[("include", location)]),
                )
            };
            // Decoys the absolute form would wrongly reach after a base join.
            write_file(tmp.path(), "child.xsd", &xsd_empty());
            write_file(tmp.path(), "a/child.xsd", &xsd_empty());
            let mut aliases = BTreeMap::new();
            if let Some(value) = alias_value {
                aliases.insert("http://example.com/x.xsd".to_string(), value.to_string());
            }

            let err = discover_schema_set(&root_path, &aliases)
                .expect_err(&format!("{label}: expected refusal"));
            assert!(
                !err.to_string().is_empty(),
                "{label}: refusal must be a bounded, actionable error"
            );
        }
    }

    // ── single file, aliases, cache key ─────────────────────────────────

    #[test]
    fn single_file_without_imports_is_one_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let contents = xsd_with_refs("xs", &[]);
        let root_path = write_file(tmp.path(), "root.xsd", &contents);

        // Case A: no aliases — exactly one entry.
        let set: SchemaSet = discover_schema_set(&root_path, &empty_aliases()).unwrap();
        assert_eq!(
            set.resources.len(),
            1,
            "{:?}",
            set.resources.keys().collect::<Vec<_>>()
        );
        assert_eq!(set.root_system_id, "root.xsd");
        assert_eq!(set.root_bytes(), contents.as_bytes());
        let key = set.cache_key();
        assert!(
            key.starts_with("xsd-") && key.len() == 4 + 64,
            "cache key must be 'xsd-<64 hex>', got: {key}"
        );
        assert!(
            key[4..].chars().all(|c| c.is_ascii_hexdigit()),
            "cache key suffix must be hex, got: {key}"
        );
        let again = discover_schema_set(&root_path, &empty_aliases()).unwrap();
        assert_eq!(again.cache_key(), key, "cache key must be deterministic");

        // Case B: a supplied alias is materialized as a redundant URI-keyed
        // entry beside the relative target, and both count.
        let mut aliases = BTreeMap::new();
        aliases.insert(
            "http://example.com/root.xsd".to_string(),
            "root.xsd".to_string(),
        );
        let aliased = discover_schema_set(&root_path, &aliases).unwrap();
        assert_eq!(
            aliased.resources.len(),
            2,
            "alias + relative count as 2 entries"
        );
        assert!(aliased.resources.contains_key("root.xsd"));
        assert!(
            aliased
                .resources
                .contains_key("http://example.com/root.xsd")
        );
        assert_eq!(
            aliased.aliases, aliases,
            "the normalized alias map is retained"
        );
        assert_ne!(aliased.cache_key(), key, "an added alias rotates the key");
    }
}
