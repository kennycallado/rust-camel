//! Skills extension catalog: load a skills root into an immutable manifest.
//!
//! Project-owned types only — no rmcp import (ADR-0020). The catalog stores the
//! exact bytes it serves so a manifest digest always matches the served file.

use std::fs;
use std::fs::File;
use std::io::ErrorKind;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use percent_encoding::AsciiSet;
use percent_encoding::NON_ALPHANUMERIC;
use percent_encoding::utf8_percent_encode;
use sha2::Digest;
use sha2::Sha256;

use crate::error::McpError;

/// Number of skills returned by one `skills/list` page.
pub const SKILLS_PAGE_SIZE: usize = 50;

/// Maximum number of files (including `SKILL.md`) one skill may contain.
pub const MAX_SKILL_FILES: usize = 512;

/// Maximum total bytes one skill may contain.
pub const MAX_SKILL_BYTES: u64 = 16_777_216;

/// URI path segments keep RFC 3986 unreserved characters raw (minus the path
/// separator, which joins segments); everything else is percent-encoded.
const URI_SEGMENT_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Loaded, immutable manifest of every skill under one skills root.
#[derive(Debug)]
pub struct McpSkillCatalog {
    skills: Vec<McpSkillEntry>,
}

/// One skill: its `SKILL.md` entry URI, parsed frontmatter, and complete file
/// manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct McpSkillEntry {
    /// Manifest entry URI of the skill's `SKILL.md` (`skill://<dir>/SKILL.md`).
    pub uri: String,
    /// YAML frontmatter as JSON, with every field preserved.
    pub frontmatter: serde_json::Value,
    /// Every regular file under the skill directory, sorted by URI.
    pub files: Vec<McpSkillFile>,
}

/// One regular file served for a skill, digest and bytes included.
#[derive(Debug, Clone, PartialEq)]
pub struct McpSkillFile {
    /// `skill://<skill-directory>/<relative-path>` with encoded segments.
    pub uri: String,
    /// `sha256:<64 lowercase hex>` over [`McpSkillFile::bytes`].
    pub digest: String,
    /// Raw byte size of [`McpSkillFile::bytes`].
    pub size: u64,
    /// MIME type derived from the file extension.
    pub mime_type: &'static str,
    /// Exact bytes the server returns for this URI.
    pub bytes: Arc<[u8]>,
}

/// One page of the skills slice.
#[derive(Debug)]
pub struct SkillPage<'a> {
    /// Skills in this page, in deterministic order.
    pub skills: &'a [McpSkillEntry],
    /// Opaque cursor for the next page; `None` on the last page.
    pub next_cursor: Option<String>,
}

/// Failure to resolve an opaque `skills/list` cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillCursorError {
    /// The cursor is non-numeric or its offset is at or beyond the catalog
    /// length.
    Invalid,
}

