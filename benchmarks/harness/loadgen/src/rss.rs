//! Process-tree RSS sampling (spec §4.9, brief 2g).
//!
//! For T4a/T4b rust-camel fixtures, the bridge subprocess holds the
//! XSLT/XSD engine. Parent-process RSS alone misses bridge-process
//! memory. The harness must aggregate parent + descendants.
//!
//! **Approach (b) — simultaneous peak** is mandated by spec §4.9 as the
//! headline number: sample VmRSS for parent + descendants every 10ms
//! during the M1 measurement window; per-sample, sum across the process
//! tree; report **max of per-sample sums** (the true simultaneous peak).
//!
//! Summed-VmHWM (approach a) may be reported as a secondary number for
//! transparency.
//!
//! Discovery via `/proc/<pid>/task/<tid>/children` recursive walk. The
//! walk is read-only and does not perturb the measured p99 (it happens
//! on the harness side, not the contender's hot path).
//!
//! ## Plant/seed separation
//!
//! The actual `/proc` walk is platform-specific and hard to unit test.
//! To keep this module unit-testable, the `/proc` reads are abstracted
//! behind a [`ProcReader`] trait. Production code uses
//! [`LinuxProcReader`]; tests use [`FakeProcReader`] to construct
//! arbitrary process trees.

use std::collections::HashMap;

/// A process-tree sample at a single point in time.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProcessTreeSample {
    /// Wall-clock offset of this sample from the start of sampling, in ns.
    pub sample_offset_ns: u64,
    /// Sum of VmRSS across the entire process tree, in KiB.
    pub summed_vmrss_kib: u64,
    /// Sum of VmHWM across the entire process tree, in KiB (secondary
    /// number, reported for transparency).
    pub summed_vmhwm_kib: u64,
    /// Number of processes sampled (tree size at this sample).
    pub process_count: usize,
}

/// Aggregated result across all samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessTreeRss {
    /// **Headline number** (spec §4.9): max of per-sample Σ VmRSS.
    /// This is the true simultaneous peak.
    pub max_summed_vmrss_kib: u64,
    /// **Secondary number**: max of per-sample Σ VmHWM (upper bound
    /// on simultaneous peak; per-process peaks may not coincide in
    /// time).
    pub max_summed_vmhwm_kib: u64,
    /// Total samples collected.
    pub n_samples: usize,
    /// Per-process tree size at the peak sample (for diagnostics).
    pub peak_process_count: usize,
    /// Wall-clock duration of the sampling window, in ns.
    pub duration_ns: u64,
    /// All per-sample sums (for plotting in the report).
    pub samples: Vec<ProcessTreeSample>,
}

/// Abstraction over `/proc` reads so the sampling logic is unit-testable.
///
/// Each method returns `Option` to model "process gone between walk and
/// read" — the caller skips vanished PIDs.
pub trait ProcReader {
    /// Return the immediate child PIDs of the given parent.
    /// On Linux, reads `/proc/<pid>/task/<tid>/children`.
    fn child_pids(&self, pid: u32) -> Vec<u32>;

    /// Return `(VmRSS in KiB, VmHWM in KiB)` for the given PID.
    /// On Linux, parses `/proc/<pid>/status`.
    fn vm_rss_hwm_kib(&self, pid: u32) -> Option<(u64, u64)>;
}

/// Walk the process tree rooted at `root_pid`, collecting all reachable
/// descendant PIDs (including `root_pid` itself).
///
/// Cycle-safe via a visited-set: malformed `/proc` states that produce
/// cycles (e.g., a process appearing as its own ancestor) are handled
/// gracefully.
pub fn collect_tree_pids<R: ProcReader>(reader: &R, root_pid: u32) -> Vec<u32> {
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut stack: Vec<u32> = vec![root_pid];
    let mut order: Vec<u32> = Vec::new();
    while let Some(pid) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }
        order.push(pid);
        for child in reader.child_pids(pid) {
            if !visited.contains(&child) {
                stack.push(child);
            }
        }
    }
    order
}

