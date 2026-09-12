# Tasks: httpsweep

## camel-component-http

### Task 1.1: Add `constructed_header` helper with drop-record unit tests

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Below `select_outbound_headers` (currently ~line 3534-3600) and above the
   `#[cfg(test)] mod tests` block, add a private helper:
   `fn constructed_header<'a>(name: &'a str, value: &str) ->
   Result<(reqwest::header::HeaderName, reqwest::header::HeaderValue),
   OutboundHeaderDrop<'a>>`.
   Body: try `reqwest::header::HeaderName::from_bytes(name.as_bytes())`;
   on `Err` return `Err(OutboundHeaderDrop { name, reason: "invalid header
   name", value_kind: None })`; then try
   `reqwest::header::HeaderValue::from_str(value)`; on `Err` return
   `Err(OutboundHeaderDrop { name, reason: "invalid header value",
   value_kind: None })`; on success return `Ok((name, val))`. Reason strings
   MUST be byte-identical to `select_outbound_headers`'s so log greps stay
   uniform.
2. In the same `#[cfg(test)] mod tests` module that hosts the
   `select_outbound_headers` tests, add the three unit tests listed under
   Tests, following that module's existing style (plain `#[test]` fns, no
   runtime needed).
3. Match/assert on the `Result` — no `unwrap`/`expect` anywhere in the new
   code (lint-unwrap).
4. Surgical edit only: no reformat of surrounding lines (`cargo fmt --check`
   must show no diff beyond your addition after `cargo fmt` on the file).
5. Give the helper a short doc comment citing bd rc-jbs1v and ADR-0051
   (drop records carry name + reason, never values), matching the sibling
   style around `select_outbound_headers`.

**Tests:** (executable spec)
- `constructed_header_invalid_value_returns_drop_record`:
  setup: helper exists. action: `constructed_header("user-agent",
  "bad\r\ns3nt1nel")`. assert: `Err(record)` where `record.reason ==
  "invalid header value"`, `record.name == "user-agent"`,
  `record.value_kind.is_none()`; `format!("{record:?}")` contains neither
  of the exact absent substrings `"bad\r\n"` nor `"s3nt1nel"` (the reason
  string itself legitimately contains the word `value` — the sentinel is
  what must be absent).
- `constructed_header_invalid_name_returns_drop_record`:
  action: `constructed_header("bad name", "ok")`. assert: `Err(record)`
  with `record.reason == "invalid header name"`, `record.name == "bad
  name"`; debug format contains no `"ok"` value fragment.
- `constructed_header_valid_pair_roundtrip`:
  action: `constructed_header("authorization", "Bearer abc123")`. assert:
  `Ok((name, val))` with `name.as_str() == "authorization"` and
  `val.to_str() == Ok("Bearer abc123")`.
- command: `cargo test -p camel-component-http constructed_header`
- expected: the three tests FAIL to compile before the helper exists
  (unresolved name), pass after Step 1-2.

**Acceptance:**
- `cargo test -p camel-component-http constructed_header` exits 0 (3 tests).
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- `cargo xtask lint-unwrap` exits 0 (no new unwrap/expect).
- Helper signature matches design.md Decision 1 exactly (lifetime `'a` on
  name and `OutboundHeaderDrop<'a>`).

- [x] 1.1

### Task 1.2: Route the four producer injection sites through the helper; wire test

Start ONLY after Task 1.1 is checked off (this task calls
`constructed_header`, which Task 1.1 introduces).

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. In `HttpProducer::call` (producer block ~3005-3130), replace the
   user-agent `if let Ok(val) = HeaderValue::from_str(user_agent)` guard:
   call `constructed_header("user-agent", user_agent)`; on `Ok((_, val))`
   push `(reqwest::header::USER_AGENT, val)` onto `collected_headers`; on
   `Err(drop)` emit the landed debug! shape (copy the no-`value_kind` arm
   used for `outbound.drops` ~3056-3073): `debug!(correlation_id =
   %exchange.correlation_id(), header = %drop.name, "outbound header
   dropped: {}", drop.reason)`. Keep the outer `if let Some(user_agent)`
   and `!config.bridge_endpoint` conditions and the
   push ORDER of `collected_headers` unchanged. Structure: nest a `match`
   on the helper `Result` INSIDE the outer `if let Some(..) && !bridge`
   block — do NOT attach an `else` to the combined condition (a spurious
   drop record whenever `user_agent` is `None` would be a defect).
2. In the `#[cfg(feature = "otel")]` injection loop, replace the
   `if let (Ok(name), Ok(val))` tuple-match on the pair construction with
   `constructed_header(&k, &v)`; on `Ok((name, val))` push; on `Err(drop)`
   emit the same debug! shape. (Routing accepted by source review per
   design.md Decision 4 — no cfg-gated test.)
3. Replace the Basic arm's `if let Ok(val) =
   HeaderValue::from_str(&format!("Basic {encoded}"))` with
   `constructed_header("authorization", &format!("Basic {encoded}"))`
   (Ok → push `(AUTHORIZATION, val)`; Err → debug! as above). Add the
   one-line comment: base64 output is always header-safe; the guard is
   kept for uniformity with Bearer. Keep the existing
   `// allow-secret:` comment lines.
4. Replace the Bearer arm the same way with
   `constructed_header("authorization", &bearer)`.
5. Replace the connection-close site
   `&& let Ok(val) = HeaderValue::from_str("close")` with
   `HeaderValue::from_static("close")` (drop the `if let` guard — infallible
   by construction; design.md Decision 2 sanctioned deviation).
