// Spike 1C loadgen: persistent Rust process, HTTP keep-alive (via
// reqwest's connection pool), monotonic `std::time::Instant` per-request
// round-trip, sends N POSTs at fixed rate, computes p50/p95/p99 + records
// distribution. Spec §4.5: baseline p99 against devnull must be <1ms — if
// higher, the loadgen design is the bottleneck, escalate to e_gpt before
// Task 2.
//
// Usage: `loadgen <url> <n> <rate_per_sec>`
// Example: `loadgen http://127.0.0.1:18080/bench 1000 1000`

use std::time::{Duration, Instant};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let url = args.next().ok_or("missing url")?;
    let n: usize = args.next().ok_or("missing n")?.parse()?;
    let rate: u64 = args.next().ok_or("missing rate")?.parse()?;
    let period_ns: u64 = 1_000_000_000 / rate.max(1);

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(4)
        .tcp_nodelay(true)
        .build()?;

    let start = Instant::now();
    let mut latencies_ns: Vec<u64> = Vec::with_capacity(n);
    for i in 0..n {
        let tick = Instant::now();
        let resp = client.post(&url).body("ping").send().await?;
        let _ = resp.bytes().await?;
        let rtt = tick.elapsed();
        latencies_ns.push(rtt.as_nanos() as u64);

        // Pace.
        let target = start + Duration::from_nanos(period_ns * (i as u64 + 1));
        let now = Instant::now();
        if target > now {
            tokio::time::sleep(target - now).await;
        }
    }

    latencies_ns.sort_unstable();
    let p = |q: f64| -> u64 {
        let idx = ((latencies_ns.len() as f64 * q).ceil() as usize)
            .saturating_sub(1)
            .min(latencies_ns.len().saturating_sub(1));
        latencies_ns[idx]
    };
    let p50 = p(0.50);
    let p95 = p(0.95);
    let p99 = p(0.99);
    let p999 = p(0.999);
    let max = *latencies_ns.last().unwrap_or(&0);
    let min = *latencies_ns.first().unwrap_or(&0);
    let mean = latencies_ns.iter().sum::<u64>() / latencies_ns.len().max(1) as u64;

    println!("SPIKE_LOADGEN_RESULT n={n} rate={rate}");
    println!("SPIKE_LOADGEN_LATENCY_NS min={min} p50={p50} mean={mean} p95={p95} p99={p99} p999={p999} max={max}");
    let p99_ms = p99 as f64 / 1_000_000.0;
    println!("SPIKE_LOADGEN_P99_MS {p99_ms:.4}");

    Ok(())
}
