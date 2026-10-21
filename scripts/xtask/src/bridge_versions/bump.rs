//! Atomic bridge version bump over the registry and registered constants.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

use super::registry::{self, Bridge};

/// Failure from `bump-bridge`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BumpError {
    /// The requested bridge is not `cxf`, `jms`, or `xml`.
    InvalidBridge(String),
    /// The requested version is not a registry-supported semantic version.
    InvalidVersion(String),
    /// Reading or validating the registry/constants failed before any write.
    Snapshot(String),
    /// A write, persist, readback, or hook failure was rolled back.
    Write { detail: String, rolled_back: bool },
    /// Post-write lint failed and the snapshots were rolled back.
    PostWrite { detail: String, rolled_back: bool },
    /// A rollback itself failed.
    Rollback { original: String, rollback: String },
}

impl fmt::Display for BumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BumpError::InvalidBridge(bridge) => {
                write!(f, "invalid bridge '{bridge}': expected cxf, jms, or xml")
            }
            BumpError::InvalidVersion(version) => write!(
                f,
                "invalid version '{version}': expected MAJOR.MINOR.PATCH with optional prerelease"
            ),
            BumpError::Snapshot(detail) => write!(f, "snapshot failed: {detail}"),
            BumpError::Write {
                detail,
                rolled_back,
            } => {
                write!(f, "write failed: {detail}")?;
                if *rolled_back {
                    write!(f, "; rolled back every snapshot")?;
                }
                Ok(())
            }
            BumpError::PostWrite {
                detail,
                rolled_back,
            } => {
                write!(f, "post-write verification failed: {detail}")?;
                if *rolled_back {
                    write!(f, "; rolled back every snapshot")?;
                }
                Ok(())
            }
            BumpError::Rollback { original, rollback } => write!(
                f,
                "rollback failed: original failure: {original}; rollback failure: {rollback}"
            ),
        }
    }
}

impl std::error::Error for BumpError {}

/// Files changed by a successful bump, relative to the workspace root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BumpReport {
    pub changed: Vec<PathBuf>,
}

/// Invocation-local progress events used to inject failures in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BumpEvent {
    BeforeWrite(PathBuf),
    AfterPersist(PathBuf),
    AfterReadback(PathBuf),
    BeforePostLint,
}

struct Snapshot {
    absolute: PathBuf,
    relative: PathBuf,
    original: Vec<u8>,
    updated: Option<Vec<u8>>,
}

/// Bump `bridge_name` to `version` and run the bridge-version lint in-process.
///
/// Input validation, registry parsing, target reads, and constant comparison
/// complete before the first write. Any later failure restores every target
/// byte-for-byte.
pub fn bump_bridge(root: &Path, bridge_name: &str, version: &str) -> Result<BumpReport, BumpError> {
    bump_bridge_with_hook(root, bridge_name, version, &mut |_| Ok(()))
}

