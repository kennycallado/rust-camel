//! Process-isolated population test for the asset registry singleton
//! (r2embed task 3.1).
//!
//! This binary deliberately contains the ONLY test that populates the
//! process-global registry: `cargo test` runs each integration-test target
//! in its own process, so the singleton state mutated here never leaks
//! into other test binaries. Emptiness assertions live only in the `--lib`
//! target (`asset_registry::asset_registry_stays_empty_outside_artifacts`);
//! global state is never reset.

use camel_bundles::asset_registry::{
    AssetRegistry, AssetRegistryError, RegistryAsset, RegistryManifestEntry,
};

fn asset(logical_path: &str, bytes: &[u8]) -> RegistryAsset {
    RegistryAsset {
        logical_path: logical_path.to_string(),
        kind: "asset".to_string(),
        bytes: bytes.to_vec(),
    }
}

fn declared(bytes: &[u8]) -> RegistryManifestEntry {
    RegistryManifestEntry {
        kind: "asset".to_string(),
        length: bytes.len() as u64,
        digest: blake3::hash(bytes).to_hex().to_string(),
    }
}

#[test]
fn asset_registry_round_trips_and_verifies_digests() {
    let registry = AssetRegistry::global();

    let cert: &[u8] = b"-----BEGIN CERTIFICATE-----\nr2embed fixture\n-----END CERTIFICATE-----\n";
    let key: &[u8] =
        b"-----BEGIN TEST PRIVATE KEY-----\nsecret bytes\n-----END TEST PRIVATE KEY-----\n";
    let page: &[u8] = b"<html><body>ok</body></html>\n";

    // Fail-closed: duplicate logical path in the store list (NOT a
    // duplicate digest — these bytes differ). Registry stays unpopulated.
    let dup_store = vec![
        asset("assets/certs/ca.pem", cert),
        asset("assets/certs/ca.pem", key),
    ];
    let pairs = vec![declared(cert), declared(key)];
    assert!(matches!(
        registry.populate(dup_store, &pairs),
        Err(AssetRegistryError::DuplicateLogicalPath { .. })
    ));
    assert!(!registry.is_populated());
    assert_eq!(registry.lookup("assets/certs/ca.pem"), None);

    // Fail-closed: store entry with no positional manifest counterpart.
    let unpaired_store = vec![
        asset("assets/certs/ca.pem", cert),
        asset("assets/web/index.html", page),
    ];
    let single = vec![declared(cert)];
    assert!(matches!(
        registry.populate(unpaired_store, &single),
        Err(AssetRegistryError::MissingManifestCounterpart { .. })
    ));
    assert!(!registry.is_populated());

    // Fail-closed: BLAKE3 digest mismatch against the manifest-declared
    // value. All-or-nothing: the registry stays unpopulated afterwards.
    let mismatch = vec![RegistryManifestEntry {
        digest: blake3::hash(b"tampered").to_hex().to_string(),
        ..declared(cert)
    }];
    let ok_store = vec![asset("assets/certs/ca.pem", cert)];
    assert!(matches!(
        registry.populate(ok_store, &mismatch),
        Err(AssetRegistryError::DigestMismatch { .. })
    ));
    assert!(!registry.is_populated());
    assert_eq!(registry.lookup("assets/certs/ca.pem"), None);

    // Valid population. The last two entries carry IDENTICAL bytes under
    // distinct logical paths — digest-duplicates are legal and both
    // populate (positional matching, duplicate = duplicate path only).
    let shared: &[u8] = b"identical payload bytes";
    let store = vec![
        asset("assets/certs/ca.pem", cert),
        asset("assets/web/index.html", page),
        asset("assets/secrets/a.key", shared),
        asset("assets/secrets/b.key", shared),
    ];
    let manifest = vec![
        declared(cert),
        declared(page),
        declared(shared),
        declared(shared),
    ];
    registry
        .populate(store.clone(), &manifest)
        .expect("valid positional pairs must populate");
    assert!(registry.is_populated());
    assert_eq!(registry.lookup("assets/certs/ca.pem"), Some(cert));
    assert_eq!(registry.lookup("assets/web/index.html"), Some(page));
    assert_eq!(registry.lookup("assets/secrets/a.key"), Some(shared));
    assert_eq!(registry.lookup("assets/secrets/b.key"), Some(shared));
    assert_eq!(registry.lookup("assets/never/embedded"), None);

    // Once-only guard: populating again is rejected even with the same
    // verified input.
    assert!(matches!(
        registry.populate(store, &manifest),
        Err(AssetRegistryError::AlreadyPopulated)
    ));
}
