//! Runtime entry points for bench-loadgen subcommands.
//!
//! These functions perform the actual network and timing work. They live
//! in a separate module from `cli` (which is parsing + dispatch only)
//! so the parsing layer can be unit-tested without spinning up tokio
//! runtimes or sockets.

use std::time::{Duration, Instant};

use crate::bca::bca_ci;
use crate::calibration::{
    calibration_phase, detect_backpressure, next_rate, Backpressure, CalibrationSample,
    CALIBRATION_BATCH_SIZE, CALIBRATION_START_RATE, CALIBRATION_TOTAL_REQUESTS,
};
use crate::stats::{median, nearest_rank_percentile};
use crate::warmup::{check_warmup_stability, WarmupConfig, WarmupOutcome};

/// Error from any runtime function.
#[derive(Debug)]
pub struct RuntimeError(pub String);

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RuntimeError {}

impl From<reqwest::Error> for RuntimeError {
    fn from(e: reqwest::Error) -> Self {
        Self(format!("reqwest: {e}"))
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(e: std::io::Error) -> Self {
        Self(format!("io: {e}"))
    }
}

// ----- devnull -----

/// Start the HTTP devnull baseline stub (loopback target for Protocol A
/// calibration/measurement baseline per spec §4.4).
pub fn run_devnull(port: u16) -> Result<(), RuntimeError> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()?;
    rt.block_on(devnull_serve(port))?;
    Ok(())
}

async fn devnull_serve(port: u16) -> Result<(), RuntimeError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let addr: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
    let listener = TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    // Marker line: harness waits on this before starting the loadgen.
    println!("BENCH_DEVNULL_READY {}", bound.port());
    use std::io::Write;
    let _ = std::io::stdout().flush();

    let resp = b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\nconnection: keep-alive\r\n\r\nok";
    loop {
        let (mut socket, _peer) = listener.accept().await?;
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                // Read until end-of-headers, then drain Content-Length body.
                let mut total = 0usize;
                let mut header_end: Option<usize> = None;
                let mut body_remaining: usize = 0;
                while total < buf.len() {
                    let n = match socket.read(&mut buf[total..]).await {
                        Ok(0) => return,
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    total += n;
                    if header_end.is_none() {
                        if let Some(pos) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
                            let he = pos + 4;
                            header_end = Some(he);
                            let header_str = std::str::from_utf8(&buf[..he]).unwrap_or("");
                            let cl = parse_content_length(header_str).unwrap_or(0);
                            let body_already = total.saturating_sub(he);
                            body_remaining = cl.saturating_sub(body_already);
                        }
                    } else {
                        body_remaining = body_remaining.saturating_sub(n);
                    }
                    if let Some(he) = header_end {
                        if total >= he + body_remaining {
                            break;
                        }
                    }
                }
                if socket.write_all(resp).await.is_err() {
                    return;
                }
            }
        });
    }
}

fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.split("\r\n") {
        let mut parts = line.splitn(2, ':');
        let name = parts.next()?.trim();
        let value = parts.next()?.trim();
        if name.eq_ignore_ascii_case("content-length") {
            return value.parse().ok();
        }
    }
    None
}

// ----- calibrate -----

/// Run a Protocol A calibration phase against the given URL.
///
/// Sends 200 requests total in batches of 25 at increasing rates
/// (10 → 20 → ... → until backpressure or budget exhausted). Returns
/// the max sustainable rate (last rate that completed its batch without
/// backpressure).
pub fn run_calibrate(url: &str, _timeout_secs: u64) -> Result<u32, RuntimeError> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()?;
    rt.block_on(calibrate_async(url))
}