/// Bump with an invocation-local hook so tests can inject a failure at an
/// exact point without process-wide mutable state.
pub(crate) fn bump_bridge_with_hook<F>(
    root: &Path,
    bridge_name: &str,
    version: &str,
    hook: &mut F,
) -> Result<BumpReport, BumpError>
where
    F: FnMut(&BumpEvent) -> Result<(), String>,
{
    let bridge = Bridge::parse(bridge_name)
        .ok_or_else(|| BumpError::InvalidBridge(bridge_name.to_string()))?;
    if !registry::is_semver(version) {
        return Err(BumpError::InvalidVersion(version.to_string()));
    }

    let registry_path = root.join("bridges/versions.toml");
    let registry = registry::parse_registry(&registry_path)
        .map_err(|findings| BumpError::Snapshot(join_findings(&findings)))?;
    let old = registry.version(bridge).to_string();

    let mut relatives = vec![PathBuf::from("bridges/versions.toml")];
    relatives.extend(bridge.const_paths().iter().map(PathBuf::from));

    let mut snapshots = Vec::with_capacity(relatives.len());
    for relative in relatives {
        let absolute = root.join(&relative);
        let original = fs::read(&absolute).map_err(|error| {
            BumpError::Snapshot(format!("read {}: {error}", absolute.display()))
        })?;
        snapshots.push(Snapshot {
            absolute,
            relative,
            original,
            updated: None,
        });
    }

    // Precheck the registry line before any write.
    let registry_text = snapshot_text(&snapshots[0])?;
    let expected_registry = format!("{} = \"{old}\"", bridge.key());
    let replacement_registry = format!("{} = \"{version}\"", bridge.key());
    let registry_updated =
        replace_exactly_one(registry_text, &expected_registry, &replacement_registry).ok_or_else(
            || {
                BumpError::Snapshot(format!(
                    "{} does not contain exactly one line '{}'",
                    snapshots[0].absolute.display(),
                    expected_registry
                ))
            },
        )?;
    if registry_updated.as_bytes() != snapshots[0].original {
        snapshots[0].updated = Some(registry_updated.into_bytes());
    }

    // Compare every target constant with the registry before any write.
    let expected_constant = format!("pub const BRIDGE_VERSION: &str = \"{old}\";");
    let replacement_constant = format!("pub const BRIDGE_VERSION: &str = \"{version}\";");
    for snapshot in snapshots.iter_mut().skip(1) {
        let text = snapshot_text(snapshot)?;
        let declarations: Vec<&str> = text.lines().filter_map(declaration_value).collect();
        match declarations.as_slice() {
            [value] if *value == old => {}
            [] => {
                return Err(BumpError::Snapshot(format!(
                    "{} has no BRIDGE_VERSION declaration",
                    snapshot.absolute.display()
                )));
            }
            [value] => {
                return Err(BumpError::Snapshot(format!(
                    "{} declares '{}' but the registry says '{}'",
                    snapshot.absolute.display(),
                    value,
                    old
                )));
            }
            _ => {
                return Err(BumpError::Snapshot(format!(
                    "{} has more than one BRIDGE_VERSION declaration",
                    snapshot.absolute.display()
                )));
            }
        }

        let updated = replace_exactly_one(text, &expected_constant, &replacement_constant)
            .ok_or_else(|| {
                BumpError::Snapshot(format!(
                    "{} does not contain exactly one line '{}'",
                    snapshot.absolute.display(),
                    expected_constant
                ))
            })?;
        if updated != text {
            snapshot.updated = Some(updated.into_bytes());
        }
    }

    let mut changed = Vec::new();
    for snapshot in &snapshots {
        let Some(updated) = &snapshot.updated else {
            continue;
        };
        if let Err(detail) = write_target(snapshot, updated, hook) {
            return Err(rollback_after_write(&snapshots, detail));
        }
        changed.push(snapshot.relative.clone());
    }

    if let Err(error) = hook(&BumpEvent::BeforePostLint) {
        return Err(rollback_after_post(
            &snapshots,
            format!("hook before post-lint: {error}"),
        ));
    }

    let outcome = match super::lint(root, None) {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(rollback_after_post(
                &snapshots,
                format!("lint after write: {error}"),
            ));
        }
    };
    if let Some(error) = outcome.tag_error {
        return Err(rollback_after_post(
            &snapshots,
            format!("tag check after write: {error}"),
        ));
    }
    if !outcome.findings.is_empty() {
        return Err(rollback_after_post(
            &snapshots,
            format!(
                "lint findings after write: {}",
                join_findings(&outcome.findings)
            ),
        ));
    }

    Ok(BumpReport { changed })
}