/// Take a single sample of the process tree rooted at `root_pid`:
/// sum VmRSS + VmHWM across all reachable processes.
pub fn sample_once<R: ProcReader>(reader: &R, root_pid: u32, offset_ns: u64) -> ProcessTreeSample {
    let pids = collect_tree_pids(reader, root_pid);
    let mut sum_rss = 0u64;
    let mut sum_hwm = 0u64;
    let mut counted = 0usize;
    for pid in &pids {
        if let Some((rss, hwm)) = reader.vm_rss_hwm_kib(*pid) {
            sum_rss = sum_rss.saturating_add(rss);
            sum_hwm = sum_hwm.saturating_add(hwm);
            counted += 1;
        }
    }
    ProcessTreeSample {
        sample_offset_ns: offset_ns,
        summed_vmrss_kib: sum_rss,
        summed_vmhwm_kib: sum_hwm,
        process_count: counted,
    }
}

/// Sample the process tree at fixed cadence for a fixed duration.
///
/// `interval_ns` is the target time between samples; `duration_ns` is
/// the total sampling window. The reader is invoked at each tick.
///
/// **Note**: this is the pure logic — actual timing is the caller's
/// responsibility (production callers use `tokio::time::interval`). This
/// function takes a list of pre-computed sample offsets, allowing tests
/// to drive the sampler deterministically.
pub fn sample_process_tree_rss<R: ProcReader>(
    reader: &R,
    root_pid: u32,
    sample_offsets_ns: &[u64],
    duration_ns: u64,
) -> ProcessTreeRss {
    let mut samples: Vec<ProcessTreeSample> = Vec::with_capacity(sample_offsets_ns.len());
    for &offset in sample_offsets_ns {
        samples.push(sample_once(reader, root_pid, offset));
    }
    aggregate_samples(samples, duration_ns)
}

/// Aggregate a vector of per-sample sums into the headline result.
///
/// Exposed publicly so tests can drive aggregation independently of the
/// sampling cadence.
pub fn aggregate_samples(samples: Vec<ProcessTreeSample>, duration_ns: u64) -> ProcessTreeRss {
    let mut max_rss = 0u64;
    let mut max_hwm = 0u64;
    let mut peak_count = 0usize;
    for s in &samples {
        if s.summed_vmrss_kib > max_rss {
            max_rss = s.summed_vmrss_kib;
            peak_count = s.process_count;
        }
        if s.summed_vmhwm_kib > max_hwm {
            max_hwm = s.summed_vmhwm_kib;
        }
    }
    // If two samples tie on rss but differ on count, keep the first max.
    // (Edge case — does not affect the headline number.)
    ProcessTreeRss {
        max_summed_vmrss_kib: max_rss,
        max_summed_vmhwm_kib: max_hwm,
        n_samples: samples.len(),
        peak_process_count: peak_count,
        duration_ns,
        samples,
    }
}

// ----- Linux production implementation. -----

/// Production `/proc` reader for Linux.
#[cfg(target_os = "linux")]
#[derive(Default)]
pub struct LinuxProcReader;

#[cfg(target_os = "linux")]
impl LinuxProcReader {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "linux")]
impl ProcReader for LinuxProcReader {
    fn child_pids(&self, pid: u32) -> Vec<u32> {
        // `/proc/<pid>/task/<tid>/children` is per-thread: only the
        // children forked by THAT specific thread. For multi-threaded
        // processes (e.g., JVMs spawning bridge subprocesses from worker
        // threads), reading only the main thread (`tid == pid`) misses
        // descendants. Walk all TIDs in /proc/<pid>/task/ and aggregate.
        let task_dir = format!("/proc/{pid}/task");
        let Ok(entries) = std::fs::read_dir(&task_dir) else {
            return Vec::new();
        };
        let mut children: Vec<u32> = Vec::new();
        for entry in entries.flatten() {
            let tid_name = entry.file_name();
            let Some(tid_s) = tid_name.to_str() else {
                continue;
            };
            let Ok(tid) = tid_s.parse::<u32>() else {
                continue;
            };
            let path = format!("/proc/{pid}/task/{tid}/children");
            if let Ok(content) = std::fs::read_to_string(&path) {
                for tok in content.split_whitespace() {
                    if let Ok(c) = tok.parse::<u32>() {
                        children.push(c);
                    }
                }
            }
        }
        children
    }

