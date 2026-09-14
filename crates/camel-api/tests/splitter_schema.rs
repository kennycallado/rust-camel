//! Schema contract tests for the stream split format enum (Task 1.1).
//!
//! Verifies the public spelling of the TAR archive formats (`tar` / `tar.gz`)
//! across JSON serialization, the generated JSON schema, and the generated
//! TypeScript bindings.

use camel_api::{StreamSplitConfig, StreamSplitFormat};

/// Checked-in canonical route spec JSON schema (relative to the crate dir).
const CANONICAL_SCHEMA_PATH: &str = "../../schemas/canonical-route-spec.json";
/// Checked-in TypeScript binding for `StreamSplitFormat`.
const STREAM_SPLIT_FORMAT_TS_PATH: &str = "../../schemas/ts/StreamSplitFormat.ts";

#[test]
fn stream_split_format_serializes_tar_and_tar_gz() {
    // A StreamSplitConfig carrying each new variant serializes the exact
    // public spelling.
    let tar_cfg = StreamSplitConfig {
        format: StreamSplitFormat::Tar,
        ..Default::default()
    };
    let tar_gz_cfg = StreamSplitConfig {
        format: StreamSplitFormat::TarGz,
        ..Default::default()
    };
    assert_eq!(
        serde_json::to_value(&tar_cfg).unwrap()["format"],
        serde_json::json!("tar")
    );
    assert_eq!(
        serde_json::to_value(&tar_gz_cfg).unwrap()["format"],
        serde_json::json!("tar.gz")
    );

    // The enum values themselves serialize to the exact JSON strings.
    assert_eq!(
        serde_json::to_string(&StreamSplitFormat::Tar).unwrap(),
        "\"tar\""
    );
    assert_eq!(
        serde_json::to_string(&StreamSplitFormat::TarGz).unwrap(),
        "\"tar.gz\""
    );

    // Round-trip: the public spellings deserialize back to the variants.
    assert_eq!(
        serde_json::from_str::<StreamSplitFormat>("\"tar\"").unwrap(),
        StreamSplitFormat::Tar
    );
    assert_eq!(
        serde_json::from_str::<StreamSplitFormat>("\"tar.gz\"").unwrap(),
        StreamSplitFormat::TarGz
    );
}

#[test]
fn stream_split_schema_lists_archive_formats() {
    let schema_text = std::fs::read_to_string(CANONICAL_SCHEMA_PATH)
        .expect("checked-in canonical-route-spec.json must exist");
    let schema: serde_json::Value =
        serde_json::from_str(&schema_text).expect("canonical schema must be valid JSON");

    let format_def = &schema["$defs"]["StreamSplitFormat"];
    let consts: Vec<&str> = format_def["oneOf"]
        .as_array()
        .expect("StreamSplitFormat must be a oneOf enum")
        .iter()
        .filter_map(|v| v["const"].as_str())
        .collect();

    for expected in ["tar", "tar.gz", "zip"] {
        assert!(
            consts.contains(&expected),
            "StreamSplitFormat schema must list '{expected}', got {consts:?}"
        );
    }
}

#[test]
fn stream_split_config_rejects_chunk_size_for_tar_and_tar_gz() {
    // Tar and TarGz are materialized archive formats: entries are whole
    // units, so chunk_size is meaningless and must be rejected like Zip.
    for format in [StreamSplitFormat::Tar, StreamSplitFormat::TarGz] {
        let config = StreamSplitConfig {
            format,
            chunk_size: Some(1024),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("materialized archive format"),
            "Tar/TarGz chunk_size error must name the materialized format, got: {msg}"
        );
        assert!(
            msg.contains("does not support chunk_size"),
            "Tar/TarGz chunk_size error must reject chunk_size, got: {msg}"
        );
    }
}

#[test]
fn stream_split_typescript_uses_tar_gz_literal() {
    let ts = std::fs::read_to_string(STREAM_SPLIT_FORMAT_TS_PATH)
        .expect("checked-in StreamSplitFormat.ts must exist");
    assert!(
        ts.contains("\"tar.gz\""),
        "StreamSplitFormat.ts must contain the exact literal \"tar.gz\""
    );
    assert!(
        !ts.contains("tar_gz"),
        "StreamSplitFormat.ts must not contain the snake_case spelling 'tar_gz'"
    );
}