impl McpSkillCatalog {
    /// Load every skill under `root`.
    ///
    /// Fails closed: a missing or non-directory root, an unresolvable path, a
    /// path escaping the canonical root, an invalid skill identity, or a skill
    /// over the file/byte limits returns a typed [`McpError`] and no partial
    /// catalog is ever produced.
    pub fn load(root: &Path) -> Result<Self, McpError> {
        let canonical_root = fs::canonicalize(root).map_err(|error| McpError::SkillLoad {
            skill: None,
            reason: format!(
                "skills root '{}' cannot be resolved: {error}",
                root.display()
            ),
        })?;
        let metadata = fs::metadata(&canonical_root).map_err(|error| McpError::SkillLoad {
            skill: None,
            reason: format!(
                "skills root '{}' cannot be inspected: {error}",
                root.display()
            ),
        })?;
        if !metadata.is_dir() {
            return Err(McpError::SkillLoad {
                skill: None,
                reason: format!("skills root '{}' is not a directory", root.display()),
            });
        }

        let entries = fs::read_dir(&canonical_root).map_err(|error| McpError::SkillLoad {
            skill: None,
            reason: format!("skills root '{}' cannot be read: {error}", root.display()),
        })?;
        let mut skills: Vec<(String, McpSkillEntry)> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| McpError::SkillLoad {
                skill: None,
                reason: format!("skills root '{}' cannot be read: {error}", root.display()),
            })?;
            let path = entry.path();
            let canonical = fs::canonicalize(&path).map_err(|error| McpError::SkillLoad {
                skill: None,
                reason: format!(
                    "skill path '{}' cannot be resolved: {error}",
                    path.display()
                ),
            })?;
            if !canonical.starts_with(&canonical_root) {
                return Err(McpError::SkillPathEscape {
                    path: canonical.display().to_string(),
                });
            }
            let metadata = fs::metadata(&canonical).map_err(|error| McpError::SkillLoad {
                skill: None,
                reason: format!(
                    "skill path '{}' cannot be inspected: {error}",
                    path.display()
                ),
            })?;
            if !metadata.is_dir() {
                continue;
            }
            // The walker validates the marker as a regular file when it builds
            // the manifest; this pre-check must fail the same way instead of
            // silently dropping a directory whose `SKILL.md` exists but is
            // unusable. The name is only diagnostic here, so a lossy spelling
            // keeps a stray non-UTF-8 *file* at the root from failing a load.
            let marker_name = entry.file_name().to_string_lossy().into_owned();
            if !skill_marker_state(&canonical, &canonical_root, &marker_name)? {
                continue;
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| McpError::SkillLoad {
                    skill: None,
                    reason: format!("skill directory '{}' has a non-UTF-8 name", path.display()),
                })?;
            skills.push((
                name.clone(),
                load_skill(&canonical_root, &canonical, &name)?,
            ));
        }
        skills.sort_by(|(left, _), (right, _)| left.cmp(right));

        Ok(Self {
            skills: skills.into_iter().map(|(_, skill)| skill).collect(),
        })
    }

    /// Every loaded skill, sorted by directory name.
    pub fn skills(&self) -> &[McpSkillEntry] {
        &self.skills
    }

    /// The skill whose `SKILL.md` URI equals `uri`.
    pub fn get(&self, uri: &str) -> Option<&McpSkillEntry> {
        self.skills.iter().find(|entry| entry.uri == uri)
    }

    /// The manifest file whose URI equals `uri`.
    pub fn file(&self, uri: &str) -> Option<&McpSkillFile> {
        self.skills
            .iter()
            .flat_map(|entry| entry.files.iter())
            .find(|file| file.uri == uri)
    }

    /// One page of skills starting at the opaque decimal `cursor`.
    ///
    /// `None` always returns the first page. A non-numeric cursor, or one whose
    /// offset is at or beyond the catalog length, is
    /// [`SkillCursorError::Invalid`]. A skill is never split across pages.
    pub fn page(&self, cursor: Option<&str>) -> Result<SkillPage<'_>, SkillCursorError> {
        let offset = match cursor {
            None => 0,
            Some(raw) => {
                let offset = raw
                    .parse::<usize>()
                    .map_err(|_| SkillCursorError::Invalid)?;
                if offset >= self.skills.len() {
                    return Err(SkillCursorError::Invalid);
                }
                offset
            }
        };
        let end = offset
            .saturating_add(SKILLS_PAGE_SIZE)
            .min(self.skills.len());
        let next_cursor = if end < self.skills.len() {
            Some(offset.saturating_add(SKILLS_PAGE_SIZE).to_string())
        } else {
            None
        };
        Ok(SkillPage {
            skills: &self.skills[offset..end],
            next_cursor,
        })
    }

    /// Number of loaded skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether the catalog holds no skills.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

/// One candidate file found during the iterative skill walk.
struct FileCandidate {
    /// Canonical path on disk used to read the bytes.
    path: PathBuf,
    /// Logical path segments relative to the skill directory (used for URIs).
    segments: Vec<String>,
}

