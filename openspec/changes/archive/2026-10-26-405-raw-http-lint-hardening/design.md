# Design: 405-raw-http-lint-hardening

The scanner walks every configured member root and inspects string/byte-string
literals; the anchoring change narrows matches to a line start so a literal that
merely contains an `HTTP/1.1` sequence is not reported. The harness allowlist grows
by one constructor used by the migration, and the root walk fails closed when a
root is absent so a renamed tree cannot silently reduce coverage. Implementation:
`scripts/xtask/src/lint_no_raw_http_test_server.rs`; tests in the same crate and
`crates/camel-component-api/src/test_support/http.rs`.