async fn calibrate_async(url: &str) -> Result<u32, RuntimeError> {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(4)
        .tcp_nodelay(true)
        .build()?;
    let mut batches: Vec<(u32, Vec<CalibrationSample>)> = Vec::new();
    let mut rate = CALIBRATION_START_RATE;
    let mut total_sent = 0u32;
    while total_sent < CALIBRATION_TOTAL_REQUESTS {
        let batch = send_batch(&client, url, rate, CALIBRATION_BATCH_SIZE).await;
        let batch_count = batch.len() as u32;
        batches.push((rate, batch));
        total_sent = total_sent.saturating_add(batch_count);
        // If this batch hit backpressure, stop here.
        if let Some((_, last)) = batches.last() {
            if detect_backpressure(rate, last).is_some() {
                break;
            }
        }
        rate = next_rate(rate);
    }
    let outcome = calibration_phase(&batches);
    // Diagnostic line for the harness log.
    let bp_str = match outcome.backpressure {
        Some(Backpressure::P99Threshold {
            observed_p99_ns, ..
        }) => {
            format!("p99_threshold observed_p99_ns={observed_p99_ns}")
        }
        Some(Backpressure::HttpThrottled { .. }) => "http_throttled".to_string(),
        Some(Backpressure::ConnectionFailed { .. }) => "connection_failed".to_string(),
        None => "none".to_string(),
    };
    eprintln!(
        "calibration: max_sustainable={} backpressure={} total_sent={}",
        outcome.max_sustainable_rate_per_sec, bp_str, outcome.total_requests_sent
    );
    Ok(outcome.max_sustainable_rate_per_sec)
}

async fn send_batch(
    client: &reqwest::Client,
    url: &str,
    rate_per_sec: u32,
    n: u32,
) -> Vec<CalibrationSample> {
    let mut out = Vec::with_capacity(n as usize);
    if rate_per_sec == 0 {
        return out;
    }
    let period_ns: u64 = 1_000_000_000 / (rate_per_sec as u64);
    let start = Instant::now();
    for i in 0..n {
        let tick = Instant::now();
        let sample = match client.post(url).body("ping").send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let _ = resp.bytes().await;
                let lat = tick.elapsed().as_nanos() as u64;
                if status == 429 || status == 503 {
                    CalibrationSample::http_throttled()
                } else {
                    CalibrationSample::ok(lat)
                }
            }
            Err(_) => CalibrationSample::conn_failed(),
        };
        out.push(sample);
        // Pace: sleep to maintain the target rate.
        let target = start + Duration::from_nanos(period_ns * (i as u64 + 1));
        let now = Instant::now();
        if target > now {
            tokio::time::sleep(target - now).await;
        }
    }
    out
}

// ----- measure-a -----