/// One directory still to be walked: its canonical path, the logical path
/// segments leading to it, and the canonical directories on the stack from the
/// skill root down to it.
struct PendingDir {
    /// Canonical directory whose children are read next.
    path: PathBuf,
    /// Logical path segments relative to the skill directory (URI spelling).
    segments: Vec<String>,
    /// Canonical ancestors, this directory included. A child canonicalizing to
    /// an entry here is a symlink cycle; two aliases of one directory carry
    /// separate stacks, so in-root aliases are legal and each is listed under
    /// its own logical path.
    ancestry: Vec<PathBuf>,
}

/// Whether `dir` holds a `SKILL.md` marker the walker can serve, or `false`
/// when there is no marker at all.
///
/// An entry that exists but cannot be served as a regular file is an error,
/// never a silent skip: a `SKILL.md` directory, fifo, socket, escaping
/// symlink, or dangling symlink fails the load (spec: a skill with an
/// unusable marker is reported, not dropped). Absence alone means "not a
/// skill" and leaves the directory out of the catalog.
fn skill_marker_state(dir: &Path, root: &Path, name: &str) -> Result<bool, McpError> {
    let marker = dir.join("SKILL.md");
    let metadata = match fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(skill_error(
                name,
                format!("cannot inspect '{}': {error}", marker.display()),
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        let canonical = fs::canonicalize(&marker).map_err(|error| {
            skill_error(
                name,
                format!("cannot resolve '{}': {error}", marker.display()),
            )
        })?;
        if !canonical.starts_with(root) {
            return Err(McpError::SkillPathEscape {
                path: canonical.display().to_string(),
            });
        }
        let target = fs::metadata(&canonical).map_err(|error| {
            skill_error(
                name,
                format!("cannot inspect '{}': {error}", canonical.display()),
            )
        })?;
        if !target.is_file() {
            return Err(skill_error(
                name,
                format!("'{}' resolves to a non-regular file", marker.display()),
            ));
        }
        return Ok(true);
    }
    if metadata.is_file() {
        return Ok(true);
    }
    Err(skill_error(
        name,
        format!("'{}' is not a regular file", marker.display()),
    ))
}

fn load_skill(root: &Path, skill_dir: &Path, name: &str) -> Result<McpSkillEntry, McpError> {
    // Ancestry-based cycle detection (not a global visited set): a directory
    // reached again through a different logical path is an in-root alias and
    // is walked again under that alias, so every logical path the tree spells
    // appears in the manifest and counts against the limits. Only a symlink
    // that resolves to one of its own ancestors is a cycle.
    let mut pending: Vec<PendingDir> = vec![PendingDir {
        path: skill_dir.to_path_buf(),
        segments: Vec::new(),
        ancestry: vec![skill_dir.to_path_buf()],
    }];
    let mut candidates: Vec<FileCandidate> = Vec::new();
    let mut declared_bytes: u64 = 0;

    while let Some(dir) = pending.pop() {
        let entries = fs::read_dir(&dir.path).map_err(|error| {
            skill_error(
                name,
                format!("cannot read directory '{}': {error}", dir.path.display()),
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                skill_error(
                    name,
                    format!("cannot read directory '{}': {error}", dir.path.display()),
                )
            })?;
            let file_name = entry.file_name().into_string().map_err(|_| {
                skill_error(
                    name,
                    format!("non-UTF-8 file name under '{}'", dir.path.display()),
                )
            })?;
            let path = entry.path();
            let canonical = fs::canonicalize(&path).map_err(|error| {
                skill_error(
                    name,
                    format!("cannot resolve '{}': {error}", path.display()),
                )
            })?;
            if !canonical.starts_with(root) {
                return Err(McpError::SkillPathEscape {
                    path: canonical.display().to_string(),
                });
            }
            let metadata = fs::metadata(&canonical).map_err(|error| {
                skill_error(
                    name,
                    format!("cannot inspect '{}': {error}", path.display()),
                )
            })?;

            let mut child_segments = dir.segments.clone();
            child_segments.push(file_name);
            if metadata.is_dir() {
                if dir.ancestry.contains(&canonical) {
                    return Err(skill_error(name, "symlink cycle"));
                }
                let mut ancestry = dir.ancestry.clone();
                ancestry.push(canonical.clone());
                pending.push(PendingDir {
                    path: canonical,
                    segments: child_segments,
                    ancestry,
                });
            } else if metadata.is_file() {
                declared_bytes = declared_bytes.saturating_add(metadata.len());
                if candidates.len() == MAX_SKILL_FILES || declared_bytes > MAX_SKILL_BYTES {
                    return Err(skill_error(
                        name,
                        format!(
                            "exceeds the per-skill limits \
                             ({MAX_SKILL_FILES} files, {MAX_SKILL_BYTES} bytes)"
                        ),
                    ));
                }
                candidates.push(FileCandidate {
                    path: canonical,
                    segments: child_segments,
                });
            } else {
                return Err(skill_error(
                    name,
                    format!(
                        "entry '{}' is neither a regular file nor a directory",
                        path.display()
                    ),
                ));
            }
        }
    }

    let mut ordered: Vec<(String, FileCandidate)> = candidates
        .into_iter()
        .map(|candidate| (skill_uri(name, &candidate.segments), candidate))
        .collect();
    ordered.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut files = Vec::with_capacity(ordered.len());
    let mut frontmatter: Option<serde_json::Value> = None;
    let mut read_bytes: u64 = 0;
    for (uri, candidate) in ordered {
        let remaining = MAX_SKILL_BYTES - read_bytes;
        let mut bytes = Vec::new();
        File::open(&candidate.path)
            .map_err(|error| skill_io_error(name, &candidate.path, error))?
            .take(remaining + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| skill_io_error(name, &candidate.path, error))?;
        if bytes.len() as u64 > remaining {
            return Err(skill_error(
                name,
                format!(
                    "file '{}' exceeds the per-skill byte budget",
                    candidate.path.display()
                ),
            ));
        }
        read_bytes += bytes.len() as u64;

        if candidate.segments.len() == 1 && candidate.segments[0] == "SKILL.md" {
            frontmatter = Some(parse_frontmatter(name, &bytes)?);
        }
        files.push(McpSkillFile {
            uri,
            digest: sha256_digest(&bytes),
            size: bytes.len() as u64,
            mime_type: mime_type(&candidate.segments),
            bytes: Arc::from(bytes),
        });
    }

    let frontmatter =
        frontmatter.ok_or_else(|| skill_error(name, "skill has no regular SKILL.md file"))?;
    Ok(McpSkillEntry {
        uri: skill_uri(name, &[String::from("SKILL.md")]),
        frontmatter,
        files,
    })
}

fn parse_frontmatter(skill: &str, bytes: &[u8]) -> Result<serde_json::Value, McpError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| skill_error(skill, format!("SKILL.md is not valid UTF-8: {error}")))?;
    let yaml = frontmatter_yaml(text)
        .ok_or_else(|| skill_error(skill, "SKILL.md has no YAML frontmatter"))?;

    let config = noyalib::ParserConfig::serde_yaml_compat()
        .duplicate_key_policy(noyalib::DuplicateKeyPolicy::Error);
    let value = noyalib::from_str_with_config::<noyalib::Value>(yaml, &config)
        .map_err(|error| skill_error(skill, format!("invalid frontmatter: {error}")))?;
    reject_unrepresentable(&value).map_err(|reason| skill_error(skill, reason))?;
    let json = serde_json::to_value(&value)
        .map_err(|error| skill_error(skill, format!("frontmatter is not JSON: {error}")))?;

    let object = json
        .as_object()
        .ok_or_else(|| skill_error(skill, "frontmatter must be a mapping"))?;
    let name = object
        .get("name")
        .and_then(|field| field.as_str())
        .ok_or_else(|| skill_error(skill, "frontmatter field 'name' must be a string"))?;
    let description = object
        .get("description")
        .and_then(|field| field.as_str())
        .ok_or_else(|| skill_error(skill, "frontmatter field 'description' must be a string"))?;

    if !valid_skill_name(name) {
        return Err(skill_error(skill, format!("invalid skill name '{name}'")));
    }
    let description_len = description.chars().count();
    if description_len == 0 || description_len > 1024 {
        return Err(skill_error(
            skill,
            "description must be 1 to 1024 characters",
        ));
    }
    if name != skill {
        return Err(skill_error(
            skill,
            format!("frontmatter name '{name}' does not match the directory name"),
        ));
    }

    Ok(json)
}