fn write_target<F>(snapshot: &Snapshot, updated: &[u8], hook: &mut F) -> Result<(), String>
where
    F: FnMut(&BumpEvent) -> Result<(), String>,
{
    hook(&BumpEvent::BeforeWrite(snapshot.relative.clone()))
        .map_err(|error| format!("hook before write {}: {error}", snapshot.relative.display()))?;

    let parent = snapshot.absolute.parent().ok_or_else(|| {
        format!(
            "target {} has no parent directory",
            snapshot.absolute.display()
        )
    })?;
    let mut temp = NamedTempFile::new_in(parent)
        .map_err(|error| format!("create temp file in {}: {error}", parent.display()))?;
    temp.write_all(updated).map_err(|error| {
        format!(
            "write temp file for {}: {error}",
            snapshot.absolute.display()
        )
    })?;
    temp.as_file_mut().flush().map_err(|error| {
        format!(
            "flush temp file for {}: {error}",
            snapshot.absolute.display()
        )
    })?;
    temp.persist(&snapshot.absolute)
        .map_err(|error| format!("persist {}: {}", snapshot.absolute.display(), error.error))?;

    hook(&BumpEvent::AfterPersist(snapshot.relative.clone())).map_err(|error| {
        format!(
            "hook after persist {}: {error}",
            snapshot.relative.display()
        )
    })?;

    let actual = fs::read(&snapshot.absolute)
        .map_err(|error| format!("read back {}: {error}", snapshot.absolute.display()))?;
    if actual != updated {
        return Err(format!(
            "read back {} differs from written bytes",
            snapshot.absolute.display()
        ));
    }

    hook(&BumpEvent::AfterReadback(snapshot.relative.clone())).map_err(|error| {
        format!(
            "hook after readback {}: {error}",
            snapshot.relative.display()
        )
    })?;

    Ok(())
}

fn rollback_after_write(snapshots: &[Snapshot], original: String) -> BumpError {
    match restore_all(snapshots) {
        Ok(()) => BumpError::Write {
            detail: original,
            rolled_back: true,
        },
        Err(rollback) => BumpError::Rollback { original, rollback },
    }
}

fn rollback_after_post(snapshots: &[Snapshot], original: String) -> BumpError {
    match restore_all(snapshots) {
        Ok(()) => BumpError::PostWrite {
            detail: original,
            rolled_back: true,
        },
        Err(rollback) => BumpError::Rollback { original, rollback },
    }
}

