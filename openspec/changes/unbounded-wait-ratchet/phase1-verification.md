# Phase 1 verification — lint mechanics (Task 1.2)

Evidence for the frozen `lint-unbounded-wait` behaviour that Phase 2 relies on.
Produced with a disposable integration-test file
`crates/camel-test/tests/unbounded_wait_mechanics_scratch.rs`, never committed
and removed before the Task 1.2 commit.

## Command

```text
cargo xtask lint-unbounded-wait 2>&1 | tee /home/shared/tmp/394-lint-p1.txt; echo EXIT:${PIPESTATUS[0]}
```

Exit status: `1` (count above the ceiling of 296, as expected).

Count line with the scratch file present:

```text
UNBOUNDED-WAIT RATCHET EXCEEDED (300 findings > max 296):
```

## (a) Helper shape is not a finding

`helper_shape_kills_finding` awaited the receive through the shared helper:

```rust
let value = timeout_wait("scratch", Duration::from_secs(30), rx.recv())
    .await
    .expect("closed");
```

Observation: **absent from the findings list**. The awaited base is a
free-function call outside the wait-class set (`recv`/`lock`/`acquire`/`wait`/
`join_next`/`connect`, spawn handles, `TcpStream::connect`), so the helper
shape introduces no new finding. The helper's own `tokio::time::timeout`
wrapping is recognised for loop sites only (see (c)); for one-shot waits the
absence is because the await is not wait-class at all.

## (b) `allow-test-wait` marker suppresses the finding

`marker_suppresses_finding` awaited a bare receive with the marker on the
same line, after a deterministic send:

```rust
tx2.send(1).await.expect("send");
let value = rx2.recv().await; // allow-test-wait: scratch: test-fed channel, value sent before recv
```

Observation: **absent from the findings list**. The non-empty marker reason
adjudicates the site at scan time and it does not count toward the ceiling.

## (c) Loop containing `timeout_wait` IS flagged (frozen-lint boundary)

`loop_helper_shape` contained a `loop` whose every iteration awaited
`timeout_wait("scratch loop", Duration::from_secs(30), rx3.recv()).await` and
exited when the channel closed.

Observation: **flagged**, at the scratch file's `loop` line
(`crates/camel-test/tests/unbounded_wait_mechanics_scratch.rs:34`). This
accounts for the count of 300 instead of 299. The frozen lint does not treat
a `timeout_wait` call as a recognised per-iteration deadline region
(`TIMEOUT_TARGETS` contains only `tokio::time::timeout` /
`tokio::time::timeout_at`), so loop-class sites in Phase 2 use direct
`tokio::time::timeout` per the shared conventions; the helper remains
correct for one-shot waits.

After the scratch file was removed the same command reported exactly:

```text
UNBOUNDED-WAIT RATCHET EXCEEDED (299 findings > max 296):
```

which confirms the scratch file contributed exactly the one loop finding and
that the helper/marker shapes contributed none.
