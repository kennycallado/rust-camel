# Proposal: client-lane-path-keying

## Why

`ClientLane` parks http client-role roundtrips keyed on the registered
partner key only. The path never enters the key. Under dynamic
references, two standalone roundtrip receives (a later `receive`, not
`expectReply`) that name different paths of one partner cross-match
oldest-first. A receive for `/b` can drain the roundtrip parked by the
send to `/a`.

The path-blind behavior was a documented deferral, not an accident.
Change rc-ps97b (archived, commit 3bddc618) recorded it in the
wire-fidelity requirement and named this issue: bd rc-cr5yf. This
change lifts that deferral.

## What Changes

- The client lane map key becomes the registered partner key joined
  with the wire `path_and_query` of the dialed target. Both the launch
  side and the take side derive the path through the same parser, so
  the two keys agree by construction.
- Same-path sends keep the bounded FIFO wire-arrival canon (rc-qogy).
  The FIFO bound, the pre-dial overflow refusal, and the
  generation-stamped failure transition stay per composite key.
- `expectReply` pairing is untouched. The grammar gates `expectReply`
  to `direct:` sends. Those sends never use the client lane.
- The `integration-tier` spec requirement "Wire-fidelity lane key and
  mismatch diagnostics" changes: the PATH-BLIND sentence and its
  scenario are replaced with path-aware semantics.
- Diagnostics that name the lane key (FIFO overflow) render the path
  half redacted under the ADR-0051 secret rule.

Affected crates: `camel-integration-test` only.

Bd issue: rc-cr5yf.

## Acceptance criteria

- Two standalone roundtrip receives that name different paths of one
  partner do not cross-match. Each drains its own path's parked entry.
- Three same-path sends with no intervening receives drain
  oldest-first, in wire order (rc-qogy canon battery stays green).
- Receive-first-then-take flows stay green. A receive with no parked
  entry under its composite key falls through to the server role.
- The `expectReply` regression battery stays green.
- The existing lane tests (`http_client_lane_test.rs`) stay green
  without semantic edits.

## Risk budget

Accepted: the overflow error now names the composite lane key, and the
overflow pre-check moves after target parse (a send with an invalid
target and a full FIFO now reports the parse error). Both are
diagnostic-shape changes only.

Out of bounds: any change to server-role arrival lanes, to
`expectReply` grammar or pairing, or to the FIFO bound itself.
