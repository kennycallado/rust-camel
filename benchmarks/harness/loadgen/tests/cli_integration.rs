//! Integration tests for the bench-loadgen binary's CLI surface.
//!
//! These tests spawn the actual `bench-loadgen` binary (built at the
//! workspace root target dir) and verify end-to-end behaviour for the
//! subcommands that the harness shell invokes. Pure-statistical logic
//! has unit tests in src/; these tests focus on the wiring + CLI
//! contract + I/O paths.
//!
//! Per the brief verification checklist:
//! - #4: bridge generation tracking — simulate PID change → round invalidates
//! - #5: process-tree RSS sampling — spawn dummy child → aggregate includes it

#![cfg(target_os = "linux")]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// Locate the bench-loadgen binary in the workspace target dir.
fn loadgen_bin() -> PathBuf {
    // CARGO_MANIFEST_DIR for this package = <workspace>/benchmarks/harness/loadgen.
    // Walk up 3 levels to reach the workspace root.
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    let workspace_root = std::path::Path::new(&manifest_dir)
        .ancestors()
        .nth(3) // .../benchmarks/harness/loadgen -> .../<workspace root>
        .unwrap_or_else(|| panic!("could not resolve workspace root from {manifest_dir}"));
    // Try CARGO_TARGET_DIR override first, then default workspace target.
    let candidate_dirs: Vec<PathBuf> = match std::env::var("CARGO_TARGET_DIR") {
        Ok(dir) => vec![PathBuf::from(&dir)],
        Err(_) => vec![
            workspace_root.join("target"),
            // Shared CI cache (NixOS):
            std::path::Path::new("/home/shared/rust-camel-target").to_path_buf(),
        ],
    };
    for dir in &candidate_dirs {
        for profile in ["debug", "release"] {
            let bin = dir.join(profile).join("bench-loadgen");
            if bin.exists() {
                return bin;
            }
        }
        // Top-level (no profile subdir) — common for `cargo build` without --release.
        let bin = dir.join("bench-loadgen");
        if bin.exists() {
            return bin;
        }
    }
    panic!(
        "bench-loadgen binary not found in any of {:?}. Build it first: \
         env -u CARGO_TARGET_DIR cargo build -p bench-loadgen",
        candidate_dirs
    );
}

/// Run bench-loadgen with the given args, returning (exit_status, stdout, stderr).
///
/// Args use the `--key=value` form (the parser does not support
/// `--key value` space-separated form).
fn run_loadgen(args: &[&str]) -> (std::process::Output, String, String) {
    let bin = loadgen_bin();
    let output = Command::new(&bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", bin.display()));
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output, stdout, stderr)
}

/// Convenience: build a `--key=value` arg from parts.
fn kv(key: &str, value: &str) -> String {
    format!("{key}={value}")
}

/// Write a temp file with the given content. Uses a unique name per
/// call (process-id + nanos + name) so parallel tests + repeat runs
/// don't collide on shared /tmp filenames.
fn write_tmp(name: &str, content: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let unique = format!(
        "{}-{}-{}-{}",
        name,
        std::process::id(),
        nanos,
        content.len()
    );
    let path = std::env::temp_dir().join(unique);
    let mut f = fs::File::create(&path).expect("create tmp");
    f.write_all(content.as_bytes()).expect("write tmp");
    // Sanity: verify the file exists and has the content we wrote.
    let read_back = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("write_tmp read-back failed for {}: {e}", path.display()));
    assert_eq!(read_back, content, "write_tmp content mismatch");
    path
}

// ----- Brief verification #4: bridge PID change invalidates round. -----

#[test]
fn bridge_check_consistent_pids_passes() {
    // Same PID at start and end → result=consistent.
    let start = write_tmp("v3-bridge-pid-it-consistent-start.txt", "12345\n");
    let end = write_tmp("v3-bridge-pid-it-consistent-end.txt", "12345\n");
    let start_arg = kv("--start", start.to_str().unwrap());
    let end_arg = kv("--end", end.to_str().unwrap());
    let (output, stdout, _stderr) = run_loadgen(&["bridge-check", &start_arg, &end_arg]);
    assert!(output.status.success(), "exit should be 0");
    assert!(
        stdout.contains("result=consistent"),
        "expected result=consistent in stdout: {stdout}"
    );
    let _ = fs::remove_file(start);
    let _ = fs::remove_file(end);
}