/// Run Protocol A measurement: warmup → N rounds × N samples.
///
/// `output_path` (if set) receives one line per sample: `<rtt_ns>` for
/// Protocol A. The harness parses this for downstream aggregation + BCa CI.
///
/// `bci_resamples` and `bci_seed` control per-round BCa CI computation
/// (brief 2h: 2000 resamples, 95% CI on per-round p50; seed makes the
/// bootstrap deterministic for a given (round, samples) input).
#[allow(clippy::too_many_arguments)]
pub fn run_measure_a(
    url: &str,
    rate_per_sec: u32,
    samples_per_round: u32,
    rounds: u32,
    warmup_cfg: WarmupConfig,
    bci_resamples: usize,
    bci_seed: u64,
    output_path: Option<&str>,
) -> Result<(), RuntimeError> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()?;
    rt.block_on(measure_a_async(
        url,
        rate_per_sec,
        samples_per_round,
        rounds,
        warmup_cfg,
        bci_resamples,
        bci_seed,
        output_path,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn measure_a_async(
    url: &str,
    rate_per_sec: u32,
    samples_per_round: u32,
    rounds: u32,
    warmup_cfg: WarmupConfig,
    bci_resamples: usize,
    bci_seed: u64,
    output_path: Option<&str>,
) -> Result<(), RuntimeError> {
    if rate_per_sec == 0 {
        return Err(RuntimeError("rate_per_sec must be > 0".to_string()));
    }
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(4)
        .tcp_nodelay(true)
        .build()?;

    // --- Warmup ---
    let warmup_start = Instant::now();
    let warmup_samples = warmup_drive(&client, url, rate_per_sec, &warmup_cfg).await?;
    let warmup_elapsed = warmup_start.elapsed().as_nanos() as u64;
    let warmup_outcome = check_warmup_stability(&warmup_samples, warmup_elapsed, &warmup_cfg);
    match warmup_outcome {
        WarmupOutcome::Stable {
            p50_first_half_ns,
            p50_second_half_ns,
            ..
        } => {
            eprintln!(
                "warmup: stable p50_first={}ns p50_second={}ns",
                p50_first_half_ns, p50_second_half_ns
            );
        }
        WarmupOutcome::FailedStability { reason, .. } => {
            return Err(RuntimeError(format!("warmup failed-stability: {reason:?}")));
        }
    }

    // --- Measurement rounds ---
    let mut all_round_p99s: Vec<u64> = Vec::with_capacity(rounds as usize);
    let mut all_round_p50s: Vec<u64> = Vec::with_capacity(rounds as usize);
    let mut all_round_p95s: Vec<u64> = Vec::with_capacity(rounds as usize);
    let mut all_round_bca_lo: Vec<u64> = Vec::with_capacity(rounds as usize);
    let mut all_round_bca_hi: Vec<u64> = Vec::with_capacity(rounds as usize);
    let mut out_file = if let Some(p) = output_path {
        Some(std::fs::File::create(p)?)
    } else {
        None
    };

    for r in 0..rounds {
        let mut samples: Vec<u64> = Vec::with_capacity(samples_per_round as usize);
        let period_ns: u64 = 1_000_000_000 / (rate_per_sec as u64);
        let start = Instant::now();
        for i in 0..samples_per_round {
            let tick = Instant::now();
            if let Ok(resp) = client.post(url).body("ping").send().await {
                let _ = resp.bytes().await;
                let rtt_ns = tick.elapsed().as_nanos() as u64;
                samples.push(rtt_ns);
                if let Some(f) = out_file.as_mut() {
                    use std::io::Write;
                    let _ = writeln!(f, "{rtt_ns}");
                }
            }
            let target = start + Duration::from_nanos(period_ns * (i as u64 + 1));
            let now = Instant::now();
            if target > now {
                tokio::time::sleep(target - now).await;
            }
        }
        let p50 = nearest_rank_percentile(&samples, 0.50).unwrap_or(0);
        let p95 = nearest_rank_percentile(&samples, 0.95).unwrap_or(0);
        let p99 = nearest_rank_percentile(&samples, 0.99).unwrap_or(0);
        // Per-round BCa CI on the p50 (brief 2h). The seed is offset by
        // the round index so different rounds don't share a resample
        // stream (would give artificially tight inter-round CIs).
        let round_seed = bci_seed.wrapping_add(r as u64);
        let (bca_lo, bca_hi) = match bca_ci(&samples, bci_resamples, round_seed) {
            Some(ci) => (ci.lower, ci.upper),
            None => (0, 0),
        };
        all_round_p50s.push(p50);
        all_round_p95s.push(p95);
        all_round_p99s.push(p99);
        all_round_bca_lo.push(bca_lo);
        all_round_bca_hi.push(bca_hi);
        eprintln!(
            "{}",
            format_round_line(r, samples.len(), p50, p95, p99, bca_lo, bca_hi)
        );
    }

    let median_p99 = median(&all_round_p99s).unwrap_or(0);
    let median_p50 = median(&all_round_p50s).unwrap_or(0);
    let median_p95 = median(&all_round_p95s).unwrap_or(0);
    println!(
        "{}",
        format_result_line(
            rounds,
            median_p50,
            median_p95,
            median_p99,
            &all_round_p99s,
            &all_round_bca_lo,
            &all_round_bca_hi
        )
    );

    Ok(())
}

/// Format a single per-round line (stderr) for `measure-a`.
///
/// Format is pinned by `format_round_line_round_trips_through_expected_fields`
/// in the unit tests below. The harness (and downstream tools) parse this
/// shape; changes here are a breaking contract change.
fn format_round_line(
    round: u32,
    n: usize,
    p50_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
    bca_lo_ns: u64,
    bca_hi_ns: u64,
) -> String {
    format!(
        "measure-a: round {round} n={n} p50={p50_ns}ns p95={p95_ns}ns p99={p99_ns}ns bca_lo={bca_lo_ns}ns bca_hi={bca_hi_ns}ns"
    )
}

/// Format the final `BENCH_MEASURE_A_RESULT` summary line (stdout).
///
/// Format is pinned by `format_result_line_round_trips_through_expected_fields`
/// in the unit tests below. The harness parses this single-line result.
fn format_result_line(
    rounds: u32,
    median_p50_ns: u64,
    median_p95_ns: u64,
    median_p99_ns: u64,
    round_p99s_ns: &[u64],
    round_bca_lo_ns: &[u64],
    round_bca_hi_ns: &[u64],
) -> String {
    format!(
        "BENCH_MEASURE_A_RESULT rounds={rounds} median_p50_ns={median_p50_ns} median_p95_ns={median_p95_ns} median_p99_ns={median_p99_ns} round_p99s_ns={round_p99s_ns:?} round_bca_lo_ns={round_bca_lo_ns:?} round_bca_hi_ns={round_bca_hi_ns:?}"
    )
}

async fn warmup_drive(
    client: &reqwest::Client,
    url: &str,
    rate_per_sec: u32,
    cfg: &WarmupConfig,
) -> Result<Vec<u64>, RuntimeError> {
    let mut samples: Vec<u64> = Vec::with_capacity(cfg.max_messages as usize);
    let period_ns: u64 = 1_000_000_000 / (rate_per_sec.max(1) as u64);
    let start = Instant::now();
    let deadline = Duration::from_secs(cfg.max_time_seconds as u64);
    for i in 0..cfg.max_messages {
        if start.elapsed() >= deadline {
            break;
        }
        let tick = Instant::now();
        if let Ok(resp) = client.post(url).body("ping").send().await {
            let _ = resp.bytes().await;
            let rtt_ns = tick.elapsed().as_nanos() as u64;
            samples.push(rtt_ns);
        }
        let target = start + Duration::from_nanos(period_ns * (i as u64 + 1));
        let now = Instant::now();
        if target > now {
            tokio::time::sleep(target - now).await;
        }
    }
    Ok(samples)
}

// ----- measure-throughput (M3 sustained load) -----

/// Run sustained-throughput measurement (M3) against the given URL.
///
/// Spawns `workers` concurrent client loops, each in a tight `send().await`
/// loop with a shared `reqwest::Client` (pool size = `workers`). A
/// bucket-collector task wakes every wall-clock second, swaps the
/// per-second atomic counter to 0, and pushes the previous second's
/// count into `buckets` (after the warmup window has elapsed). After
/// the configured duration, workers are cancelled via
/// [`tokio_util::sync::CancellationToken`] and the result is written
/// to `--out` (if set) or stdout as a JSON object.
///
/// The `reqwest::Client` is shared via cheap `Arc` clone so all workers
/// draw from one connection pool — this is the load shape that
/// saturation testing requires (per-worker clients would measure
/// per-worker pool, not aggregate capacity).
///
/// Per-response body drain is **required before counting** (r_glm
/// review fix): without draining, the connection sits "in flight" in
/// the pool and workers back-pressure on the next iteration instead
/// of issuing a new request.
pub fn run_measure_throughput(
    url: &str,
    duration_secs: u64,
    warmup_secs: u64,
    workers: usize,
    output_path: Option<&str>,
) -> Result<(), RuntimeError> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(workers.max(2))
        .build()?;
    rt.block_on(run_throughput_async(
        url,
        duration_secs,
        warmup_secs,
        workers,
        output_path,
    ))
}

async fn run_throughput_async(
    url: &str,
    duration_secs: u64,
    warmup_secs: u64,
    workers: usize,
    output_path: Option<&str>,
) -> Result<(), RuntimeError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    // ONE shared client. Workers clone the Arc handle (cheap), so they
    // share the connection pool. Without this, each worker would have
    // its own pool of size 1 and we'd measure per-worker capacity, not
    // the aggregate the spec requires.
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(workers)
        .pool_idle_timeout(Duration::from_secs(60))
        .tcp_nodelay(true)
        .build()?;

    // Per-second counter is swapped to 0 by the bucket collector; workers
    // accumulate against it. non_2xx and error counters are summed
    // across the whole measurement window for the error-rate calc.
    let current_count = Arc::new(AtomicU64::new(0));
    let non_2xx_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let cancel = CancellationToken::new();

    let mut worker_handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let client = client.clone();
        let url = url.to_string();
        let current_count = current_count.clone();
        let non_2xx_count = non_2xx_count.clone();
        let error_count = error_count.clone();
        let cancel = cancel.clone();
        worker_handles.push(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        return;
                    }
                    result = client.post(&url).body("bench").send() => {
                        match result {
                            Ok(response) => {
                                let status = response.status().as_u16();
                                // CRITICAL: drain the response body BEFORE
                                // counting. Without this, the connection
                                // remains "in flight" in the pool and
                                // workers back-pressure on the next send
                                // instead of issuing a new request —
                                // collapsing throughput to the pool
                                // capacity. (r_glm review fix.)
                                let _ = response.bytes().await;
                                if (200..300).contains(&status) {
                                    current_count.fetch_add(1, Ordering::Relaxed);
                                } else {
                                    non_2xx_count.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(_) => {
                                error_count.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
        }));
    }

    // Bucket collector: sleep 1s, swap the per-second counter, push
    // into `buckets` once we're past warmup_secs. Warmup samples are
    // discarded (spec §3.4 — "first N seconds discarded"). Non-2xx and
    // error counters are NOT windowed: we sum them across the whole
    // measurement window for the error-rate calculation.
    let total_secs = warmup_secs.saturating_add(duration_secs);
    let mut buckets: Vec<u64> = Vec::with_capacity(duration_secs as usize);
    let mut window_non_2xx: u64 = 0;
    let mut window_errors: u64 = 0;
    for sec in 0..total_secs {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let count = current_count.swap(0, Ordering::Relaxed);
        let non_2xx = non_2xx_count.swap(0, Ordering::Relaxed);
        let errors = error_count.swap(0, Ordering::Relaxed);
        if sec >= warmup_secs {
            buckets.push(count);
            window_non_2xx = window_non_2xx.saturating_add(non_2xx);
            window_errors = window_errors.saturating_add(errors);
        }
    }

    // Cancel all workers and wait for them to drain. `cancel()` wakes
    // every `cancelled()` future; the worker loops return on the next
    // select! poll.
    cancel.cancel();
    for h in worker_handles {
        let _ = h.await;
    }

    let result = crate::throughput::aggregate_throughput(buckets, window_non_2xx, window_errors);

    // JSON output. Per the brief, downstream tools consume the full
    // `ThroughputResult` plus the run parameters for context. The
    // per-second buckets let the report plot throughput over time.
    let json = serde_json::json!({
        "url": url,
        "duration_secs": duration_secs,
        "warmup_secs": warmup_secs,
        "workers": workers,
        "mean_msgs_per_sec": result.mean_msgs_per_sec,
        "p50_msgs_per_sec": result.p50_msgs_per_sec,
        "min_msgs_per_sec": result.min_msgs_per_sec,
        "cv": result.cv,
        "half_life_ratio": result.half_life_ratio,
        "total_2xx": result.total_2xx,
        "total_non_2xx": result.total_non_2xx,
        "total_errors": result.total_errors,
        "total_attempts": result.total_attempts,
        "error_rate_pct": result.error_rate_pct,
        "per_second_buckets": result.per_second_buckets,
        "is_steady": crate::throughput::is_steady(&result),
    });

    let json_str = serde_json::to_string_pretty(&json)
        .unwrap_or_else(|e| format!("{{\"error\":\"serialize failed: {e}\"}}"));

    if let Some(p) = output_path {
        std::fs::write(p, &json_str)?;
    } else {
        println!("{json_str}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the per-round `measure-a` line shape. The harness + downstream
    /// tools (e.g. report generators) parse this exact field order. Brief
    /// I1 (reviewer fix) requires this contract to be locked in.
    #[test]
    fn format_round_line_round_trips_through_expected_fields() {
        let line = format_round_line(0, 10_000, 87_233, 120_385, 146_664, 85_000, 90_000);
        assert_eq!(
            line,
            "measure-a: round 0 n=10000 p50=87233ns p95=120385ns p99=146664ns bca_lo=85000ns bca_hi=90000ns"
        );
    }

    /// Pin the `BENCH_MEASURE_A_RESULT` summary line shape (the single
    /// line the harness greps for to capture per-cell measurement
    /// results). Field order, separators, and Debug-formatted Vec
    /// representation must remain stable.
    #[test]
    fn format_result_line_round_trips_through_expected_fields() {
        let p99s = vec![146_664_u64];
        let bca_lo = vec![85_000_u64];
        let bca_hi = vec![90_000_u64];
        let line = format_result_line(1, 87_233, 120_385, 146_664, &p99s, &bca_lo, &bca_hi);
        assert_eq!(
            line,
            "BENCH_MEASURE_A_RESULT rounds=1 median_p50_ns=87233 median_p95_ns=120385 median_p99_ns=146664 round_p99s_ns=[146664] round_bca_lo_ns=[85000] round_bca_hi_ns=[90000]"
        );
    }

    /// When BCa has no samples (degenerate input), the formatters should
    /// still emit the expected fields with zero values. This locks in the
    /// `match None` branch behavior.
    #[test]
    fn format_round_line_emits_zero_bca_fields_on_degenerate_input() {
        let line = format_round_line(2, 0, 0, 0, 0, 0, 0);
        assert_eq!(
            line,
            "measure-a: round 2 n=0 p50=0ns p95=0ns p99=0ns bca_lo=0ns bca_hi=0ns"
        );
    }
}
