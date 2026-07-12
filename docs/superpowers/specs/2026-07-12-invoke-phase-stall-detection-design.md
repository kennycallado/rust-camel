# Invoke-Phase Stall Detection for Streaming-Input Guests

## 1. Problem

After fixing the watchdog false-trip regression (rc-u1h6, commit fff9340), the
phase-aware watchdog (`drive_with_drain_watchdog`) has **no timeout in Phase 1
(invoke)**. It waits indefinitely for `drain_started` or completion.

Epoch interruption bounds *executing* WASM but not *suspended async*. A guest
parked on `await upstream_chunk` that never arrives is not "executing" — epoch
won't fire. The only protection is `timeout_secs` via epoch, which doesn't cover
this case.

This is a pre-existing gap (not a regression). The old watchdog accidentally
covered it by false-tripping at 60s on any invoke without progress — at the cost
of false-tripping on legitimate CPU-bound work.

## 2. Scope

**In scope:** Arm the Phase 1 watchdog for guests with **streaming input** only.
Non-streaming input guests remain unguarded in Phase 1 (epoch covers them
completely).

**Non-goals:**
- Guest-cooperative heartbeats (infeasible for third-party guests)
- Separating "CPU-bound after reading" from "upstream stalled" at the host level
  (requires epoch-consumption feedback wasmtime doesn't expose)
- New config knob (reuses `timeout_secs`)

## 3. Design

### 3.1 Key insight

`coord.progress` is a single `Arc<Notify>` shared by both:
- Input path: `BoxStreamProducer` fires `notify_one()` per input chunk shipped to guest
- Output path: drain fires `notify_one()` per output chunk shipped downstream

Phase 1 arming uses the **same signal** Phase 2 uses. "Progress" = "a chunk
moved in either direction."

### 3.2 Timeout source: `timeout_secs`, not `no_progress_timeout`

`no_progress_timeout` (60s) is semantically "drain backpressure tolerance" — how
long an output stream may idle. Phase 1 stall is a different failure (upstream
liveness). Its natural bound is `timeout_secs` — the same wall-clock the
operator configured for "how long may this invocation take."

A CPU-bound guest that exceeds `timeout_secs` would trip epoch anyway. Arming the
Phase 1 watchdog with the same duration makes the two bounds **consistent**: if
the guest is CPU-bound, epoch fires; if the guest is I/O-suspended, the watchdog
fires. Both at the same deadline.

### 3.3 Signature change

```rust
pub(crate) async fn drive_with_drain_watchdog<F, T>(
    run_fut: F,
    progress_notify: &Notify,
    drain_started: &Notify,
    drain_timeout: Duration,
    invoke_stall_timeout: Option<Duration>,  // NEW
) -> Result<T, WasmError>
```

- `None` → Phase 1 unguarded (non-streaming input, current behavior)
- `Some(t)` → Phase 1 armed with progress watchdog, timeout = `t`

### 3.4 Phase 1 loop

```rust
match invoke_stall_timeout {
    None => tokio::select! {
        r = &mut run_fut => return r,
        _ = drain_started.notified() => {}
    },
    Some(t) => loop {
        tokio::select! {
            r = &mut run_fut => return r,
            _ = drain_started.notified() => break,
            _ = progress_notify.notified() => continue,
            _ = tokio::time::sleep(t) => {
                return Err(WasmError::GuestPanic(
                    "wasm: invoke stalled — no input progress \
                     (upstream stalled or guest deadlocked)".into(),
                ));
            }
        }
    },
}
// Phase 2 unchanged
```

### 3.5 Flag flow

`spawn_return_drain` gains `invoke_stall_timeout: Option<Duration>`, passed
straight to `drive_with_drain_watchdog`.

**Plugin path** (`runtime.rs process_streaming_exchange`):
```rust
let invoke_stall_timeout = stream_parts
    .as_ref()
    .map(|_| Duration::from_secs(self.config.timeout_secs));
```
Checked before `stream_parts` is moved into the closure.

**Bean path** (`bean.rs call`):
```rust
let invoke_stall_timeout = stream_parts
    .as_ref()
    .map(|_| Duration::from_secs(self.ctx.config.timeout_secs));
```

### 3.6 Source path

The source path (`source_host.rs`) does **not** use `spawn_return_drain` or
`drive_with_drain_watchdog`. It drains via `Accessor::spawn(SubmitExchangeDrain)`
on the same event loop. No change needed — source hosts don't invoke guests
that consume input streams.

## 4. Edge cases

| Scenario | Behavior |
|----------|----------|
| Non-streaming input | `invoke_stall_timeout = None` → Phase 1 unchanged |
| Streaming input, chunks flow normally | Each input chunk fires `progress` → resets timer |
| Streaming input, upstream stalls | No progress for `timeout_secs` → trip |
| Streaming input, guest CPU-bound > `timeout_secs` | Trip — but epoch would also trip. Consistent. |
| Guest completes before `drain_started` | `run_fut` resolves → return immediately |
| Cancel fires during Phase 1 | Outer `select!` in `spawn_return_drain` handles it (unchanged) |

## 5. Testing

1. **Phase 1 trips on stalled streaming input** — `Some(timeout)`, no progress,
   no `drain_started` → error within timeout.
2. **Phase 1 progress resets** — `Some(timeout)`, periodic `progress` → no trip.
3. **Phase 1 None = unguarded** — `None`, no progress, no `drain_started` →
   hangs (verified by timeout in test harness, not by watchdog).
4. **Phase 1 completes before drain_started** — `Some(timeout)`, `run_fut`
   resolves → returns value.

## 6. Files touched

- `runtime.rs` — `drive_with_drain_watchdog` signature + Phase 1 loop; 4 existing
  unit tests updated to pass `None`; 3 new tests
- `return_stream.rs` — `spawn_return_drain` gains `invoke_stall_timeout` param,
  passes to watchdog
- `bean.rs` — call site computes `invoke_stall_timeout` from `stream_parts`
- `producer.rs` — no change (producer calls `process_streaming_exchange` which
  handles it internally)

## 7. Alternatives rejected

- **Option A (wall-clock invoke timeout for all):** Reintroduces the false-trip
  on non-streaming CPU-bound work — the exact regression we just fixed.
- **Option C (guest heartbeat):** Infeasible without third-party guest
  cooperation.
