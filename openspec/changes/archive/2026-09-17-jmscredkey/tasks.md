# Tasks: jmscredkey

## camel-component-jms

### Task 1.1: Pin the credential-shaped-key exception at the component boundary

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified)

**Steps:**
1. In `crates/components/camel-jms/src/config.rs`, locate `mod tests`
   and the existing pin `redact_broker_url_delegation_pin`.
2. Below that pin, add one test `#[test] fn redact_broker_url_suppresses_credential_shaped_key()`
   with a doc comment citing bd rc-tfugr and the jms spec scenario
   "credential-shaped key never echoes" (ADR-0076 appendix,
   key-position credential-shape symmetry).
3. Add a second test `#[test] fn redact_broker_url_keeps_non_shaped_sensitive_keys()`
   with a doc comment citing the jms spec scenario "non-shaped
   sensitive keys keep their key names".
4. Both tests call the crate-local `redact_broker_url` (already in
   scope from `super::*`) and assert exact outputs, matching the
   exact-output pin style of
   `redact_broker_url_masks_userinfo_and_sensitive_query`.

**Tests:** (executable spec — name, arrange, act, assert)
- `redact_broker_url_suppresses_credential_shaped_key`: fixture
  `"tcp://h:61616?user%3Asecret%40host=1"` (uppercase hex) →
  `assert_eq!(redact_broker_url(fixture), "tcp://h:61616?<redacted>")`
- same test: fixture `"tcp://h:61616?user%3asecret%40host=1"`
  (lowercase hex) → `assert_eq!(..., "tcp://h:61616?<redacted>")`
- same test: fixture `"tcp://h:61616?user:pass@host=1"` (literal) →
  `assert_eq!(..., "tcp://h:61616?<redacted>")`
- same test: negative containment —
  `assert!(!redacted.contains("secret") && !redacted.contains("user%3Asecret%40") && !redacted.contains("user:pass"))`
  over each redacted output
- `redact_broker_url_keeps_non_shaped_sensitive_keys`: fixture
  `"tcp://h:61616?password=p&jms.userName=admin&user=u&keepAlive=true"` →
  `assert_eq!(redact_broker_url(fixture), "tcp://h:61616?password=<redacted>&jms.userName=<redacted>&user=<redacted>&keepAlive=true")`
- same test: fixture `"tcp://h:61616?user@host=1"` (lone `@` key) →
  `assert_eq!(..., "tcp://h:61616?user@host=<redacted>")`
- same test: fixture `"tcp://h:61616?pass%77ord=p"` →
  `assert_eq!(..., "tcp://h:61616?pass%77ord=<redacted>")`
- existing tests unchanged and green: `redact_broker_url_masks_userinfo_and_sensitive_query`,
  `redact_broker_url_delegation_pin`, `redact_exact_userinfo_mask`,
  `redact_exact_failover_param_boundaries`
- command: `RUSTC_WRAPPER= cargo test -p camel-component-jms redact`
  (from the worktree root; both new tests must pass; before this
  task the functions do not exist, so compilation fails — expected
  red)

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-component-jms` exits 0
- `RUSTC_WRAPPER= cargo fmt --check` exits 0
- `RUSTC_WRAPPER= cargo clippy -p camel-component-jms -- -D warnings`
  exits 0
- No production line outside `mod tests` changes
  (`git diff -- crates/components/camel-jms/src/config.rs` shows
  additions inside the test module only)

- [x] 1.1

## openspec

### Task 1.2: Validate the delta, archive the change, refresh the ADR note

**Files:**
- `openspec/changes/jmscredkey/tasks.md` (modified — checkbox flip only)
- `docs/adr/0076-url-redaction-strictest-wins.md` (modified — one
  annotation line in the appendix §2 spec note)

**Steps:**
1. Run `openspec validate jmscredkey --type change --json` from the
   worktree root; require zero delta-structure errors.
2. Confirm every task block above is checked.
3. Run `openspec archive jmscredkey --json` from the worktree root;
   this syncs the delta into `openspec/specs/jms/spec.md` (replacing
   the requirement block) and moves the change to
   `openspec/changes/archive/`.
4. In `docs/adr/0076-url-redaction-strictest-wins.md`, appendix §2
   spec note, append one sentence after "over-masking is safe
   (ADR-0051).": "Landed: openspec change jmscredkey (bd rc-tfugr)
   amended the jms spec canon with this qualifier." This keeps the
   ADR note true after archive (r_glm finding 1).
5. Commit the archive result plus the ADR annotation on
   `feat/jmsqual` with message `chore(openspec): archive jmscredkey`.

**Tests:**
- `validate strict`: `openspec validate jmscredkey --type change --strict --json`
  → no errors before archive
- `canon check`: after archive, `grep -c "key-position credential-shape exception" openspec/specs/jms/spec.md`
  returns 1 and `grep -c "Scenario: credential-shaped key never echoes" openspec/specs/jms/spec.md`
  returns 1
- `archive clean`: `openspec validate --json` on the synced spec
  (or `openspec list --json`) reports the jms spec valid and the
  change no longer active

**Acceptance:**
- `openspec/changes/archive/` gains the jmscredkey directory
- `openspec/specs/jms/spec.md` carries the exception clause and both
  new scenarios, with all six prior scenarios intact
- `docs/adr/0076-url-redaction-strictest-wins.md` §2 spec note
  carries the "Landed: openspec change jmscredkey" sentence
- Worktree `git status` clean after the archive commit

- [x] 1.2