    fn vm_rss_hwm_kib(&self, pid: u32) -> Option<(u64, u64)> {
        let path = format!("/proc/{pid}/status");
        let content = std::fs::read_to_string(&path).ok()?;
        let mut rss: Option<u64> = None;
        let mut hwm: Option<u64> = None;
        for line in content.lines() {
            // Lines look like: "VmRSS:     12345 kB"
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                rss = parse_kb_value(rest);
            } else if let Some(rest) = line.strip_prefix("VmHWM:") {
                hwm = parse_kb_value(rest);
            }
            if rss.is_some() && hwm.is_some() {
                break;
            }
        }
        match (rss, hwm) {
            (Some(r), Some(h)) => Some((r, h)),
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
fn parse_kb_value(rest: &str) -> Option<u64> {
    // "  12345 kB" → 12345
    rest.split_whitespace()
        .next()
        .and_then(|s| s.parse::<u64>().ok())
}

// ----- Test double. -----

/// In-memory fake `/proc` reader for unit tests. Maps PID → (VmRSS,
/// VmHWM) and PID → child PIDs.
#[derive(Debug, Default, Clone)]
pub struct FakeProcReader {
    pub rss_hwm: HashMap<u32, (u64, u64)>,
    pub children: HashMap<u32, Vec<u32>>,
}

impl FakeProcReader {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_process(mut self, pid: u32, rss_kib: u64, hwm_kib: u64) -> Self {
        self.rss_hwm.insert(pid, (rss_kib, hwm_kib));
        self
    }
    pub fn with_child(mut self, parent: u32, child: u32) -> Self {
        self.children.entry(parent).or_default().push(child);
        self
    }
}

impl ProcReader for FakeProcReader {
    fn child_pids(&self, pid: u32) -> Vec<u32> {
        self.children.get(&pid).cloned().unwrap_or_default()
    }
    fn vm_rss_hwm_kib(&self, pid: u32) -> Option<(u64, u64)> {
        self.rss_hwm.get(&pid).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_tree_pids_single_process() {
        let reader = FakeProcReader::new().with_process(1, 1000, 2000);
        let pids = collect_tree_pids(&reader, 1);
        assert_eq!(pids, vec![1]);
    }

    #[test]
    fn collect_tree_pids_parent_and_child() {
        let reader = FakeProcReader::new()
            .with_process(1, 1000, 2000)
            .with_process(2, 500, 600)
            .with_child(1, 2);
        let pids = collect_tree_pids(&reader, 1);
        assert_eq!(pids.len(), 2);
        assert!(pids.contains(&1));
        assert!(pids.contains(&2));
    }

    #[test]
    fn collect_tree_pids_grandchild_chain() {
        // 1 → 2 → 3
        let reader = FakeProcReader::new()
            .with_process(1, 1000, 2000)
            .with_process(2, 500, 600)
            .with_process(3, 250, 300)
            .with_child(1, 2)
            .with_child(2, 3);
        let pids = collect_tree_pids(&reader, 1);
        assert_eq!(pids.len(), 3);
    }

    #[test]
    fn collect_tree_pids_cycle_safe() {
        // 1 → 2 → 1 (cycle). Should not infinite-loop; visited-set guards.
        let reader = FakeProcReader::new()
            .with_process(1, 1000, 2000)
            .with_process(2, 500, 600)
            .with_child(1, 2)
            .with_child(2, 1);
        let pids = collect_tree_pids(&reader, 1);
        assert_eq!(pids.len(), 2);
    }

    #[test]
    fn sample_once_sums_rss_across_tree() {
        // Parent (1) RSS=1000KiB; Child (2) RSS=500KiB → sum = 1500.
        let reader = FakeProcReader::new()
            .with_process(1, 1000, 1500)
            .with_process(2, 500, 700)
            .with_child(1, 2);
        let s = sample_once(&reader, 1, 0);
        assert_eq!(s.summed_vmrss_kib, 1500);
        assert_eq!(s.summed_vmhwm_kib, 2200);
        assert_eq!(s.process_count, 2);
    }

    #[test]
    fn sample_once_skips_vanished_processes() {
        // Parent's child PID has no entry in rss_hwm (vanished between
        // walk and read) — only the parent contributes.
        let reader = FakeProcReader::new()
            .with_process(1, 1000, 1500)
            .with_child(1, 2); // child 2 has no rss_hwm entry
        let s = sample_once(&reader, 1, 0);
        assert_eq!(s.summed_vmrss_kib, 1000);
        assert_eq!(s.process_count, 1);
    }

    #[test]
    fn sample_once_saturating_add_on_huge_values() {
        // Defensive: two processes with u64::MAX RSS should not overflow.
        let reader = FakeProcReader::new()
            .with_process(1, u64::MAX, u64::MAX)
            .with_process(2, u64::MAX, u64::MAX)
            .with_child(1, 2);
        let s = sample_once(&reader, 1, 0);
        assert_eq!(s.summed_vmrss_kib, u64::MAX);
    }

    #[test]
    fn aggregate_picks_max_summed_rss_across_samples() {
        // Simulate samples at different points in time as a process
        // ramps up then shrinks.
        let samples = vec![
            ProcessTreeSample {
                sample_offset_ns: 0,
                summed_vmrss_kib: 1000,
                summed_vmhwm_kib: 1100,
                process_count: 1,
            },
            ProcessTreeSample {
                sample_offset_ns: 10_000_000, // 10ms
                summed_vmrss_kib: 3000,
                summed_vmhwm_kib: 3100,
                process_count: 2,
            },
            ProcessTreeSample {
                sample_offset_ns: 20_000_000, // 20ms
                summed_vmrss_kib: 2500,
                summed_vmhwm_kib: 2600,
                process_count: 2,
            },
        ];
        let result = aggregate_samples(samples, 30_000_000);
        assert_eq!(result.max_summed_vmrss_kib, 3000); // peak at sample 2
        assert_eq!(result.max_summed_vmhwm_kib, 3100);
        assert_eq!(result.peak_process_count, 2);
        assert_eq!(result.n_samples, 3);
    }

    #[test]
    fn aggregate_empty_samples_returns_zero() {
        let result = aggregate_samples(vec![], 0);
        assert_eq!(result.max_summed_vmrss_kib, 0);
        assert_eq!(result.n_samples, 0);
    }

    #[test]
    fn sample_process_tree_rss_drives_via_offsets() {
        // Parent + child; child grows over time.
        let reader_t0 = FakeProcReader::new()
            .with_process(1, 1000, 1100)
            .with_process(2, 500, 600)
            .with_child(1, 2);
        // The current API takes a single reader — model growth by
        // pre-computing what the aggregate would see. This test asserts
        // the wiring works for the simple constant-tree case.
        let result =
            sample_process_tree_rss(&reader_t0, 1, &[0, 10_000_000, 20_000_000], 30_000_000);
        assert_eq!(result.max_summed_vmrss_kib, 1500);
        assert_eq!(result.peak_process_count, 2);
        assert_eq!(result.n_samples, 3);
    }

    #[test]
    fn headline_uses_simultaneous_peak_not_upper_bound() {
        // Spec §4.9: max-of-per-sample-Σ-RSS is the headline, NOT
        // max-of-Σ-VmHWM. The two diverge when per-process peaks don't
        // coincide in time.
        //
        // Sample 1: parent peaks (RSS=1000, HWM=1000); child small (RSS=100, HWM=1000).
        // Sample 2: parent small (RSS=100); child peaks (RSS=1000, HWM=1000).
        //   → Simultaneous peak = 1000 + 100 = 1100 (sample 1 OR sample 2).
        //   → Σ-VmHWM upper bound = 1000 + 1000 = 2000.
        let samples = vec![
            ProcessTreeSample {
                sample_offset_ns: 0,
                summed_vmrss_kib: 1100,
                summed_vmhwm_kib: 2000,
                process_count: 2,
            },
            ProcessTreeSample {
                sample_offset_ns: 10_000_000,
                summed_vmrss_kib: 1100,
                summed_vmhwm_kib: 2000,
                process_count: 2,
            },
        ];
        let result = aggregate_samples(samples, 20_000_000);
        // Headline is simultaneous peak.
        assert_eq!(result.max_summed_vmrss_kib, 1100);
        // Upper bound is reported as secondary for transparency.
        assert_eq!(result.max_summed_vmhwm_kib, 2000);
        assert_ne!(result.max_summed_vmrss_kib, result.max_summed_vmhwm_kib);
    }
}
