//! Offline publish-registration drift gate.
//!
//! Compares the authoritative workspace publish order (produced by
//! `resolve_publish_order`) with the committed trustpub registration
//! manifest. This module owns parsing, validation, and the pure
//! comparison; those parts are I/O-free. The `run` entry point reads
//! the manifest from disk and, in `--online` mode, performs HTTP
//! observations. The command wrapper lives in `main.rs`.
//!
//! Lifecycle model (chicken-and-egg on crates.io):
//! - `registered` — maintainer assertion that trusted publishing is
//!   configured for the crate. crates.io does not expose trustpub
//!   configuration, so this is the manifest's word, nothing more.
//! - `published-unregistered` (Case A) — the crate exists on crates.io
//!   but trustpub is not registered for it: remedy is `register first`.
//! - `new-unpublished` (Case B) — the crate has never been published:
//!   the owner must perform a manual first publish with
//!   `CARGO_REGISTRY_TOKEN`, then register trustpub.
//!
//! The manifest is an ordered array of unique entries under the
//! top-level key `crates`; manifest order must match the publish order
//! exactly.
//!
//! Explicit `--online` mode observes crate-level existence for every
//! publishable crate the manifest does not assert as `registered` —
//! including publishable names with no manifest entry — and the
//! observation supersedes the manifest for classification
//! (HTTP 200 → Case A, HTTP 404 → Case B).

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Maintainer-asserted lifecycle state of one publishable crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegistrationState {
    /// Trustpub registration asserted by the manifest maintainer.
    Registered,
    /// Case A: published on crates.io, no trustpub registration.
    PublishedUnregistered,
    /// Case B: never published; manual first publish required.
    NewUnpublished,
}

/// One ordered manifest entry.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEntry {
    /// crates.io crate name; unique across the manifest.
    pub name: String,
    /// Maintainer-asserted lifecycle state.
    pub state: RegistrationState,
}

/// Shape of the committed registration manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    crates: Vec<ManifestEntry>,
}

/// Parse and validate the ordered registration manifest.
///
/// Rejects malformed TOML, malformed entries (missing or unknown
/// fields), empty crate names, duplicate crate names, and lifecycle
/// states outside the documented three, each with a named error.
pub fn parse_manifest(toml_src: &str) -> Result<Vec<ManifestEntry>, String> {
    let file: ManifestFile = toml::from_str(toml_src)
        .map_err(|e| format!("invalid publish-registration manifest: {e}"))?;

    let mut seen: HashSet<&str> = HashSet::new();
    for (idx, entry) in file.crates.iter().enumerate() {
        if entry.name.is_empty() {
            return Err(format!(
                "invalid publish-registration manifest: entry {idx} has an empty crate name"
            ));
        }
        if !seen.insert(entry.name.as_str()) {
            return Err(format!(
                "invalid publish-registration manifest: duplicate entry for crate `{}`",
                entry.name
            ));
        }
    }

    Ok(file.crates)
}

/// Kind of drift reported by [`compare_registration`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindingKind {
    /// Publishable crate absent from the manifest and not covered by an
    /// online observation: registration status unknown (fails closed).
    MissingEntry,
    /// Manifest entry that names no publishable workspace crate.
    StaleEntry,
    /// Manifest position disagrees with the authoritative publish order.
    PositionDrift,
    /// Case A: `published-unregistered` — remedy is `register first`.
    CaseA,
    /// Case B: `new-unpublished` — manual first publish, then register
    /// trustpub.
    CaseB,
}

/// Remediation text for Case A: the crate exists on crates.io but
/// trustpub is not registered for it.
fn case_a_message(name: &str) -> String {
    format!(
        "crate `{name}` is published on crates.io but is not \
         registered for trusted publishing: register first before tagging"
    )
}

/// Remediation text for Case B: the crate has never been published.
fn case_b_message(name: &str) -> String {
    format!(
        "crate `{name}` has never been published: the owner must perform \
         a manual first publish with CARGO_REGISTRY_TOKEN, then register \
         trustpub for `{name}`, then update the manifest state to `registered`"
    )
}

