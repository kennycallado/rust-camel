//! Route interception contract (Tasks 3+4+5+6 of `advice-route-interception`).
//!
//! Task 3: interception rules may only be installed before first use: once a
//! route is registered or the context is started, the rule set is frozen
//! because compiled pipelines capture it at compile time. These tests pin the
//! two freeze trip points (`add_route` success and `MarkStarted`) and prove
//! the freeze survives a failed start (no trip) and a stop/restart cycle (no
//! unfreeze).
//!
//! Task 4: a `SkipTo` rule substitutes the send URI at the `To` compile point
//! before component resolution. These tests pin first-match-wins, the
//! no-op behaviour of an empty rule set, substitution to an unregistered
//! source component, the enriched compile error for an unresolvable target,
//! and the send-side fence (the intercepted send never enqueues).
//!
//! Task 5: a `DivertCopyTo` rule composes a wiretap copy stage in front of
//! the real producer at the `To` compile point. These tests pin dual
//! delivery, CallerRuns saturation (the copy runs inline before the real
//! send), outcome isolation in both directions (copy failures never alter
//! the real `Result` and vice versa), copy-readiness suppression, the
//! enriched compile error for an unresolvable copy target, real-producer
//! readiness driving, and divert survival across route stop/restart.
//!
//! Task 6: a recompiled pipeline (hot-reload `compile_route_definition` +
//! `swap_pipeline`) keeps applying the frozen rule set.
//!
//! The suite is split into submodules under `tests/route_interception/`:
//! `plumbing` (freeze contract), `skip` (SkipTo substitution + hot-reload
//! rule consistency), `divert` (DivertCopyTo composition), with shared
//! helpers in `common` and divert stubs in `support`.

#[path = "route_interception/common.rs"]
mod common;
#[path = "route_interception/divert.rs"]
mod divert;
#[path = "route_interception/plumbing.rs"]
mod plumbing;
#[path = "route_interception/skip.rs"]
mod skip;
#[path = "route_interception/support.rs"]
mod support;