#[test]
fn bridge_check_changed_pids_reports_change() {
    // Different PIDs at start vs end → result=changed (round invalidates).
    let start = write_tmp("v3-bridge-pid-it-changed-start.txt", "11111\n");
    let end = write_tmp("v3-bridge-pid-it-changed-end.txt", "99999\n");
    let start_arg = kv("--start", start.to_str().unwrap());
    let end_arg = kv("--end", end.to_str().unwrap());
    let (output, stdout, _stderr) = run_loadgen(&["bridge-check", &start_arg, &end_arg]);
    assert!(output.status.success(), "exit should be 0");
    assert!(
        stdout.contains("result=changed"),
        "expected result=changed in stdout: {stdout}"
    );
    let _ = fs::remove_file(start);
    let _ = fs::remove_file(end);
}

#[test]
fn bridge_check_missing_start_file_reports_unreadable_start() {
    // Missing start file → result=unreadable-start.
    let start = PathBuf::from("/tmp/v3-bridge-pid-it-missing-start-DOES-NOT-EXIST.txt");
    let end = write_tmp("v3-bridge-pid-it-missing-start-end.txt", "12345\n");
    let start_arg = kv("--start", start.to_str().unwrap());
    let end_arg = kv("--end", end.to_str().unwrap());
    let (output, stdout, _stderr) = run_loadgen(&["bridge-check", &start_arg, &end_arg]);
    assert!(output.status.success(), "exit should be 0");
    assert!(
        stdout.contains("result=unreadable-start"),
        "expected result=unreadable-start: {stdout}"
    );
    let _ = fs::remove_file(end);
}

// ----- Brief verification #5: RSS sampling includes child processes. -----

#[test]
fn rss_sample_includes_child_process_rss() {
    // Spawn a child `sleep` process so it has known-alive PID + non-zero
    // RSS. Then sample the *test process's* PID, verify peak_process_count
    // >= 2 (we + child).
    //
    // We poll /proc/<our_pid>/task/<our_pid>/children ourselves first to
    // confirm the child is registered before invoking the sampler (the
    // kernel can take a beat to populate /proc after fork).
    let mut child = Command::new("sleep")
        .arg("10")
        .spawn()
        .expect("spawn sleep child");
    let our_pid = std::process::id();

    // Wait for the kernel to register the child in /proc/<pid>/task/*/children.
    // (Cargo's test runner is multi-threaded — the sleep may be attributed
    // to a worker thread's task, not the main thread's.)
    let task_dir = format!("/proc/{our_pid}/task");
    let mut detected = false;
    for _ in 0..50 {
        if let Ok(entries) = fs::read_dir(&task_dir) {
            for entry in entries.flatten() {
                let children_path = format!(
                    "/proc/{our_pid}/task/{}/children",
                    entry.file_name().to_string_lossy()
                );
                if let Ok(content) = fs::read_to_string(&children_path) {
                    if !content.trim().is_empty() {
                        detected = true;
                        break;
                    }
                }
            }
        }
        if detected {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        detected,
        "sleep child never registered in /proc before timeout"
    );

    // Run rss-sample on our own PID. The child sleep process will appear
    // in our process tree.
    let pid_arg = kv("--pid", &our_pid.to_string());
    let interval_arg = kv("--interval-ms", "10");
    let duration_arg = kv("--duration-ms", "200");
    let (output, stdout, _stderr) =
        run_loadgen(&["rss-sample", &pid_arg, &interval_arg, &duration_arg]);

    // Reap the child regardless of test outcome.
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        output.status.success(),
        "rss-sample failed: {:?}",
        output.status
    );
    // Look for "peak_process_count=N" with N >= 2.
    let line = stdout
        .lines()
        .find(|l| l.starts_with("BENCH_RSS_OUTCOME"))
        .unwrap_or_else(|| panic!("missing BENCH_RSS_OUTCOME line in stdout: {stdout}"));
    eprintln!("DEBUG rss_sample stdout line: {line}");
    // Also dump the current /proc task children for diagnosis.
    if let Ok(entries) = fs::read_dir(format!("/proc/{our_pid}/task")) {
        for entry in entries.flatten() {
            let p = format!(
                "/proc/{our_pid}/task/{}/children",
                entry.file_name().to_string_lossy()
            );
            eprintln!("DEBUG {} = {:?}", p, fs::read_to_string(&p).ok());
        }
    }
    let count_str = line
        .split(' ')
        .find(|tok| tok.starts_with("peak_process_count="))
        .unwrap_or_else(|| panic!("missing peak_process_count= in line: {line}"));
    let count: usize = count_str
        .split('=')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap_or_else(|e| panic!("peak_process_count not an int: {e}"));
    assert!(
        count >= 2,
        "expected peak_process_count >= 2 (parent + sleep child), got {count}: {stdout}"
    );

    // Also verify summed VmRSS is non-zero (this test process uses some RAM).
    let rss_str = line
        .split(' ')
        .find(|tok| tok.starts_with("max_summed_vmrss_kib="))
        .unwrap_or_else(|| panic!("missing max_summed_vmrss_kib= in line: {line}"));
    let rss: u64 = rss_str
        .split('=')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap_or_else(|e| panic!("max_summed_vmrss_kib not an int: {e}"));
    assert!(rss > 0, "expected non-zero summed RSS, got {rss}");
}

