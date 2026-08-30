# Tasks: mcp-spec-exposure-gate-wording

## openspec

### Task 1: Verify wording against pinned tests and archive

**Files:**
- `openspec/changes/mcp-spec-exposure-gate-wording/specs/mcp-component/spec.md` (the delta, already authored)

**Steps:**
1. Cross-check every claim in the MODIFIED requirement's first
   paragraph and the two replacement scenarios against
   `crates/components/camel-component-mcp/tests/server_config_test.rs`:
   `policy_less_config_no_longer_refused_at_validate` (loopback
   `Ok(None)`, non-loopback `Ok(Some(NonLoopback))`) and
   `mcp_old_bind_gate_removed` (kernel gate refuses non-loopback
   policy-less without ack). No code change is allowed.
2. Run `openspec validate mcp-spec-exposure-gate-wording --type change
   --json` and require zero delta-structure failures.

**Tests:**
- `delta-matches-pinned-tests`: each SHALL in the new first paragraph
  maps to one of the two named tests or to the ADR-0061 rule → manual
  mapping recorded in the change dir review; no contradictory test
  exists (grep for `MissingSecurityPolicy` returns nothing in src/).
- `validate-passes`: `openspec validate mcp-spec-exposure-gate-wording
   --type change --json` exits 0.

**Acceptance:**
- `grep -rn "MissingSecurityPolicy" crates/components/camel-component-mcp/src/` returns no matches (old gate really gone).
- `openspec validate mcp-spec-exposure-gate-wording --type change --json` reports no failed items.

- [x] 1.1