fn restore_all(snapshots: &[Snapshot]) -> Result<(), String> {
    let mut failures = Vec::new();
    for snapshot in snapshots {
        if let Err(error) = fs::write(&snapshot.absolute, &snapshot.original) {
            failures.push(format!("restore {}: {error}", snapshot.absolute.display()));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn snapshot_text(snapshot: &Snapshot) -> Result<&str, BumpError> {
    std::str::from_utf8(&snapshot.original).map_err(|error| {
        BumpError::Snapshot(format!(
            "{} is not UTF-8: {error}",
            snapshot.absolute.display()
        ))
    })
}

fn declaration_value(line: &str) -> Option<&str> {
    const PREFIX: &str = "pub const BRIDGE_VERSION: &str = \"";
    const SUFFIX: &str = "\";";

    let trimmed = line.trim();
    let rest = trimmed.strip_prefix(PREFIX)?;
    let value = rest.strip_suffix(SUFFIX)?;
    (!value.contains('"')).then_some(value)
}

/// Replace exactly one trimmed line match, preserving indentation and the
/// original line ending. Returns `None` when the count is not exactly one.
fn replace_exactly_one(contents: &str, expected: &str, replacement: &str) -> Option<String> {
    let matches = contents
        .split_inclusive('\n')
        .filter(|segment| split_line_ending(segment).0.trim() == expected)
        .count();
    if matches != 1 {
        return None;
    }

    let mut updated = String::with_capacity(contents.len());
    let mut replaced = false;
    for segment in contents.split_inclusive('\n') {
        let (line, ending) = split_line_ending(segment);
        if !replaced && line.trim() == expected {
            let start = line.len() - line.trim_start().len();
            let end = start + line.trim().len();
            updated.push_str(&line[..start]);
            updated.push_str(replacement);
            updated.push_str(&line[end..]);
            replaced = true;
        } else {
            updated.push_str(line);
        }
        updated.push_str(ending);
    }

    Some(updated)
}

fn split_line_ending(segment: &str) -> (&str, &str) {
    if let Some(line) = segment.strip_suffix("\r\n") {
        (line, "\r\n")
    } else if let Some(line) = segment.strip_suffix('\n') {
        (line, "\n")
    } else {
        (segment, "")
    }
}

fn join_findings(findings: &[super::Finding]) -> String {
    findings
        .iter()
        .map(|finding| finding.detail.clone())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_versions::lint;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    const REGISTRY: &str = "bridges/versions.toml";
    const CXF: &str = "crates/components/camel-cxf/src/lib.rs";
    const JMS: &str = "crates/components/camel-jms/src/lib.rs";
    const XSLT: &str = "crates/components/camel-xslt/src/lib.rs";
    const VALIDATOR: &str = "crates/components/camel-validator/src/lib.rs";

    const TARGETS: [&str; 5] = [REGISTRY, CXF, JMS, XSLT, VALIDATOR];

    fn write_file(root: &Path, relative_path: &str, contents: &str) {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture directory");
        }
        std::fs::write(path, contents).expect("write fixture file");
    }

    fn constant(version: &str) -> String {
        format!("pub const BRIDGE_VERSION: &str = \"{version}\";\n")
    }

    fn fixture() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        write_file(
            dir.path(),
            REGISTRY,
            "cxf = \"0.8.4\"\njms = \"0.8.4\"\nxml = \"0.8.1\"\n",
        );
        write_file(dir.path(), CXF, &constant("0.8.4"));
        write_file(dir.path(), JMS, &constant("0.8.4"));
        write_file(dir.path(), XSLT, &constant("0.8.1"));
        write_file(dir.path(), VALIDATOR, &constant("0.8.1"));
        dir
    }

    fn snapshots(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        TARGETS
            .iter()
            .map(|relative| {
                let path = root.join(relative);
                let bytes = std::fs::read(&path).expect("snapshot target");
                (path, bytes)
            })
            .collect()
    }

    fn assert_snapshots_unchanged(snapshots: &[(PathBuf, Vec<u8>)]) {
        for (path, bytes) in snapshots {
            let actual = std::fs::read(path).expect("read target after failure");
            assert_eq!(
                &actual,
                bytes,
                "{} changed after a failed bump",
                path.display()
            );
        }
    }

    fn read(root: &Path, relative_path: &str) -> String {
        std::fs::read_to_string(root.join(relative_path)).expect("read fixture target")
    }

    fn changed_set(report: &BumpReport) -> BTreeSet<PathBuf> {
        report.changed.iter().cloned().collect()
    }

    #[test]
    fn bump_xml_updates_registry_and_both_constants() {
        let dir = fixture();
        let report = bump_bridge(dir.path(), "xml", "0.8.2").expect("xml bump succeeds");

        assert_eq!(
            read(dir.path(), REGISTRY),
            "cxf = \"0.8.4\"\njms = \"0.8.4\"\nxml = \"0.8.2\"\n"
        );
        assert!(read(dir.path(), XSLT).contains("BRIDGE_VERSION: &str = \"0.8.2\""));
        assert!(read(dir.path(), VALIDATOR).contains("BRIDGE_VERSION: &str = \"0.8.2\""));
        assert!(read(dir.path(), CXF).contains("BRIDGE_VERSION: &str = \"0.8.4\""));
        assert!(read(dir.path(), JMS).contains("BRIDGE_VERSION: &str = \"0.8.4\""));

        let outcome = lint(dir.path(), None).expect("lint fixture after bump");
        assert!(outcome.findings.is_empty(), "{:?}", outcome.findings);

        let expected: BTreeSet<PathBuf> = [REGISTRY, XSLT, VALIDATOR]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(changed_set(&report), expected);
    }

    #[test]
    fn bump_cxf_changes_only_cxf() {
        let dir = fixture();
        let before = snapshots(dir.path());

        let report = bump_bridge(dir.path(), "cxf", "9.9.9").expect("cxf bump succeeds");

        assert_eq!(
            read(dir.path(), REGISTRY),
            "cxf = \"9.9.9\"\njms = \"0.8.4\"\nxml = \"0.8.1\"\n"
        );
        assert!(read(dir.path(), CXF).contains("BRIDGE_VERSION: &str = \"9.9.9\""));
        assert_eq!(
            read(dir.path(), JMS),
            String::from_utf8(before[2].1.clone()).unwrap()
        );
        assert_eq!(
            read(dir.path(), XSLT),
            String::from_utf8(before[3].1.clone()).unwrap()
        );
        assert_eq!(
            read(dir.path(), VALIDATOR),
            String::from_utf8(before[4].1.clone()).unwrap()
        );

        let expected: BTreeSet<PathBuf> = [REGISTRY, CXF].into_iter().map(PathBuf::from).collect();
        assert_eq!(changed_set(&report), expected);
    }

    #[test]
    fn bump_constant_mismatch_fails_before_writes() {
        let dir = fixture();
        write_file(dir.path(), CXF, &constant("0.8.3"));
        let before = snapshots(dir.path());

        let result = bump_bridge(dir.path(), "cxf", "9.9.9");

        assert!(matches!(result, Err(BumpError::Snapshot(_))));
        assert_snapshots_unchanged(&before);
    }

    #[test]
    fn rollback_continues_after_restore_failure() {
        let dir = fixture();
        let before = snapshots(dir.path());
        let registry = dir.path().join(REGISTRY);
        let mut persisted = BTreeSet::new();
        let mut arranged = false;

        let result = bump_bridge_with_hook(dir.path(), "xml", "0.8.2", &mut |event| match event {
            BumpEvent::AfterPersist(relative) => {
                persisted.insert(relative.clone());
                Ok(())
            }
            BumpEvent::BeforePostLint => {
                let expected: BTreeSet<PathBuf> = [REGISTRY, XSLT, VALIDATOR]
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();
                assert_eq!(persisted, expected, "all xml targets must be persisted");

                std::fs::remove_file(&registry).expect("remove registry before rollback");
                std::fs::create_dir(&registry).expect("replace registry with a directory");
                arranged = true;
                Err("injected pre-lint failure".to_string())
            }
            _ => Ok(()),
        });

        assert!(arranged, "test hook must replace the registry path");
        let error = match result {
            Ok(report) => panic!("bump unexpectedly succeeded: {report:?}"),
            Err(error) => error,
        };
        match &error {
            BumpError::Rollback { original, rollback } => {
                assert!(
                    original.contains("hook before post-lint: injected pre-lint failure"),
                    "{original}"
                );
                assert!(rollback.contains("restore"), "{rollback}");
                assert!(
                    rollback.contains(&registry.display().to_string()),
                    "rollback must name the registry path: {rollback}"
                );
            }
            other => panic!("expected BumpError::Rollback, got {other:?}"),
        }

        for relative in [XSLT, VALIDATOR] {
            let path = dir.path().join(relative);
            let (_, expected) = before
                .iter()
                .find(|(snapshot_path, _)| snapshot_path == &path)
                .expect("xml constant snapshot");
            let actual = std::fs::read(&path).expect("read restored xml constant");
            assert_eq!(
                &actual,
                expected,
                "{} was not restored after the first restore failure",
                path.display()
            );
        }
    }

    #[test]
    fn bump_rolls_back_after_real_postlint_findings() {
        let dir = fixture();
        let live_path = "examples/xml-demo/README.md";
        write_file(
            dir.path(),
            live_path,
            "The bridge version 9.9.9 is pinned.\n",
        );
        let before = snapshots(dir.path());
        let live_before = std::fs::read(dir.path().join(live_path)).expect("read live fixture");
        let mut events = Vec::new();

        let result = bump_bridge_with_hook(dir.path(), "xml", "0.8.2", &mut |event| {
            events.push(format!("{event:?}"));
            Ok(())
        });

        let error = match result {
            Ok(report) => panic!("bump unexpectedly succeeded: {report:?}"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(
            matches!(
                &error,
                BumpError::PostWrite {
                    rolled_back: true,
                    ..
                }
            ),
            "expected a rolled-back post-write failure, got {error:?}"
        );
        assert!(message.contains("lint findings after write"), "{message}");
        assert!(
            message.contains("bridge version '9.9.9'"),
            "real live finding missing from error: {message}"
        );
        assert!(
            events.iter().any(|event| event == "BeforePostLint"),
            "post-lint hook must run: {events:?}"
        );
        assert_snapshots_unchanged(&before);
        assert_eq!(
            std::fs::read(dir.path().join(live_path)).expect("read live fixture after rollback"),
            live_before
        );
    }

    #[test]
    fn bump_rolls_back_every_byte_after_postlint_failure() {
        let dir = fixture();
        let before = snapshots(dir.path());

        let result = bump_bridge_with_hook(dir.path(), "cxf", "9.9.9", &mut |event| match event {
            BumpEvent::BeforePostLint => Err("injected post-lint failure".to_string()),
            _ => Ok(()),
        });

        assert!(matches!(
            result,
            Err(BumpError::PostWrite {
                rolled_back: true,
                ..
            })
        ));
        assert_snapshots_unchanged(&before);
    }

    #[test]
    fn bump_rolls_back_after_first_persist_failure() {
        let dir = fixture();
        let before = snapshots(dir.path());
        let mut seen_first = false;

        let result = bump_bridge_with_hook(dir.path(), "cxf", "9.9.9", &mut |event| match event {
            BumpEvent::AfterPersist(_) if !seen_first => {
                seen_first = true;
                Err("injected persist failure".to_string())
            }
            _ => Ok(()),
        });

        assert!(matches!(
            result,
            Err(BumpError::Write {
                rolled_back: true,
                ..
            })
        ));
        assert_snapshots_unchanged(&before);
    }

    #[test]
    fn bump_rolls_back_after_readback_failure() {
        let dir = fixture();
        let before = snapshots(dir.path());
        let mut seen_first = false;

        let result = bump_bridge_with_hook(dir.path(), "cxf", "9.9.9", &mut |event| match event {
            BumpEvent::AfterReadback(_) if !seen_first => {
                seen_first = true;
                Err("injected readback failure".to_string())
            }
            _ => Ok(()),
        });

        assert!(matches!(
            result,
            Err(BumpError::Write {
                rolled_back: true,
                ..
            })
        ));
        assert_snapshots_unchanged(&before);
    }

    #[test]
    fn bump_rolls_back_after_first_write_failure() {
        let dir = fixture();
        let before = snapshots(dir.path());
        let mut seen_first = false;

        let result = bump_bridge_with_hook(dir.path(), "cxf", "9.9.9", &mut |event| match event {
            BumpEvent::BeforeWrite(_) if !seen_first => {
                seen_first = true;
                Err("injected write failure".to_string())
            }
            _ => Ok(()),
        });

        assert!(matches!(
            result,
            Err(BumpError::Write {
                rolled_back: true,
                ..
            })
        ));
        assert_snapshots_unchanged(&before);
    }

    #[test]
    fn bump_invalid_bridge_writes_nothing() {
        let dir = fixture();
        let before = snapshots(dir.path());

        let result = bump_bridge(dir.path(), "http", "0.8.2");

        assert!(matches!(result, Err(BumpError::InvalidBridge(_))));
        assert_snapshots_unchanged(&before);
    }

    #[test]
    fn bump_invalid_version_writes_nothing() {
        let dir = fixture();
        let before = snapshots(dir.path());

        let result = bump_bridge(dir.path(), "xml", "not-a-version");

        assert!(matches!(result, Err(BumpError::InvalidVersion(_))));
        assert_snapshots_unchanged(&before);
    }
}
