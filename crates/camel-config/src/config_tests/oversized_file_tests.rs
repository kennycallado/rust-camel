use super::*;
use std::io::Write;

#[test]
fn from_file_rejects_oversized_config() {
    // A single key with a very long string value — valid TOML, > 16 MiB
    let val = "a".repeat(17 * 1024 * 1024);
    let big_content = format!("x = \"{val}\"\n");
    assert!(
        big_content.len() > 16 * 1024 * 1024,
        "test content must exceed 16 MiB (was {} bytes)",
        big_content.len()
    );

    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    f.write_all(big_content.as_bytes()).expect("write");
    f.flush().expect("flush");
    let result = CamelConfig::from_file_with_profile(f.path().to_str().unwrap(), Some("default"));
    assert!(result.is_err(), "oversized config file must be rejected");
}