/// One deterministic drift diagnostic.
#[derive(Clone, Debug)]
pub struct RegistrationFinding {
    /// Crate the finding is about (for `StaleEntry`: the manifest-only
    /// name). Read by the tests; the binary prints only `message`, so
    /// outside `#[cfg(test)]` the field is never read.
    #[allow(dead_code)]
    pub crate_name: String,
    /// Machine-readable drift kind. Read by the tests; the binary
    /// prints only `message`, so outside `#[cfg(test)]` the field is
    /// never read.
    #[allow(dead_code)]
    pub kind: FindingKind,
    /// Human-readable diagnostic including the remediation text.
    pub message: String,
}

/// Compare the authoritative publish order with the manifest.
///
/// `publish_order` holds the ordered names of every publishable
/// workspace crate as returned by `resolve_publish_order`; members with
/// `publish = false` are excluded before this boundary and never appear
/// in either input. Positions are 0-based.
///
/// `observations` maps crate names to explicit online existence results;
/// offline callers pass an empty map. An observation for a publishable
/// name supersedes both the asserted manifest state and the
/// missing-entry unknown, classifying the name as Case A (`Exists`) or
/// Case B (`NotFound`). Without an observation the manifest decides, so
/// a missing entry stays unknown and fails closed.
///
/// Findings are deterministic and ordered: position drift and lifecycle
/// diagnostics are emitted walking the publish order; stale manifest
/// entries follow in manifest order.
pub fn compare_registration(
    publish_order: &[String],
    entries: &[ManifestEntry],
    observations: &HashMap<String, OnlineObservation>,
) -> Vec<RegistrationFinding> {
    let manifest_position: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(position, entry)| (entry.name.as_str(), position))
        .collect();
    let publishable: HashSet<&str> = publish_order.iter().map(String::as_str).collect();

    let mut findings = Vec::new();

    // Walk the authoritative publish order: position drift and lifecycle
    // findings are emitted in publish order.
    for (expected, name) in publish_order.iter().enumerate() {
        // Position drift needs a manifest entry; an observed name with
        // no entry has nothing to disagree with.
        let manifest_index = manifest_position.get(name.as_str()).copied();
        if let Some(actual) = manifest_index
            && actual != expected
        {
            findings.push(RegistrationFinding {
                crate_name: name.clone(),
                kind: FindingKind::PositionDrift,
                message: format!(
                    "crate `{name}` is at manifest position {actual} but the \
                     publish order expects position {expected}"
                ),
            });
        }

        // Classification, once: an explicit observation wins over the
        // manifest; without one the asserted state decides.
        let classified = if let Some(observation) = observations.get(name.as_str()) {
            Some(match observation {
                OnlineObservation::Exists => (FindingKind::CaseA, case_a_message(name)),
                OnlineObservation::NotFound => (FindingKind::CaseB, case_b_message(name)),
            })
        } else {
            match manifest_index.map(|actual| entries[actual].state) {
                Some(RegistrationState::PublishedUnregistered) => {
                    Some((FindingKind::CaseA, case_a_message(name)))
                }
                Some(RegistrationState::NewUnpublished) => {
                    Some((FindingKind::CaseB, case_b_message(name)))
                }
                Some(RegistrationState::Registered) | None => None,
            }
        };

        match classified {
            Some((kind, message)) => findings.push(RegistrationFinding {
                crate_name: name.clone(),
                kind,
                message,
            }),
            // No observation and no entry: the registration status is
            // unknown, so the gate fails closed (offline behavior, and
            // the honest answer for any unobserved missing entry).
            None if manifest_index.is_none() => findings.push(RegistrationFinding {
                crate_name: name.clone(),
                kind: FindingKind::MissingEntry,
                message: format!(
                    "crate `{name}` is publishable but has no manifest entry; \
                     registration status is unknown - add an entry with state \
                     `published-unregistered` (Case A) or `new-unpublished` (Case B)"
                ),
            }),
            // Asserted `registered`: clean, nothing to report.
            None => {}
        }
    }

    // Stale manifest entries: names that match no publishable workspace
    // crate, reported in manifest order.
    for entry in entries {
        if !publishable.contains(entry.name.as_str()) {
            findings.push(RegistrationFinding {
                crate_name: entry.name.clone(),
                kind: FindingKind::StaleEntry,
                message: format!(
                    "manifest entry `{}` does not match any publishable workspace \
                     crate (publish = false or unknown name) - remove it",
                    entry.name
                ),
            });
        }
    }

    findings
}

// ----- command API (Task 1.2) -----

/// Fixed crates.io API base URL used for the explicit online existence
/// observation. Overridable with `PUBLISH_REGISTRATION_BASE_URL` for
/// tests; the URL never carries credentials.
pub const CRATES_IO_API_BASE_URL: &str = "https://crates.io/api/v1";

