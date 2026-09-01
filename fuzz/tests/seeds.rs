//! Committed seed corpus tests for the `dsl_yaml` fuzz target.
//!
//! The corpus lives in `seeds/dsl_yaml` next to the crate. These tests pin
//! its exact shape (the six named files, no extras), exercise every seed
//! through the harness so a regression that makes a committed seed panic
//! fails the run, and confirm the valid seed still parses.

use std::path::{Path, PathBuf};

use camel_fuzz::dsl_yaml_harness;

const SEED_DIR: &str = "seeds/dsl_yaml";

const REQUIRED_SEEDS: [&str; 6] = [
    "alias_bomb.yaml",
    "deep_nesting.yaml",
    "malformed_empty_id.yaml",
    "malformed_flow_seq.yaml",
    "malformed_unknown_step.yaml",
    "valid_minimal.yaml",
];

fn seed_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(SEED_DIR)
}

#[test]
fn required_seed_shapes_present() {
    let mut names: Vec<String> = std::fs::read_dir(seed_dir())
        .expect("seed directory must exist")
        .map(|entry| {
            entry
                .expect("seed directory entry must be readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();

    let mut required: Vec<&str> = REQUIRED_SEEDS.to_vec();
    required.sort();

    assert_eq!(
        names, required,
        "seed directory must contain exactly the six named files, no extras"
    );
}

#[test]
fn all_seeds_no_panic() {
    for entry in std::fs::read_dir(seed_dir()).expect("seed directory must exist") {
        let path = entry.expect("seed directory entry must be readable").path();
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("failed to read seed {}: {e}", path.display()));
        dsl_yaml_harness(&bytes);
    }
}

#[test]
fn valid_seed_parses_ok() {
    let content = std::fs::read_to_string(seed_dir().join("valid_minimal.yaml"))
        .expect("valid_minimal.yaml must exist");
    camel_dsl::yaml::parse_yaml(&content)
        .expect("valid_minimal.yaml must parse as a valid route document");
}
