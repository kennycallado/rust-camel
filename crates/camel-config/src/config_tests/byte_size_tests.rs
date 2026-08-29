use super::parse_byte_size;

#[test]
fn parses_plain_bytes() {
    assert_eq!(parse_byte_size("4096"), Ok(4096));
}

#[test]
fn parses_decimal_and_binary_mb() {
    assert_eq!(parse_byte_size("384MB"), Ok(384_000_000));
    assert_eq!(parse_byte_size("512MiB"), Ok(536_870_912));
}

#[test]
fn parses_case_insensitive_suffix() {
    assert_eq!(parse_byte_size("256mib"), Ok(268_435_456));
}

#[test]
fn parses_gb_and_gib() {
    assert_eq!(parse_byte_size("1GB"), Ok(1_000_000_000));
    assert_eq!(parse_byte_size("1GiB"), Ok(1_073_741_824));
}

#[test]
fn rejects_garbage() {
    let err = parse_byte_size("thirty").unwrap_err();
    assert!(err.contains("cache_repo.cache_size"), "err: {err}");
}

#[test]
fn rejects_unknown_suffix() {
    assert!(parse_byte_size("5XB").is_err());
}

#[test]
fn rejects_space_between_number_and_suffix() {
    assert!(parse_byte_size("512 MiB").is_err());
}

#[test]
fn rejects_overflow() {
    let err = parse_byte_size("18446744073709551616B").unwrap_err();
    assert!(err.contains("overflow"), "err: {err}");
}