/// Workspace-root-relative location of the committed registration
/// manifest.
pub const MANIFEST_RELATIVE_PATH: &str = "scripts/xtask/trustpub-registrations.toml";

/// Single-request budget for the online existence observation.
const OBSERVATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Environment variable overriding the crates.io API base URL (tests).
const BASE_URL_ENV: &str = "PUBLISH_REGISTRATION_BASE_URL";

/// What the explicit online observation concluded about one crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnlineObservation {
    /// HTTP 200: the crate exists on crates.io (Case A for an
    /// unregistered name).
    Exists,
    /// HTTP 404: the crate has never been published (Case B).
    NotFound,
}

/// Result of one command execution.
#[derive(Debug)]
pub struct RunOutcome {
    /// Deterministic drift diagnostics; non-empty means gate failure.
    pub findings: Vec<RegistrationFinding>,
    /// Crate names queried on crates.io in query order (offline: empty).
    /// Read by the command-level tests (offline zero-query proof and
    /// online query-order determinism); the binary itself never prints
    /// it, so the field is dead outside `#[cfg(test)]`.
    #[allow(dead_code)]
    pub online_queries: Vec<String>,
}

/// Resolve the observation base URL: `PUBLISH_REGISTRATION_BASE_URL`
/// when set, the fixed crates.io API base URL otherwise.
pub fn base_url_from_env() -> String {
    std::env::var(BASE_URL_ENV).unwrap_or_else(|_| CRATES_IO_API_BASE_URL.to_string())
}

/// Run the publish-registration gate.
///
/// Offline (`online = false`) the command reads only workspace metadata
/// and the committed manifest — no network client is constructed and no
/// request is made. Online mode adds an explicit crate-level existence
/// query to crates.io for every publishable crate the manifest does not
/// assert as `registered`: non-`registered` entries and publishable
/// names with no manifest entry alike. Classification happens once in
/// [`compare_registration`], where the observation supersedes the
/// asserted (or missing) state. Any status other than 200/404, a
/// transport error, or a timeout fails closed. Credentials are never
/// read or serialized.
pub fn run(workspace_root: &Path, online: bool, base_url: &str) -> Result<RunOutcome, String> {
    run_with_timeout(workspace_root, online, base_url, OBSERVATION_TIMEOUT)
}

/// `run` with an explicit observation timeout (test seam).
fn run_with_timeout(
    workspace_root: &Path,
    online: bool,
    base_url: &str,
    timeout: std::time::Duration,
) -> Result<RunOutcome, String> {
    let manifest_path = workspace_root.join(MANIFEST_RELATIVE_PATH);
    let toml_src = std::fs::read_to_string(&manifest_path).map_err(|e| {
        format!(
            "cannot read publish-registration manifest at {}: {e}",
            manifest_path.display()
        )
    })?;
    let entries = parse_manifest(&toml_src)?;

    // The resolver excludes `publish = false` members before this
    // boundary, so the publish order carries only publishable names.
    let (sorted, _, _) = crate::resolve_publish_order(workspace_root)?;
    let publish_order: Vec<String> = sorted.into_iter().map(|c| c.name).collect();

    let mut online_queries = Vec::new();
    let mut observations: HashMap<String, OnlineObservation> = HashMap::new();

    if online {
        let agent = observation_agent(timeout);
        let asserted: HashMap<&str, RegistrationState> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.state))
            .collect();

        // Observe every publishable crate the manifest does not assert
        // as `registered`: non-`registered` entries and publishable
        // names with no entry at all. Queries run in publish order.
        // Fails closed on any non-200/404 response, transport error, or
        // timeout.
        for name in &publish_order {
            let needs_observation = match asserted.get(name.as_str()) {
                Some(&state) => state != RegistrationState::Registered,
                None => true,
            };
            if !needs_observation {
                continue;
            }
            let observation = observe_crate_exists(&agent, base_url, name)?;
            online_queries.push(name.clone());
            observations.insert(name.clone(), observation);
        }
    }

    // Classify once: observations (online) and manifest assertions
    // (offline) feed the same comparison, so no post-hoc finding
    // replacement is needed.
    let findings = compare_registration(&publish_order, &entries, &observations);

    Ok(RunOutcome {
        findings,
        online_queries,
    })
}