// ----- Brief verification: parse-protocol-b round aggregation. -----

#[test]
fn parse_protocol_b_handles_multi_round_log() {
    // Synthetic Protocol B log: 2 rounds, 5 samples each, blank-line
    // separated. Field 2 is ns (direct). Verify the CLI parses and
    // reports both rounds.
    let log_content = "BENCH_LATENCY 0 10000000
BENCH_LATENCY 1 10000000
BENCH_LATENCY 2 10000000
BENCH_LATENCY 3 10000000
BENCH_LATENCY 4 10000000

BENCH_LATENCY 0 20000000
BENCH_LATENCY 1 20000000
BENCH_LATENCY 2 20000000
BENCH_LATENCY 3 20000000
BENCH_LATENCY 4 20000000
";
    let log_path = write_tmp("v3-protocol-b-it-multi.log", log_content);
    let log_arg = kv("--log", log_path.to_str().unwrap());
    let (output, stdout, stderr) = run_loadgen(&["parse-protocol-b", &log_arg]);
    assert!(
        output.status.success(),
        "parse-protocol-b failed (status {:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("rounds=2"),
        "expected rounds=2 in stdout: {stdout}"
    );
    // Median of [10ms, 20ms] (emitted as ns) = 15ms = 15_000_000 ns.
    assert!(
        stdout.contains("median_p99_ns=15000000"),
        "expected median_p99_ns=15000000: {stdout}"
    );
    let _ = fs::remove_file(log_path);
}

#[test]
fn parse_protocol_b_ignores_non_latency_lines() {
    // Non-BENCH_LATENCY lines (other markers/comments) are ignored as
    // irrelevant noise — NOT counted as malformed, NOT invalidating.
    let log_content = "BENCH_LATENCY 0 5000000
NOT_A_BENCH_LINE garbage
BENCH_LATENCY 1 5000000
";
    let log_path = write_tmp("v3-protocol-b-it-noise.log", log_content);
    let log_arg = kv("--log", log_path.to_str().unwrap());
    let (output, stdout, stderr) = run_loadgen(&["parse-protocol-b", &log_arg]);
    assert!(
        output.status.success(),
        "parse-protocol-b failed (status {:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("malformed=0"),
        "expected malformed=0 (non-latency lines ignored): {stdout}"
    );
    assert!(
        stdout.contains("total_samples=2"),
        "expected total_samples=2: {stdout}"
    );
    let _ = fs::remove_file(log_path);
}

// ----- Brief verification: --help / unknown subcommand error handling. -----

#[test]
fn help_flag_lists_all_subcommands() {
    let (output, stdout, stderr) = run_loadgen(&["--help"]);
    assert!(output.status.success());
    let combined = format!("{stdout}{stderr}");
    for sub in [
        "devnull",
        "calibrate",
        "measure-a",
        "parse-protocol-b",
        "rss-sample",
        "bridge-check",
    ] {
        assert!(
            combined.contains(sub),
            "expected subcommand '{sub}' in help output: {combined}"
        );
    }
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let (output, _stdout, _stderr) = run_loadgen(&["this-is-not-a-subcommand"]);
    assert_ne!(
        output.status.code(),
        Some(0),
        "unknown subcommand should exit nonzero"
    );
}

#[test]
fn missing_required_flag_exits_nonzero() {
    // calibrate requires --url.
    let (output, _stdout, stderr) = run_loadgen(&["calibrate"]);
    assert_ne!(output.status.code(), Some(0));
    assert!(
        stderr.contains("--url is required"),
        "expected --url is required error: {stderr}"
    );
}