6. Add the wire test under Tests to the same `mod tests` module (it already
   uses `start_request_capturing_server` elsewhere; that fixture serves
   exactly ONE request — start two instances).
7. Surgical edits only; no reformat of untouched lines.

**Tests:** (executable spec)
- `producer_invalid_configured_headers_surfaced`
  (`#[tokio::test]` + `#[tracing_test::traced_test]`, dev-dep
  `tracing-test` already present):
  setup: two `start_request_capturing_server` instances; producer/route 1
  configured with `user_agent = Some("bad\r\nua")` and Bearer auth whose
  token contains `"\r\n"` (e.g. `"tok\r\nen"`), destination server 1;
  producer/route 2 configured with valid `user_agent = Some("httpsweep-ok/1")`
  and Bearer token `"valid-token"`, destination server 2. Input exchanges
  are bare: `Exchange::new(Message::default())` — no input headers, so
  `select_outbound_headers` contributes zero drop records and the exact
  count of 2 is deterministic; count records with the module's existing
  `logs_assert` counting pattern (see
  `test_suppressed_body_logs_exactly_one_warn`). Build both producers'
  configuration PROGRAMMATICALLY: construct the endpoint config directly
  with the `user_agent` field set to `Some(<test value>)` and `auth` set
  to `HttpAuth::Bearer { token: <test value> }`
  fields set — mirroring the config-construction style of the neighboring
  producer tests — so the CRLF-bearing values never pass through URI
  parsing.
  action: send one exchange through each producer; await both captured
  requests.
  assert (invalid producer): captured request 1 has NO `authorization`
  header and NO `user-agent` header whose value equals `"bad\r\nua"`
  (value-absence, not "any UA" — reqwest may inject a default user-agent);
  captured logs contain EXACTLY 2 `"outbound header dropped"` records for
  this producer's correlation_id: one with `header=user-agent`, one with
  `header=authorization`, both with reason `invalid header value`; the
  sentinel fragments `"bad\r\nua"` and `"tok\r\nen"` appear in NO captured
  log line.
  assert (valid producer): captured request 2 carries `user-agent:
  httpsweep-ok/1` and `authorization: Bearer valid-token` exactly; zero
  `"outbound header dropped"` records for its correlation_id.
  command: `cargo test -p camel-component-http
  producer_invalid_configured_headers_surfaced`
  expected: before Steps 1-5 the invalid-config assertions fail (headers
  silently absent but NO drop records in logs); after, all pass.
- Regression: `cargo test -p camel-component-http` (full crate) exits 0.

**Acceptance:**
- All four dynamic sites route through `constructed_header` (source-visible;
  otel loop included).
- `cargo clippy -p camel-component-http --features otel -- -D warnings`
  exits 0 (compiles the cfg-gated otel loop — plain clippy never sees it).
- Every gate from the mission's list exits 0, each run as its own command
  from the worktree: `cargo fmt --check --all`;
  `cargo clippy -p camel-component-http -- -D warnings`;
  `cargo xtask lint-unwrap`; `cargo xtask lint-secrets`;
  `cargo xtask lint-non-exhaustive`; `cargo xtask lint-log-levels`;
  `cargo xtask lint-ignore`; `cargo xtask lint-publish-cycles`;
  `cargo xtask lint-component-deps`; `cargo xtask lint-gate-forwarding`;
  `cargo xtask lint-context-citations`; `cargo xtask lint-metric-labels`;
  `cargo xtask schema --check`;
  `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-http --no-deps`;
  `cargo test -p camel-component-http` (full crate).
- connection-close site uses `HeaderValue::from_static("close")`.
- Push order of `collected_headers` blocks unchanged (user-agent block
  before otel block before outbound extend before auth block before
  connection-close).

- [x] 1.2

### Task 1.3: bd bookkeeping — successor bd for Tier A remainder, close sweep + defect bds

Conductor-executed ONLY, from the main repo root, and ONLY after Task 1.2
has passed its acceptance (including every gate) and the implementation has
landed. Never runs while implementation is unreviewed.

**Files:**
- (no repo files — bd operations only, run from the MAIN repo root
  `/home/kenny/dev/rust-camel`, never from the worktree)

**Steps:**
1. `bd create "tech-debt-sweep: camel-http ADR-0070 inline probes (Tier A batch)" --description="Successor of rc-wsx2y item 2: ~20 inline bind-read-drop probes in camel-http lib.rs (drop(listener) sites ~5648-7718 and ~10812), incl. consumer-start tests rebinding via ServerRegistry — same ADR-0070 race class rc-1dgvg removed from the helper. Cluster >=5 = Tier A dedicated batch mission. rc-wsx2y transferred this item on close." -t task -p 3 --deps discovered-from:rc-wsx2y --json` — capture the new id.
2. `bd close rc-wsx2y --reason "swept; item 2 transferred to bd-<new-id>" --json`
   (only after Step 1 succeeds; if creation fails, rc-wsx2y stays open).
3. `bd close rc-jbs1v --reason "fixed + tested: user-agent/otel/Basic/Bearer construction failures now emit DEBUG drop records (name+reason, no values, ADR-0051) via constructed_header; connection-close promoted to HeaderValue::from_static (infallible)." --json`

**Tests:**
- `bd show <new-id> --json` returns status open, priority 3,
  discovered-from:rc-wsx2y.
- `bd show rc-wsx2y --json` and `bd show rc-jbs1v --json` return closed
  with the reasons above.

**Acceptance:**
- Successor bd exists BEFORE rc-wsx2y closes (ordering enforced by Steps).
- Both closes recorded; close reasons name the 4 sites / transfer target.

- [ ] 1.3
