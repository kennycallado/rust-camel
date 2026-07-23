//! M2 benchmark harness load generator (spec §4.5).
//!
//! This crate provides the persistent Rust HTTP load generator for Protocol A
//! measurement (request-response against the T3 HTTP-server fixture), plus
//! the novel statistical and sampling logic the v3 harness invokes:
//!
//! - [`stats`] — percentile, median-of-rounds aggregation, nearest-rank.
//! - [`calibration`] — Protocol A calibration phase (spec §4.4): doubling
//!   rate every 25 requests until p99 > 100ms or backpressure observed.
//! - [`warmup`] — within-warmup stability criterion (spec §4.3):
//!   p50 of messages 501–1000 within 10% of p50 of messages 1–500.
//! - [`protocol_b`] — parse `/tmp/v3-protocol-b-<cell>.log` per-tick records
//!   (`BENCH_LATENCY <tick> <ns>`), aggregate per-round and across rounds.
//! - [`bca`] — BCa (bias-corrected and accelerated) bootstrap confidence
//!   interval on per-round p50 (e_opus deferral to plan, spec §4.2).
//! - [`rss`] — process-tree RSS sampling via `/proc/<pid>/task/<tid>/children`
//!   recursive walk (spec §4.9); headline = max of per-sample Σ VmRSS.
//! - [`bridge`] — bridge generation tracking (spec §4.6 + §4.11): PID at
//!   round boundaries, invalidate-on-change, 30s first-output health
//!   timeout.
//!
//! The load generator binary lives at [`src/main.rs`](../src/main.rs) and
//! dispatches on subcommands (`devnull`, `calibrate`, `measure-a`,
//! `rss-sample`, `parse-protocol-b`, `bridge-check`). The devnull stub is
//! also built as a standalone `bench-devnull` binary so the harness can
//! launch it without subcommand dispatch noise.
//!
//! **Spec contradictions handled** (plan documents 3, all per the §4.4
//! post-fix authoritative version):
//!
//! 1. Raw distributions published side-by-side (NOT subtracted
//!    percentile-by-percentile). Baseline subtraction applies ONLY to p50
//!    location — see [`stats::baseline_corrected_p50`].
//! 2. Timer period held constant at 10ms across all contenders (Protocol
//!    B). Saturation escalates via `won't-measure`, not period adaptation.
//! 3. PID check at round boundaries only (NOT per-message) — per-message
//!    `/proc` reads perturb the measured p99.

pub mod bca;
pub mod bridge;
pub mod calibration;
pub mod cli;
pub mod cli_runtime;
pub mod protocol_b;
pub mod rss;
pub mod stats;
pub mod throughput;
pub mod warmup;

pub use bca::bca_ci;
pub use bridge::{BridgeHealth, PidMismatch};
pub use calibration::{calibration_phase, Backpressure, CalibrationOutcome};
pub use protocol_b::{parse_protocol_b_log, ProtocolBOutcome};
pub use rss::{sample_process_tree_rss, ProcessTreeRss};
pub use stats::{baseline_corrected_p50, median, nearest_rank_percentile, percentile};
pub use warmup::{check_warmup_stability, WarmupConfig, WarmupOutcome};

/// Tmpfs path conventions (spec §4.6 + §4.11 + brief 2d/2f).
/// These live in /tmp (tmpfs) so per-tick writes don't hit disk on the hot
/// path, and so the files do not bloat the repo (benchmarks/results/ is
/// reserved for aggregated stats only).
pub mod paths {
    /// Protocol B per-tick latency log: `BENCH_LATENCY <tick_id> <latency_ns>`.
    pub fn protocol_b_log(cell: &str) -> String {
        format!("/tmp/v3-protocol-b-{cell}.log")
    }
    /// Bridge subprocess PID file (written once at bridge spawn).
    pub fn bridge_pid(cell: &str) -> String {
        format!("/tmp/v3-bridge-pid-{cell}.txt")
    }
    /// M2 per-round raw samples (Protocol A round-trip latencies in ns).
    pub fn m2_samples(cell: &str, round: u32) -> String {
        format!("/tmp/v3-m2-samples-{cell}-{round}.txt")
    }
}
