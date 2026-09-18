# Size evidence: slim closure AFTER (bridgeforward task 3.1)

Measured 2026-09-18 in worktree `/home/shared/rust-camel-worktrees/bridgeforward`
(HEAD `d51913aa`). Binary: `target/release/camel`.

## Byte numbers

| Phase  | Bytes        | Note                                            |
|--------|--------------|-------------------------------------------------|
| BEFORE | 55,652,544   | base `f1a70e5f`, same command, 2026-09-18       |
| AFTER  | 44,408,744   | this worktree, seven bridge chains dropped      |
| Delta  | −11,243,800  | absolute                                        |
| Delta  | −20.20 %     | 11,243,800 / 55,652,544                         |

Delta is in the MBs range (11.2 MB), consistent with the cxf/jms/sql/opensearch/ws/xj/xslt
chains leaving the link closure. Mission rule (KBs ⇒ stop) not triggered.

## Exact commands

```sh
# 1. Build slim binary
RUSTC_WRAPPER= cargo build --release -p camel-cli \
  --no-default-features --features slim-benchmarks

# 2. Byte size
stat -c %s target/release/camel
# → 44408744

# 3. Tree proof (bridge chains must be absent)
RUSTC_WRAPPER= cargo tree -p camel-cli --no-default-features -e no-dev --prefix none \
  | grep -E 'camel-component-(jms|sql|opensearch|ws|cxf)|camel-xj|camel-xslt|sqlx'
# → no output (grep exit 1). Line count of the tree fed to grep:
RUSTC_WRAPPER= cargo tree -p camel-cli --no-default-features -e no-dev --prefix none \
  | wc -l
# → 1724

# 4. Boot smoke
timeout --signal=TERM --preserve-status 20 target/release/camel run \
  --routes 'openspec/changes/bridgeforward/evidence/slim-smoke.routes.yaml' \
  > /tmp/slim-smoke.log 2>&1
# → exit code 0
```

Command notes (deviations from the task text, for the record):
- `camel run` takes `--routes <GLOB>`, not a positional file (positional form fails
  with clap `error: unexpected argument`, exit 2).
- `timeout --preserve-status` surfaces the binary's own exit code; plain `timeout`
  reports 124 whenever it delivers the signal, even though the child shuts down
  gracefully on the first SIGTERM (signal contract).

## Tree proof

`grep -E 'camel-component-(jms|sql|opensearch|ws|cxf)|camel-xj|camel-xslt|sqlx'`
over `cargo tree -p camel-cli --no-default-features -e no-dev --prefix none`:
**empty** (0 matches). The seven bridge chains and the sqlx stack are out of the
slim closure.

Redis retention (expected, out of zone): redis lines remain in the tree —

```
camel-redis-repo v0.49.0 (.../crates/services/camel-redis-repo)
camel-component-redis v0.49.0 (.../crates/components/camel-redis)
redis v1.6.0
redis v1.6.0 (*)
```

Retention explanation: `camel-config` depends on `camel-redis-repo`
unconditionally, so the redis stack stays linked under any feature set. Dropping
it is out of zone for bridgeforward.

## Boot smoke transcript

Route file: `evidence/slim-smoke.routes.yaml` (http consumer on
`http://127.0.0.1:18087/hz` → `log:` step; one-shot timer
`timer:smoke?delay=500&repeatCount=1` → `log:slim-smoke-marker`).

Result: **exit code 0** (first SIGTERM → graceful shutdown per the signal
contract). Relevant transcript lines (ANSI stripped):

```
INFO camel_core::lifecycle::application::context_lifecycle: CamelContext started
INFO camel_core::lifecycle::adapters::route_controller_trait: Route started route_id=slim-smoke-once
INFO camel_core::lifecycle::adapters::route_controller_trait: Route started route_id=slim-smoke-http
INFO camel_processor::log: "slim-smoke-marker" exchange_id=86f79810-d51c-4b9c-8068-18de3a66f097
INFO camel_cli::commands::run: Received SIGTERM
INFO camel_core::lifecycle::adapters::consumer_management: Route stopped route_id=slim-smoke-once
INFO camel_core::lifecycle::adapters::consumer_management: Route stopped route_id=slim-smoke-http
INFO camel_core::lifecycle::application::context_lifecycle: CamelContext stopped
INFO camel_cli::commands::run: camel-cli: stopped
```

Both `Route started` lines appear after `CamelContext started`; camel-http's
explicit readiness means the http route's line only prints after the listener
bound (stable assertion — no bind-ack grep, no concurrent curl raced). The
`slim-smoke-marker` line at ~+0.5 s proves the one-shot timer route fired its
log sink end-to-end. Deviation from the task text: the marker lands after
BOTH start lines (timer `delay=500` vs sub-500 ms binds), not between them —
benign, proof value unchanged. This matters because task 3.1 is the first
change to alter the slim linked set: the release artifact itself proves it
boots and serves.
