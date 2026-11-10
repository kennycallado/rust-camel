# Design: mcp-connect-deadline

## The wrapped future

`RmcpClient::connect` (`src/adapter/client.rs:82-90`) awaits, today with no
deadline:

```rust
().serve_with_lifecycle(transport, ClientLifecycleMode::Discover { .. })
```

That call is `ClientServiceExt::serve_with_lifecycle` →
`serve_client_with_lifecycle` → `serve_client_with_lifecycle_and_ct`, an
`async fn` that runs the whole discover handshake — transport POST of
`server/discover`, response correlation, version negotiation — and resolves
to `RunningService`. The future owns the transport by value: rmcp calls
`transport.into_transport()` on it, producing a
`WorkerTransport<StreamableHttpClientWorker>` inside the future's state;
the worker task itself runs as an independent `tokio::spawn`ed task.

The lever is exactly this future. We wrap it:

```rust
let connect_deadline = Duration::from_secs(config.connect_timeout_secs);
let service = tokio::time::timeout(
    connect_deadline,
    ().serve_with_lifecycle(transport, ClientLifecycleMode::Discover {
        preferred_versions: vec![PROTOCOL_VERSION],
    }),
)
.await
.map_err(|_| connect_deadline_error(name, connect_deadline))?
.map_err(|error| map_connect_error(name, error))?;
```

**What the deadline bounds: the caller.** `connect` returns the named error
at the deadline, so route start (the `McpProducerLifecycle::start` await —
today the hang) is rescued. The DNS-pinning resolve
(`build_pinned_http_client`, ahead of the wrap) keeps its own 5 s bound
(`RESOLVE_TIMEOUT`) and is not covered; the deadline starts after the
pinned reqwest client is built. The post-connect `request_timeout_secs`
deadline is unaffected — it is applied per request after `connect`
returns.

`request_timeout_secs` is **not** the knob for this, deliberately: its
documented contract (docs, CONTEXT.md, 567 spec) is the *post-connect*
request deadline with a 120 s default. Route start must not hang 2 minutes
on a wedged remote, and folding both phases into one value would force one
number to serve two budgets (a connect-safe 15 s would break slow LLM-backed
tool calls; a request-safe 120 s leaves connect effectively unbounded).
Connect gets its own option with its own default.

## Drop semantics: what is reclaimed, what is not (verified, not assumed)

If the timeout fires, `tokio::time::timeout` drops the
`serve_with_lifecycle` future. Two regimes follow, verified against rmcp
3.9.0 sources (Cargo.lock; no fork):

**Bootstrap phase — the worker and its socket are NOT reclaimed.** In
Discover mode the discover POST *is* rmcp's bootstrap POST:
`StreamableHttpClientWorker::run` awaits
`post_message_with_max_sse_event_size(...)` **directly**
(`streamable_http_client.rs:1183-1186`), before the event loop's main
`select!` — the only place `transport_task_ct.cancelled()` is watched
(`:1437-1439`) — is entered. Dropping `WorkerTransport` cancels the token
(its `DropGuard`, `worker.rs:169-180`), but nothing selects on the token
during bootstrap: the parked worker task stays in the POST await and the
reqwest request — hence the socket — stays open until the remote answers
or TCP dies. Dropping the join handle detaches it; it cannot abort. The
`posts`/`control_posts` collections that *would* be dropped clean
(`FuturesUnordered`; `streams` is the only `JoinSet`) live in the main
loop, which is never entered. **This is a real residual: one parked worker
task + one socket per wedged connect attempt**, persisted until the remote
responds. Filed upstream as bd `rc-s4xod` (fix belongs in rmcp: select the
bootstrap POST against the token, or abort the join handle on drop).
Amplification is operator-bounded: producer start has no retry loop
(`producer.rs:203-211` — one `connect` per start, failure fails the route
start), so growth comes only from operator-driven restarts.

**Post-bootstrap phase — clean cooperative exit.** Once the main loop is
running, token cancellation breaks the loop (`WorkerQuitReason::Cancelled`),
the worker future drops, its pending POST collections drop (aborting the
in-flight futures), and unpooled connections close
(`pool_max_idle_per_host(0)`). In Discover mode connect this regime is
unreachable during a deadline firing — bootstrap *is* the connect phase —
but it is what protects every later drop path (running-service drop on
shutdown), and is why the wrap cannot make shutdown worse.

**Bounded what we can; residuals stated.** Per the mission's escape hatch:
route start is bounded (the P3 hang is fixed); the worker/socket reclamation
in the wedge shape is upstream rmcp's to fix (`rc-s4xod`). Alternatives
rejected:

- **reqwest total per-request timeout** on the pinned client: it would
  bound the bootstrap POST *and* the worker would exit — but the same
  client serves every later request, so post-connect tool calls would die
  at the connect bound with a generic reqwest error, breaking the
  `request_timeout_secs` contract (mission hard limit).
- **rmcp transport config knobs**: 3.9.0 exposes only
  `control_request_timeout` and `session_recovery_timeout` (post-bootstrap
  control POSTs / recovery) — neither covers bootstrap. No knob exists
  without a fork (banned).
- **Auto lifecycle mode**: its 10 s timeout would fire, but Auto then falls
  back to the legacy `initialize` handshake, which this component must
  never speak (fail-fast taxonomy, `McpError::IncompatibleRemote`).
- **Aborting the worker from outside**: the join handle is private inside
  the dropped future's state; no external reach exists.

**Test observability follows the same honesty**: a socket-EOF assertion
after a timed-out connect would *fail* (truthfully — the socket persists
until the remote answers). The harness therefore does not assert socket
reclamation; it asserts the caller bound, the named error, the healthy
path, and that the connect bound does not leak into post-connect requests.
The residual is stated here, in the proposal, and in the docs — not
silently skipped.

## The value: 15 s default, from evidence

rmcp's own discover-handshake budget is 10 s: `DEFAULT_AUTO_DISCOVER_TIMEOUT
= Duration::from_secs(10)` (`rmcp/src/service/client.rs:833`), applied in
Auto mode as `tokio::time::timeout(10s, discover_startup(...))` — a budget
that already covers the lazy TCP connect and the discover POST *together*
(client.rs:868-879). The component's separate 5 s TCP budget is an inner
limit that fires first for stalled TCP; the outer connect deadline must not
fire under a healthy remote that consumed it.

15 s is **policy margin, not a budget sum**: the same handshake rmcp gives
10 s gets 15 s here, so a remote that legitimately uses its full 5 s TCP
budget before the POST still gets rmcp's complete 10 s POST budget (in
Auto mode rmcp would starve that POST to 5 s — we choose not to). A healthy
MCP server's discover handshake is one POST round-trip plus negotiation —
tens of milliseconds on loopback, low hundreds over TLS to a sane remote;
15 s sits two orders of magnitude above the healthy cost and below the
"route start feels hung" range.

Behaviour change, stated: connect previously never returned on this
failure; it now fails by default after ≤15 s. Worst-case startup cost
addition is bounded by DNS (≤5 s) + connect deadline (≤15 s) ≈ 20 s, not
15 s alone. A deployment with a legitimately very slow remote sets
`connect_timeout_secs` higher; nothing else changes.

## Configuration

`connect_timeout_secs: u64` on `McpRemoteConfig`, in the exact style of
`request_timeout_secs` (567 established it as the component's option
pattern):

- `const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 15;` + `#[serde(default =
  "default_connect_timeout_secs")]`
- `deserialize_connect_timeout_secs`: whole seconds, at least 1; `0` →
  `connect_timeout_secs must be at least 1 (got 0)`; non-numeric →
  `connect_timeout_secs must be an integer number of seconds of at least 1`
  — each naming the option, mirroring the existing deserializer's
  rationale (a plain `u64` cannot name itself).
- Existing `McpRemoteConfig` literal constructions in tests gain the field
  (mechanical, 7 sites).

## The error

```rust
fn connect_deadline_error(server: &str, deadline: Duration) -> McpError {
    McpError::Endpoint(format!(
        "connect to remote '{server}' did not complete the discover \
         handshake within {}s", deadline.as_secs()
    ))
}
```

Names the phase (connect), the mechanism (discover handshake), and the
bound. The 5 s TCP (`... exceeded the 5s TCP connect limit`) and DNS
(`timed out resolving remote host '<host>' after 5s`) messages stay byte-
identical — 567 just made them honest; this change does not touch them.
Ordering and precedence: the wrapper's elapsed arm is distinct from
`map_connect_error` — an inner transport error still routes through the
existing taxonomy (including the 5 s TCP-timeout arm); only the *elapsed*
deadline produces the new message. Precedence between the outer deadline
and the inner TCP budget depends on the *remaining* outer time when the
TCP connect starts: the outer timer is armed first (right after DNS
pinning) and can win even at a configured 5 s — the TCP error is
reachable only when the outer bound leaves more than 5 s of budget at
that point (e.g. the default 15 s); with a tighter bound the outer error
firing first is the configured contract, not a regression. The 5 s texts
stay exercised by the existing 567-era tests.

## Test design

Fake-server harness from 567 (`tests/common/mod.rs`), extended:

- `MockOptions.stall_discover: bool` — the mock's `discover` handler parks
  (`std::future::pending`) instead of answering: TCP is accepted (axum has
  the connection), the `server/discover` POST is never answered. This is
  the wedge shape the deadline must rescue.
- `MockOptions.tool_delay_ms: u64` — canned `tools/call` answer delayed by
  the given milliseconds (0 = immediate). For proving the connect bound
  does not become a global request bound.
- New integration file `tests/connect_deadline_test.rs` (bind
  `127.0.0.52`, the per-file convention):
  1. **stalled discover trips the deadline**: parked-discover mock,
     `connect_timeout_secs = 1` (whole-second config floor; the mission's
     200 ms example is not expressible in the `*_secs` unit convention —
     1 s is the shortest configurable bound), outer 10 s watchdog, error
     contains `did not complete the discover handshake within 1s` and the
     server name. Also asserts no 5 s-budget text fired (the mock accepts
     promptly, so the inner budgets are not what fired).
  2. **healthy connect unaffected**: normal mock, same explicit bound,
     connect succeeds and a canned `tools/call` round-trips (deadline
     wrapper does not disturb a healthy handshake).
  3. **the connect bound is not a global request bound**:
     `connect_timeout_secs = 1`, `request_timeout_secs = 30`,
     `tool_delay_ms = 1500` — connect succeeds (fast handshake), the call
     takes ~1.5 s (exceeding the connect bound) and still succeeds: the
     deadline is connect-scoped, not leaked into reqwest as a total
     request timeout.
  4. **worker reclamation is not asserted**: per the drop-semantics
     analysis, a socket-EOF assertion would be false in the wedge shape
     (rmcp 3.9.0's bootstrap POST is un-cancellable, `rc-s4xod`). The
     harness states this limitation and tests the caller-side contract
     instead; no socket/task-leak claim is made or tested.
- Config parse/validation (config.rs unit tests, mirroring 567's): default
  15 applied when absent; explicit value used; `0` rejected naming the
  option; `""` rejected naming the option.
- Unit test for `connect_deadline_error` message shape (server name +
  bound + phase text).

The default value is not behaviour-tested end-to-end (a real 15 s sleep per
test run); the default's parse-level application is tested and the wiring
is shared with the explicit-value path — one code path, proven with the
short bound.

## Docs

`docs/src/components/mcp.md` timeout section (line 57 area): gains the
connect bound — `connect_timeout_secs` (default 15) bounds the discover
lifecycle future (TCP connect + `server/discover` POST); DNS resolution
precedes it with its own unchanged 5 s budget (not covered by this
deadline). TCP's 5 s budget is inner to the wrapper and surfaces first
only when the outer bound leaves more than 5 s remaining when the connect
starts (true for the default 15 s); a tighter outer bound fires first
with the named connect error. Connect-deadline errors name the bound.
States the residual honestly: on a wedged remote the in-flight handshake's
connection is reclaimed by upstream rmcp mechanics when the remote answers
or TCP gives up, not at the deadline (rmcp 3.9.0 limitation, tracked
upstream) — route start itself is bounded either way. Worst-case startup
addition is DNS + connect bound (≈ 20 s with defaults).

`crates/components/camel-component-mcp/CONTEXT.md` config-keys section:
the remote option list gains `connect_timeout_secs` with one line on what
it bounds.

## Alternatives rejected

- **Extend `request_timeout_secs`** — rejected above: two phases, two
  budgets, one knob cannot serve both (120 s default would leave connect
  unbounded in practice; 15 s would break slow tool calls).
- **rmcp Auto mode with fallback** — rejected: Auto falls back to the
  legacy `initialize` handshake on discover timeout, which this component
  must never speak (fail-fast taxonomy, `McpError::IncompatibleRemote`);
  it would also silently widen the protocol surface 567 deliberately
  closed.
- **Fork/patch rmcp for a discover-phase knob** — rejected by mission hard
  limit; the component-boundary wrap achieves the caller bound with no
  fork, and the worker-reclamation residual is filed upstream (`rc-s4xod`).
- **reqwest total request timeout** — rejected in the drop-semantics
  section: unbreakable coupling with the post-connect budget (hard limit).
- **Bound each rmcp-internal await separately** — impossible without the
  fork; the internals are private.