/// Build the HTTP agent used for online observations. Constructed only
/// in online mode; the offline path never touches the network.
fn observation_agent(timeout: std::time::Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .new_agent()
}

/// Explicit crate-level existence observation:
/// `GET {base_url}/crates/{name}`. HTTP 200 resolves to
/// [`OnlineObservation::Exists`], HTTP 404 to
/// [`OnlineObservation::NotFound`]; every other status, transport
/// error, or timeout fails closed. Never reads or sends credentials.
fn observe_crate_exists(
    agent: &ureq::Agent,
    base_url: &str,
    name: &str,
) -> Result<OnlineObservation, String> {
    let url = format!("{base_url}/crates/{name}");
    match agent.get(&url).call() {
        Ok(response) if response.status().as_u16() == 200 => Ok(OnlineObservation::Exists),
        Ok(response) => Err(format!(
            "crates.io observation failed for `{name}`: unexpected HTTP \
             status {} - failing closed",
            response.status().as_u16()
        )),
        Err(ureq::Error::StatusCode(404)) => Ok(OnlineObservation::NotFound),
        Err(ureq::Error::StatusCode(code)) => Err(format!(
            "crates.io observation failed for `{name}`: unexpected HTTP \
             status {code} - failing closed"
        )),
        Err(e) => Err(format!(
            "crates.io observation failed for `{name}`: {e} - failing closed"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries_from(pairs: &[(&str, RegistrationState)]) -> Vec<ManifestEntry> {
        pairs
            .iter()
            .map(|(name, state)| ManifestEntry {
                name: (*name).to_string(),
                state: *state,
            })
            .collect()
    }

    fn names_of(findings: &[RegistrationFinding]) -> Vec<&str> {
        findings.iter().map(|f| f.crate_name.as_str()).collect()
    }

    fn kinds_of(findings: &[RegistrationFinding]) -> Vec<FindingKind> {
        findings.iter().map(|f| f.kind).collect()
    }

    /// Full current-fleet shape: 66 unique ordered publishable names,
    /// every entry `registered`, manifest order identical to the publish
    /// order — the clean reconciliation must produce no findings.
    #[test]
    fn registered_manifest_matches_publish_order() {
        // arrange: 66 unique ordered publishable names, all registered.
        let publish_order: Vec<String> = (0..66).map(|i| format!("camel-gate-{i:02}")).collect();
        let entries: Vec<ManifestEntry> = publish_order
            .iter()
            .map(|name| ManifestEntry {
                name: name.clone(),
                state: RegistrationState::Registered,
            })
            .collect();

        // act: empty observation map = offline classification.
        let findings = compare_registration(&publish_order, &entries, &HashMap::new());

        // assert
        assert!(findings.is_empty(), "unexpected findings: {findings:?}");
    }

    /// `publish = false` members are excluded by the resolver before the
    /// comparison boundary: the publish order carries only publishable
    /// names, so a manifest entry naming a `publish = false` crate is
    /// stale rather than silently accepted.
    #[test]
    fn publish_false_exclusion_at_comparison_boundary() {
        // arrange: camel-gate-99 is `publish = false` and filtered out by
        // resolve_publish_order, so it never appears in the publish order;
        // the manifest still lists it.
        let publish_order = vec!["camel-a".to_string(), "camel-b".to_string()];
        let entries = entries_from(&[
            ("camel-a", RegistrationState::Registered),
            ("camel-b", RegistrationState::Registered),
            ("camel-gate-99", RegistrationState::Registered),
        ]);

        // act: empty observation map = offline classification.
        let findings = compare_registration(&publish_order, &entries, &HashMap::new());

        // assert: only the publish = false name is flagged, as stale.
        assert_eq!(names_of(&findings), vec!["camel-gate-99"]);
        assert_eq!(kinds_of(&findings), vec![FindingKind::StaleEntry]);
        assert!(
            findings[0].message.contains("publish = false"),
            "{}",
            findings[0].message
        );
    }

    /// Two Case A and two Case B entries interleaved in publish order
    /// surface as findings in that same publish order, each carrying the
    /// exact remediation text for its case.
    #[test]
    fn mixed_case_findings_are_publish_ordered() {
        // arrange: Case A and Case B entries interleaved across the set.
        let publish_order: Vec<String> = [
            "camel-a", "camel-b", "camel-c", "camel-d", "camel-e", "camel-f",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let entries = entries_from(&[
            ("camel-a", RegistrationState::Registered),
            ("camel-b", RegistrationState::PublishedUnregistered),
            ("camel-c", RegistrationState::NewUnpublished),
            ("camel-d", RegistrationState::Registered),
            ("camel-e", RegistrationState::PublishedUnregistered),
            ("camel-f", RegistrationState::NewUnpublished),
        ]);

        // act: empty observation map = offline classification.
        let findings = compare_registration(&publish_order, &entries, &HashMap::new());

        // assert: output order matches publish order.
        assert_eq!(
            names_of(&findings),
            vec!["camel-b", "camel-c", "camel-e", "camel-f"]
        );
        assert_eq!(
            kinds_of(&findings),
            vec![
                FindingKind::CaseA,
                FindingKind::CaseB,
                FindingKind::CaseA,
                FindingKind::CaseB,
            ]
        );
        for finding in &findings {
            assert!(finding.message.contains(&finding.crate_name));
            match finding.kind {
                FindingKind::CaseA => {
                    assert!(
                        finding.message.contains("register first"),
                        "{}",
                        finding.message
                    );
                }
                FindingKind::CaseB => {
                    assert!(
                        finding
                            .message
                            .contains("manual first publish with CARGO_REGISTRY_TOKEN"),
                        "{}",
                        finding.message
                    );
                    assert!(
                        finding.message.contains("register trustpub"),
                        "{}",
                        finding.message
                    );
                }
                other => panic!("unexpected kind in mixed-case test: {other:?}"),
            }
        }
    }

    /// A publishable crate with no manifest entry is missing (unknown
    /// registration status); a manifest name that matches no publishable
    /// crate is stale.
    #[test]
    fn missing_and_stale_entries_are_reported() {
        // arrange: publish order a,b,c — manifest lists a, stale x, c
        // (b has no entry).
        let publish_order: Vec<String> = ["camel-a", "camel-b", "camel-c"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let entries = entries_from(&[
            ("camel-a", RegistrationState::Registered),
            ("camel-x", RegistrationState::Registered),
            ("camel-c", RegistrationState::Registered),
        ]);

        // act: empty observation map = offline classification.
        let findings = compare_registration(&publish_order, &entries, &HashMap::new());

        // assert
        assert_eq!(names_of(&findings), vec!["camel-b", "camel-x"]);
        assert_eq!(
            kinds_of(&findings),
            vec![FindingKind::MissingEntry, FindingKind::StaleEntry]
        );
        assert!(
            findings[0]
                .message
                .contains("registration status is unknown"),
            "{}",
            findings[0].message
        );
    }

    /// An online observation for a publishable name with no manifest
    /// entry supersedes the missing-entry unknown: HTTP 200 classifies
    /// it as Case A (`register first`) and no `MissingEntry` finding is
    /// emitted; a position check cannot apply because there is no entry.
    #[test]
    fn observation_supersedes_missing_entry() {
        // arrange: publish order a,b — manifest lists only a; an online
        // 200 observation covers the missing b.
        let publish_order: Vec<String> = ["camel-a", "camel-b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let entries = entries_from(&[("camel-a", RegistrationState::Registered)]);
        let mut observations = HashMap::new();
        observations.insert("camel-b".to_string(), OnlineObservation::Exists);

        // act
        let findings = compare_registration(&publish_order, &entries, &observations);

        // assert: the missing entry is classified, not left unknown.
        assert_eq!(names_of(&findings), vec!["camel-b"]);
        assert_eq!(kinds_of(&findings), vec![FindingKind::CaseA]);
        assert!(
            findings[0].message.contains("register first"),
            "{}",
            findings[0].message
        );

        // Without the observation the same shape fails closed offline.
        let findings = compare_registration(&publish_order, &entries, &HashMap::new());
        assert_eq!(kinds_of(&findings), vec![FindingKind::MissingEntry]);
    }

    /// Malformed TOML, malformed entries, duplicate names, and unknown
    /// lifecycle states are each rejected with a named validation error.
    #[test]
    fn manifest_validation_rejects_invalid_input() {
        // malformed TOML
        let err = parse_manifest("crates = [").unwrap_err();
        assert!(
            err.contains("invalid publish-registration manifest"),
            "{err}"
        );

        // malformed entry: missing `state` field
        let err = parse_manifest("[[crates]]\nname = \"camel-a\"\n").unwrap_err();
        assert!(err.contains("missing field `state`"), "{err}");

        // empty crate name
        let err = parse_manifest("[[crates]]\nname = \"\"\nstate = \"registered\"\n").unwrap_err();
        assert!(err.contains("empty crate name"), "{err}");

        // duplicate crate names
        let err = parse_manifest(
            "[[crates]]\nname = \"camel-dup\"\nstate = \"registered\"\n\n[[crates]]\nname = \"camel-dup\"\nstate = \"registered\"\n",
        )
        .unwrap_err();
        assert!(
            err.contains("duplicate entry") && err.contains("camel-dup"),
            "{err}"
        );

        // unknown lifecycle state
        let err =
            parse_manifest("[[crates]]\nname = \"camel-a\"\nstate = \"retired\"\n").unwrap_err();
        assert!(
            err.contains("unknown variant") && err.contains("retired"),
            "{err}"
        );
    }

    /// Same crate set in different positions: every displaced crate is
    /// reported with its expected (publish-order) and actual (manifest)
    /// 0-based positions.
    #[test]
    fn order_drift_is_reported() {
        // arrange: identical name sets, b and c swapped in the manifest.
        let publish_order: Vec<String> = ["camel-a", "camel-b", "camel-c"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let entries = entries_from(&[
            ("camel-a", RegistrationState::Registered),
            ("camel-c", RegistrationState::Registered),
            ("camel-b", RegistrationState::Registered),
        ]);

        // act: empty observation map = offline classification.
        let findings = compare_registration(&publish_order, &entries, &HashMap::new());

        // assert
        assert_eq!(names_of(&findings), vec!["camel-b", "camel-c"]);
        assert_eq!(
            kinds_of(&findings),
            vec![FindingKind::PositionDrift, FindingKind::PositionDrift]
        );
        assert!(
            findings[0].message.contains("manifest position 2")
                && findings[0].message.contains("expects position 1"),
            "{}",
            findings[0].message
        );
        assert!(
            findings[1].message.contains("manifest position 1")
                && findings[1].message.contains("expects position 2"),
            "{}",
            findings[1].message
        );
    }

    // ----- command-level tests (Task 1.2) -----

    /// Minimal scriptable HTTP server standing in for the crates.io API.
    struct MockRegistry {
        base_url: String,
        connections: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    /// Behaviour programmed into a [`MockRegistry`].
    enum MockScript {
        /// Respond with this status to every request.
        Status(u16),
        /// Respond per crate name taken from the `/crates/<name>` path.
        PerPath(std::collections::HashMap<String, u16>),
        /// Accept the connection and never respond (forces client timeout).
        Hang,
    }

    impl MockRegistry {
        fn spawn(script: MockScript) -> Self {
            use std::io::{Read, Write};
            use std::net::TcpListener;

            let listener = TcpListener::bind("127.0.0.1:0").unwrap(); // allow-unwrap
            let addr = listener.local_addr().unwrap(); // allow-unwrap
            let connections = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counter = std::sync::Arc::clone(&connections);

            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let mut stream = stream;
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                    // Drain the request head; only the path matters here.
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 512];
                    loop {
                        match stream.read(&mut chunk) {
                            Ok(n) if n > 0 => {
                                buf.extend_from_slice(&chunk[..n]);
                                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024
                                {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                    let head = String::from_utf8_lossy(&buf);
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();

                    let respond = |code: u16| -> Vec<u8> {
                        let reason = match code {
                            200 => "OK",
                            404 => "Not Found",
                            _ => "Internal Server Error",
                        };
                        format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .into_bytes()
                    };

                    match &script {
                        MockScript::Status(code) => {
                            let _ = stream.write_all(&respond(*code));
                        }
                        MockScript::PerPath(map) => {
                            let name = path.rsplit('/').next().unwrap_or("");
                            let code = map.get(name).copied().unwrap_or(404);
                            let _ = stream.write_all(&respond(code));
                        }
                        MockScript::Hang => {
                            // Hold the connection open without responding so
                            // the client's timeout fires.
                            std::thread::sleep(std::time::Duration::from_secs(30));
                        }
                    }
                }
            });

            Self {
                base_url: format!("http://{addr}"),
                connections,
            }
        }
    }

    /// Write one fake publishable workspace crate under `<root>/crates/`.
    fn write_fake_crate(root: &Path, name: &str, normal_deps: &[&str]) {
        let dir = root.join("crates").join(name);
        std::fs::create_dir_all(&dir).unwrap(); // allow-unwrap
        let mut toml = format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n");
        if !normal_deps.is_empty() {
            toml.push_str("\n[dependencies]\n");
            for dep in normal_deps {
                toml.push_str(&format!("{dep} = \"0.1\"\n"));
            }
        }
        std::fs::write(dir.join("Cargo.toml"), toml).unwrap(); // allow-unwrap
    }

    /// Write the registration manifest at its workspace-root-relative
    /// location (`scripts/xtask/trustpub-registrations.toml`).
    fn write_manifest(root: &Path, entries: &[(&str, &str)]) {
        let dir = root.join("scripts").join("xtask");
        std::fs::create_dir_all(&dir).unwrap(); // allow-unwrap
        let mut toml = String::new();
        for (name, state) in entries {
            toml.push_str(&format!(
                "[[crates]]\nname = \"{name}\"\nstate = \"{state}\"\n\n"
            ));
        }
        std::fs::write(dir.join("trustpub-registrations.toml"), toml).unwrap(); // allow-unwrap
    }

    /// Default (offline) execution must complete the comparison without
    /// constructing a network client or issuing a single request.
    #[test]
    fn offline_command_does_not_query_network() {
        // arrange: a valid manifest and a registry that would fail the
        // run if it were ever contacted.
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_fake_crate(root, "camel-reg-a", &[]);
        write_fake_crate(root, "camel-reg-b", &["camel-reg-a"]);
        write_manifest(
            root,
            &[("camel-reg-a", "registered"), ("camel-reg-b", "registered")],
        );
        let registry = MockRegistry::spawn(MockScript::Status(500));

        // act: run WITHOUT `--online`, pointing the (unused) observation
        // URL at the mock server.
        let outcome = run(root, false, &registry.base_url).unwrap(); // allow-unwrap

        // assert: comparison completed; the server saw zero connections.
        assert!(outcome.findings.is_empty(), "{:?}", outcome.findings);
        assert!(outcome.online_queries.is_empty());
        assert_eq!(
            registry
                .connections
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "offline run must not touch the network"
        );
    }

    /// With `--online`, HTTP 200 classifies an unregistered name as
    /// Case A (`register first`) and HTTP 404 as Case B (manual first
    /// publish); findings remain non-empty, so the gate exits non-zero.
    #[test]
    fn online_statuses_classify_cases() {
        // arrange: Case A and Case B entries with scripted responses.
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_fake_crate(root, "camel-reg-a", &[]);
        write_fake_crate(root, "camel-reg-b", &["camel-reg-a"]);
        write_manifest(
            root,
            &[
                ("camel-reg-a", "published-unregistered"),
                ("camel-reg-b", "new-unpublished"),
            ],
        );
        let mut script = std::collections::HashMap::new();
        script.insert("camel-reg-a".to_string(), 200u16);
        script.insert("camel-reg-b".to_string(), 404u16);
        let registry = MockRegistry::spawn(MockScript::PerPath(script));

        // act
        let outcome = run(root, true, &registry.base_url).unwrap(); // allow-unwrap

        // assert
        assert_eq!(outcome.online_queries, vec!["camel-reg-a", "camel-reg-b"]);
        let kinds: Vec<FindingKind> = outcome.findings.iter().map(|f| f.kind).collect();
        assert_eq!(kinds, vec![FindingKind::CaseA, FindingKind::CaseB]);
        assert!(
            outcome.findings[0].message.contains("register first"),
            "{}",
            outcome.findings[0].message
        );
        assert!(
            outcome.findings[1]
                .message
                .contains("manual first publish with CARGO_REGISTRY_TOKEN"),
            "{}",
            outcome.findings[1].message
        );
        assert!(!outcome.findings.is_empty(), "gate must exit non-zero");
    }

    /// Any status other than 200/404 and any transport-level timeout
    /// fail closed: the command returns an error naming the observation
    /// failure, and no success result is produced.
    #[test]
    fn online_unexpected_status_fails_closed() {
        // arrange (a): a registry answering HTTP 500.
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_fake_crate(root, "camel-reg-a", &[]);
        write_manifest(root, &[("camel-reg-a", "published-unregistered")]);
        let registry = MockRegistry::spawn(MockScript::Status(500));

        // act (a)
        let err = run(root, true, &registry.base_url).unwrap_err(); // allow-unwrap

        // assert (a)
        assert!(err.contains("camel-reg-a"), "{err}");
        assert!(err.contains("observation"), "{err}");
        assert!(err.contains("failing closed"), "{err}");

        // arrange (b): a registry that accepts and never responds.
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_fake_crate(root, "camel-reg-a", &[]);
        write_manifest(root, &[("camel-reg-a", "published-unregistered")]);
        let registry = MockRegistry::spawn(MockScript::Hang);

        // act (b): short observation budget so the test stays fast.
        let outcome = run_with_timeout(
            root,
            true,
            &registry.base_url,
            std::time::Duration::from_millis(500),
        );

        // assert (b)
        let err = outcome.unwrap_err(); // allow-unwrap
        assert!(err.contains("camel-reg-a"), "{err}");
        assert!(err.contains("failing closed"), "{err}");
    }

    /// An online 200 for a crate the manifest still calls
    /// `new-unpublished` supersedes the stale assertion: the reported
    /// classification is Case A (`register first`), not Case B.
    #[test]
    fn online_corrects_stale_new_unpublished() {
        // arrange: stale `new-unpublished` entry plus a 200 response.
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_fake_crate(root, "camel-reg-a", &[]);
        write_manifest(root, &[("camel-reg-a", "new-unpublished")]);
        let registry = MockRegistry::spawn(MockScript::Status(200));

        // act
        let outcome = run(root, true, &registry.base_url).unwrap(); // allow-unwrap

        // assert: Case A output replaces the stale state.
        assert_eq!(outcome.findings.len(), 1, "{:?}", outcome.findings);
        assert_eq!(outcome.findings[0].kind, FindingKind::CaseA);
        assert!(
            outcome.findings[0].message.contains("register first"),
            "{}",
            outcome.findings[0].message
        );
    }

    /// A publishable crate with no manifest entry is observed online
    /// like any other unregistered name: HTTP 200 classifies it as
    /// Case A (`register first`) instead of the offline unknown-status
    /// failure. Exact spec scenario for online missing-entry behavior.
    #[test]
    fn online_missing_manifest_entry_http_200_is_case_a() {
        // arrange: camel-reg-b is publishable but has no manifest entry;
        // crates.io answers 200 for it.
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_fake_crate(root, "camel-reg-a", &[]);
        write_fake_crate(root, "camel-reg-b", &["camel-reg-a"]);
        write_manifest(root, &[("camel-reg-a", "registered")]);
        let registry = MockRegistry::spawn(MockScript::Status(200));

        // act
        let outcome = run(root, true, &registry.base_url).unwrap(); // allow-unwrap

        // assert: the missing name is queried and classified as Case A.
        assert_eq!(outcome.online_queries, vec!["camel-reg-b"]);
        assert_eq!(outcome.findings.len(), 1, "{:?}", outcome.findings);
        assert_eq!(outcome.findings[0].crate_name, "camel-reg-b");
        assert_eq!(outcome.findings[0].kind, FindingKind::CaseA);
        assert!(
            outcome.findings[0].message.contains("register first"),
            "{}",
            outcome.findings[0].message
        );
    }

    /// A publishable crate with no manifest entry that gets HTTP 404
    /// online is classified as Case B (manual first publish), keeping
    /// the gate non-zero.
    #[test]
    fn online_missing_manifest_entry_http_404_is_case_b() {
        // arrange: camel-reg-b is publishable but has no manifest entry;
        // crates.io answers 404 for it.
        let dir = tempfile::tempdir().unwrap(); // allow-unwrap
        let root = dir.path();
        write_fake_crate(root, "camel-reg-a", &[]);
        write_fake_crate(root, "camel-reg-b", &["camel-reg-a"]);
        write_manifest(root, &[("camel-reg-a", "registered")]);
        let registry = MockRegistry::spawn(MockScript::Status(404));

        // act
        let outcome = run(root, true, &registry.base_url).unwrap(); // allow-unwrap

        // assert: the missing name is queried and classified as Case B.
        assert_eq!(outcome.online_queries, vec!["camel-reg-b"]);
        assert_eq!(outcome.findings.len(), 1, "{:?}", outcome.findings);
        assert_eq!(outcome.findings[0].crate_name, "camel-reg-b");
        assert_eq!(outcome.findings[0].kind, FindingKind::CaseB);
        assert!(
            outcome.findings[0]
                .message
                .contains("manual first publish with CARGO_REGISTRY_TOKEN"),
            "{}",
            outcome.findings[0].message
        );
    }
}