/// The YAML body between the opening `---` line and the closing `---` or `...`
/// line, or `None` when the file does not start with frontmatter or the closing
/// line is absent.
fn frontmatter_yaml(text: &str) -> Option<&str> {
    let rest = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" || trimmed == "..." {
            return rest.get(..offset);
        }
        offset += line.len();
    }
    None
}

/// Reject frontmatter that cannot render faithfully as JSON.
fn reject_unrepresentable(value: &noyalib::Value) -> Result<(), String> {
    match value {
        noyalib::Value::Number(number) => {
            if number.as_f64().is_finite() {
                Ok(())
            } else {
                Err("frontmatter contains a non-finite number".to_string())
            }
        }
        noyalib::Value::Tagged(tagged) => Err(format!(
            "frontmatter contains the unsupported tag '{}'",
            tagged.tag().as_str()
        )),
        noyalib::Value::Mapping(mapping) => {
            for (_, item) in mapping.iter() {
                reject_unrepresentable(item)?;
            }
            Ok(())
        }
        noyalib::Value::Sequence(sequence) => {
            for item in sequence {
                reject_unrepresentable(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// 1–64 characters of `[a-z0-9-]`, no leading/trailing hyphen, no `--`.
fn valid_skill_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn mime_type(segments: &[String]) -> &'static str {
    let extension = segments
        .last()
        .and_then(|segment| Path::new(segment).extension())
        .and_then(|extension| extension.to_str());
    match extension {
        Some("md") => "text/markdown",
        Some("json") => "application/json",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

fn sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("sha256:{hex}")
}

fn encode_segment(segment: &str) -> String {
    utf8_percent_encode(segment, URI_SEGMENT_SET).to_string()
}

fn skill_uri(name: &str, segments: &[String]) -> String {
    let mut uri = String::from("skill://");
    uri.push_str(&encode_segment(name));
    for segment in segments {
        uri.push('/');
        uri.push_str(&encode_segment(segment));
    }
    uri
}

fn skill_error(skill: &str, detail: impl Into<String>) -> McpError {
    let detail = detail.into();
    McpError::SkillLoad {
        skill: Some(skill.to_string()),
        reason: format!("skill '{skill}': {detail}"),
    }
}

fn skill_io_error(skill: &str, path: &Path, error: std::io::Error) -> McpError {
    McpError::SkillLoad {
        skill: Some(skill.to_string()),
        reason: format!("cannot read '{}': {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    use sha2::Digest;
    use sha2::Sha256;

    use super::*;
    use crate::error::McpError;

    const CODE_REVIEW_SKILL_MD: &str =
        "---\nname: code-review\ndescription: Review code.\nlicense: MIT\n---\n# Code review\n";

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(tag: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "camel-mcp-skills-unit-{tag}-{}-{sequence}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create test dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn join(&self, relative: &str) -> PathBuf {
            self.path.join(relative)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, bytes).expect("write file");
    }

    fn write_skill(root: &Path, dir: &str, skill_md: &str, files: &[(&str, &[u8])]) {
        let skill = root.join(dir);
        write_file(&skill.join("SKILL.md"), skill_md.as_bytes());
        for (relative, bytes) in files {
            write_file(&skill.join(relative), bytes);
        }
    }

    fn write_sparse(path: &Path, len: u64) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        let file = fs::File::create(path).expect("create sparse file");
        file.set_len(len).expect("set sparse file length");
    }

    fn digest_of(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            hex.push_str(&format!("{byte:02x}"));
        }
        format!("sha256:{hex}")
    }

    #[test]
    fn catalog_builds_complete_manifest_with_digests() {
        let root = TestDir::new("manifest");
        write_skill(
            root.path(),
            "code-review",
            CODE_REVIEW_SKILL_MD,
            &[("references/checklist.md", b"# checklist\n")],
        );

        let catalog = McpSkillCatalog::load(root.path()).expect("catalog loads");

        assert_eq!(catalog.len(), 1);
        let entry = &catalog.skills()[0];
        assert_eq!(entry.uri, "skill://code-review/SKILL.md");
        assert_eq!(entry.frontmatter["name"], serde_json::json!("code-review"));
        assert_eq!(
            entry.frontmatter["description"],
            serde_json::json!("Review code.")
        );
        assert_eq!(entry.frontmatter["license"], serde_json::json!("MIT"));
        assert_eq!(entry.files.len(), 2);
        assert_eq!(entry.files[0].uri, "skill://code-review/SKILL.md");

        let expected = [
            (
                "skill://code-review/SKILL.md",
                CODE_REVIEW_SKILL_MD.as_bytes(),
            ),
            (
                "skill://code-review/references/checklist.md",
                b"# checklist\n".as_slice(),
            ),
        ];
        for (file, (uri, bytes)) in entry.files.iter().zip(expected) {
            assert_eq!(file.uri, uri);
            assert_eq!(file.digest, digest_of(bytes));
            assert_eq!(file.size, bytes.len() as u64);
            assert_eq!(&*file.bytes, bytes);
        }
    }

    #[test]
    fn catalog_percent_encodes_uri_segments() {
        let root = TestDir::new("percent");
        write_skill(
            root.path(),
            "code-review",
            CODE_REVIEW_SKILL_MD,
            &[("references/my notes.md", b"# notes\n")],
        );

        let catalog = McpSkillCatalog::load(root.path()).expect("catalog loads");

        let entry = &catalog.skills()[0];
        assert!(
            entry
                .files
                .iter()
                .any(|file| file.uri == "skill://code-review/references/my%20notes.md"),
            "files: {:?}",
            entry.files.iter().map(|f| &f.uri).collect::<Vec<_>>()
        );
    }

    #[test]
    fn catalog_rejects_invalid_skill_identity() {
        let long_name = "a".repeat(65);
        let cases: Vec<(&str, String, &str)> = vec![
            (
                "code-review",
                "# no frontmatter\n".to_string(),
                "code-review",
            ),
            (
                "code-review",
                "---\nname: other\ndescription: Review code.\n---\n".to_string(),
                "code-review",
            ),
            (
                "Bad_Name",
                "---\nname: Bad_Name\ndescription: Review code.\n---\n".to_string(),
                "Bad_Name",
            ),
            (
                "code-review",
                "---\nname: code-review\ndescription: \"\"\n---\n".to_string(),
                "code-review",
            ),
            (
                "code-review",
                format!("---\nname: {long_name}\ndescription: Review code.\n---\n"),
                "code-review",
            ),
            (
                "a--b",
                "---\nname: a--b\ndescription: Review code.\n---\n".to_string(),
                "a--b",
            ),
            (
                "code-review",
                "---\ndescription: Review code.\n---\n".to_string(),
                "code-review",
            ),
        ];

        for (dir, skill_md, expected_skill) in cases {
            let root = TestDir::new("identity");
            write_skill(root.path(), dir, &skill_md, &[]);
            let error = McpSkillCatalog::load(root.path())
                .expect_err("invalid skill identity must fail the load");
            match error {
                McpError::SkillLoad { skill, .. } => {
                    assert_eq!(
                        skill.as_deref(),
                        Some(expected_skill),
                        "case for directory '{dir}'"
                    );
                }
                other => panic!("expected SkillLoad for directory '{dir}', got {other:?}"),
            }
        }
    }

    #[test]
    fn catalog_rejects_duplicate_and_nonfinite_frontmatter() {
        let duplicate =
            "---\nname: code-review\nname: code-review\ndescription: Review code.\n---\n";
        let nonfinite = "---\nname: code-review\ndescription: Review code.\nx: .inf\n---\n";

        for skill_md in [duplicate, nonfinite] {
            let root = TestDir::new("frontmatter");
            write_skill(root.path(), "code-review", skill_md, &[]);
            match McpSkillCatalog::load(root.path()) {
                Err(McpError::SkillLoad { skill, .. }) => {
                    assert_eq!(skill.as_deref(), Some("code-review"));
                }
                other => panic!("expected SkillLoad, got {other:?}"),
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn catalog_rejects_escaping_symlink() {
        use std::os::unix::fs::symlink;

        let outer = TestDir::new("escape");
        let root = outer.join("root");
        write_file(&outer.join("outside.txt"), b"secret\n");
        write_skill(&root, "code-review", CODE_REVIEW_SKILL_MD, &[]);
        symlink(outer.join("outside.txt"), root.join("code-review/leak.md"))
            .expect("create escaping symlink");

        let error = McpSkillCatalog::load(&root).expect_err("escaping symlink must fail");
        assert!(
            matches!(error, McpError::SkillPathEscape { .. }),
            "expected SkillPathEscape, got {error:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn catalog_allows_in_root_directory_aliases() {
        use std::os::unix::fs::symlink;

        let root = TestDir::new("aliases");
        write_skill(
            root.path(),
            "code-review",
            CODE_REVIEW_SKILL_MD,
            &[("shared/extra.md", b"# extra\n")],
        );
        let skill = root.join("code-review");
        symlink("shared", skill.join("a")).expect("create alias a");
        symlink("shared", skill.join("b")).expect("create alias b");

        let catalog = McpSkillCatalog::load(root.path()).expect("in-root aliases must be legal");

        let uris: Vec<&str> = catalog.skills()[0]
            .files
            .iter()
            .map(|file| file.uri.as_str())
            .collect();
        assert!(
            uris.contains(&"skill://code-review/SKILL.md"),
            "the marker must be listed: {uris:?}"
        );
        assert!(
            uris.contains(&"skill://code-review/a/extra.md"),
            "alias a must be listed under its logical path: {uris:?}"
        );
        assert!(
            uris.contains(&"skill://code-review/b/extra.md"),
            "alias b must be listed under its logical path: {uris:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn catalog_rejects_symlink_cycle() {
        use std::os::unix::fs::symlink;

        let root = TestDir::new("cycle");
        write_skill(root.path(), "code-review", CODE_REVIEW_SKILL_MD, &[]);
        symlink(".", root.join("code-review/loop")).expect("create cycle symlink");

        let error = McpSkillCatalog::load(root.path()).expect_err("a symlink cycle must fail");
        match error {
            McpError::SkillLoad { skill, reason } => {
                assert_eq!(skill.as_deref(), Some("code-review"));
                assert!(
                    reason.contains("symlink cycle"),
                    "the error must name the cycle, got: {reason}"
                );
            }
            other => panic!("expected SkillLoad, got {other:?}"),
        }
    }

    #[test]
    fn catalog_rejects_broken_skill_markers() {
        // (a) `SKILL.md` exists as a directory: the skill is unusable and must
        // be reported, not silently dropped from the catalog.
        let root = TestDir::new("marker-dir");
        write_skill(root.path(), "code-review", CODE_REVIEW_SKILL_MD, &[]);
        fs::remove_file(root.join("code-review/SKILL.md")).expect("remove marker file");
        fs::create_dir(root.join("code-review/SKILL.md")).expect("marker as directory");

        let error =
            McpSkillCatalog::load(root.path()).expect_err("a marker directory must fail the load");
        match error {
            McpError::SkillLoad { skill, .. } => {
                assert_eq!(skill.as_deref(), Some("code-review"));
            }
            other => panic!("expected SkillLoad, got {other:?}"),
        }

        // (b) `SKILL.md` is a symlink whose target does not exist: resolving
        // it fails, which is a load error rather than "not a skill".
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let root = TestDir::new("marker-dangling");
            write_skill(root.path(), "code-review", CODE_REVIEW_SKILL_MD, &[]);
            fs::remove_file(root.join("code-review/SKILL.md")).expect("remove marker file");
            symlink(
                root.join("code-review/missing.md"),
                root.join("code-review/SKILL.md"),
            )
            .expect("create dangling marker symlink");

            let error = McpSkillCatalog::load(root.path())
                .expect_err("a dangling marker symlink must fail the load");
            match error {
                McpError::SkillLoad { skill, .. } => {
                    assert_eq!(skill.as_deref(), Some("code-review"));
                }
                other => panic!("expected SkillLoad, got {other:?}"),
            }
        }
    }

    #[test]
    fn catalog_first_page_is_valid_on_an_empty_catalog() {
        let root = TestDir::new("empty");
        write_file(&root.join("README.md"), b"# no skills here\n");

        let catalog = McpSkillCatalog::load(root.path()).expect("catalog loads");

        let first = catalog.page(None).expect("first page is always valid");
        assert!(first.skills.is_empty());
        assert_eq!(first.next_cursor, None);
        assert!(matches!(
            catalog.page(Some("0")),
            Err(SkillCursorError::Invalid)
        ));
    }

    #[test]
    fn skill_limits_are_inclusive_at_512_files_and_16_mib() {
        let root_512 = TestDir::new("files-512");
        write_skill(root_512.path(), "code-review", CODE_REVIEW_SKILL_MD, &[]);
        for index in 0..511 {
            write_file(
                &root_512
                    .join("code-review")
                    .join(format!("file-{index:03}.txt")),
                b"x",
            );
        }
        let catalog =
            McpSkillCatalog::load(root_512.path()).expect("512 files are within the limit");
        assert_eq!(catalog.skills()[0].files.len(), 512);

        let root_513 = TestDir::new("files-513");
        write_skill(root_513.path(), "code-review", CODE_REVIEW_SKILL_MD, &[]);
        for index in 0..512 {
            write_file(
                &root_513
                    .join("code-review")
                    .join(format!("file-{index:03}.txt")),
                b"x",
            );
        }
        assert!(matches!(
            McpSkillCatalog::load(root_513.path()),
            Err(McpError::SkillLoad { .. })
        ));

        let exact = TestDir::new("bytes-exact");
        write_skill(exact.path(), "code-review", CODE_REVIEW_SKILL_MD, &[]);
        let filler = MAX_SKILL_BYTES - CODE_REVIEW_SKILL_MD.len() as u64;
        write_sparse(&exact.join("code-review/big.bin"), filler);
        let catalog =
            McpSkillCatalog::load(exact.path()).expect("16 MiB total is within the limit");
        let total: u64 = catalog.skills()[0].files.iter().map(|file| file.size).sum();
        assert_eq!(total, MAX_SKILL_BYTES);

        let over = TestDir::new("bytes-over");
        write_skill(over.path(), "code-review", CODE_REVIEW_SKILL_MD, &[]);
        write_sparse(&over.join("code-review/big.bin"), filler + 1);
        assert!(matches!(
            McpSkillCatalog::load(over.path()),
            Err(McpError::SkillLoad { .. })
        ));
    }
}
