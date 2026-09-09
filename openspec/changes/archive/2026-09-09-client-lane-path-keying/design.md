# Design: client-lane-path-keying

## Approach

The composite lane key is `registered_key + "\x1f" + path_and_query`.
The `\x1f` (US separator) cannot appear in a valid URI authority or an
origin-form path, so the join is collision-free.

Composition lives inside `ClientLane` (`adapters/http.rs`), one module
away from `ParsedTarget`. `ClientLane::launch` already parses the
target URI before the dial, so it composes the map key from
`ParsedTarget::parse(target_uri).target` with no second parse.
`ClientLane::take` gains a `uri` parameter and composes the same way
from the interpolated reference. Both sides derive the path through
the identical function, so the keys agree by construction. The
authority rewrite (`rewrite_authority`) preserves path and query, so
the launch-side wire target and the take-side interpolated reference
yield the same path bytes.

`ClientLane` stays a string-keyed FIFO store. The rc-qogy invariants
move with the key: the FIFO bound, the pre-dial overflow refusal, the
booking re-check under the insert lock, and the generation-stamped
failure transition all operate per composite key. The spawned exchange
carries the composite for `fail_lane_entry`.

Order change inside `launch`: target parse moves before the overflow
pre-check, because the composite key needs the parsed path. A send
with an invalid target and a full FIFO now reports the parse error.
No wire effect happens in either order, so the pre-dial refusal
invariant holds.

Fall-through is preserved. `take` returns `None` when the reference
fails to parse (a bare authority) or when no entry sits under the
composite key. The router then delegates to the server role exactly as
before. A bare-authority dynamic receive keeps its ps97b apparatus
error, raised by the server-role path.

Diagnostics: `TransportError::LaneFifoOverflow` renders the lane key
as `redact(key) + " " + redact(path)`, each half masked on its own
under the ADR-0051 secret rule. The raw composite never leaves the
module. The runner's render-site redaction stays a second defense:
the exact pre-rendered shape (one space, path half leading `/`)
redacts per half, and any other string, including a raw third-party
key, redacts as one value so no split can sever a secret.
`await_parked` keeps receiving the plain interpolated endpoint, which
renders redacted as today.

`expectReply` is untouched by construction: the document parser gates
it to `direct:` sends (`document.rs`), and `direct:` sends return the
synchronous reply through the `Ok` slot. They never touch the client
lane.

## Affected crates

- `camel-integration-test`: `adapters/http.rs` (composite key in
  `launch`/`take`, overflow rendering, unit tests),
  `adapters.rs` (the `take` call site passes the interpolated
  reference), `tests/http_client_lane_test.rs` (overflow redaction
  test), `tests/http_partner_scripting_test.rs` (the path-blind
  characterization inverts into a path-aware no-cross-match test).

## Architecture boundaries

The change stays inside the harness apparatus (ADR-0069 section 5).
No route runtime, DSL, component, or engine code changes. The
two-key dispatch contract (declared key, interpolated wire address)
is unchanged. Only the client-lane map key composition changes.
Redaction follows the ADR-0051 positive secret rule at the same
render sites as today.

## Related decisions

- ADR-0069 section 5: partner wire is the normative proof. The
  apparatus must not cross-match what the wire kept distinct.
- ADR-0051: positive secret rule. Overflow lane-key rendering masks
  secret-marked query values.
- rc-qogy: bounded FIFO canon, preserved per composite key.
- rc-ps97b: per-path server-role receive, the baseline this change
  completes for the client role.
