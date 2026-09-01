use camel_dsl::SecurityCompileContext;

/// Feed arbitrary bytes as UTF-8 text to camel-dsl YAML route parsing.
///
/// Invalid UTF-8 input is skipped. Parsing must never panic: the parse call
/// returns either `Ok` or `Err`, and the result is discarded.
pub fn dsl_yaml_harness(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = camel_dsl::yaml::parse_yaml_with_threshold_and_security(
            s,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        );
    }
}
