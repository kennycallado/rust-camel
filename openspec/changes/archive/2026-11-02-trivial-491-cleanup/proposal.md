# trivial-491-cleanup: mail docs step name, siumai ref sweep, llm config test split

Bounded P3 cleanup batch: one docs prose fix, one five-file stale-reference sweep, one mechanical test-module extraction.

## Why

Trivial change — three low-priority tickets (rc-6juiy, rc-j712s, rc-13njc) that need no design: prose fixes and a mechanical `#[path]` test-split mirroring the existing `genai_adapter_tests.rs` pattern. No behavior change intended anywhere.

## What changes

- **rc-6juiy** — `docs/src/components/mail.md`: replace the nonexistent `removeHeaders` step with the real singular `remove_header` step (`key:`, no plural alias, no wildcard support) in the http-to-mail prose.
- **rc-j712s** — sweep stale `siumai` cross-references left by the genai adapter swap (ADR-0020 amendment) from `camel-redis/CONTEXT.md`, `camel-sql/CONTEXT.md`, `camel-platform-kubernetes/CONTEXT.md`, `camel-config/src/config_tests/parity_golden_tests.rs`, `camel-lint/src/rules/rschema.rs`; comments/prose only, now citing the `genai` SDK behind `provider/genai_adapter.rs`.
- **rc-13njc** — extract the inline `#[cfg(test)] mod tests` body of `camel-component-llm/src/config.rs` (~700 lines) into `src/config_tests.rs` via `#[cfg(test)] #[path = "config_tests.rs"] mod tests;`; mechanical move only, `cargo test -p camel-component-llm --lib` stays green.
