//! `echo x | camel run` end-to-end parity test (openspec change
//! `stream-component`, Phase 2 Task 2.4).
//!
//! Pipes `alpha\nbeta\n` into the real `camel` binary running the checked-in
//! `stream:in → transform → stream:out` route document, observes stdout until
//! both echoed lines appear, then sends exactly one SIGTERM and requires a
//! graceful exit 0. No runner lifecycle change is exercised: EOF completes
//! the consumer's route, and the process lifetime stays signal-managed
//! (`crates/camel-cli/src/commands/run.rs`).
//!
//! Harness mirrors `run_empty_discovery_test.rs` (observe-then-signal), with
//! stdout captured separately from stderr so the exact-bytes assertion holds.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn drain_to_buffer<R: Read + Send + 'static>(mut reader: R, buffer: Arc<Mutex<String>>) {
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if let Ok(mut guard) = buffer.lock() {
                    guard.push_str(&String::from_utf8_lossy(&chunk[..n]));
                }
            }
            Err(_) => break,
        }
    }
}

#[test]
fn echo_pipe_end_to_end_via_camel_run() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stream-echo-route.yaml");

    let mut child = Command::new(env!("CARGO_BIN_EXE_camel"))
        .arg("run")
        .arg("--routes")
        .arg(&fixture)
        .arg("--no-watch")
        // `camel run`'s general log layer writes to stdout; silence it so the
        // stdout assertion sees only the stream:out data plane.
        .env("RUST_LOG", "off")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()
        .expect("failed to spawn `camel` binary");

    // Pipe the payload, then close stdin: EOF completes the consumer.
    {
        let mut stdin = child
            .stdin
            .take()
            .expect("child stdin was configured as piped");
        stdin
            .write_all(b"alpha\nbeta\n")
            .expect("write pipe payload to camel stdin");
    }

    let stdout = child
        .stdout
        .take()
        .expect("child stdout was configured as piped");
    let stderr = child
        .stderr
        .take()
        .expect("child stderr was configured as piped");

    let out_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let err_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let out_handle = thread::spawn({
        let buf = Arc::clone(&out_buf);
        move || drain_to_buffer(stdout, buf)
    });
    let err_handle = thread::spawn({
        let buf = Arc::clone(&err_buf);
        move || drain_to_buffer(stderr, buf)
    });

    // Observe phase: wait until both echoed lines are on stdout (60 s cap).
    let expected = "echo: alpha\necho: beta\n";
    let start = Instant::now();
    let step = Duration::from_millis(25);
    let observed = loop {
        {
            let guard = out_buf.lock().expect("stdout buffer lock poisoned");
            if guard.contains(expected) {
                break true;
            }
        }
        if start.elapsed() >= Duration::from_secs(60) {
            break false;
        }
        if let Ok(Some(_)) = child.try_wait() {
            break false;
        }
        thread::sleep(step);
    };

    if !observed {
        // Deterministic failure path: kill the child, show both streams.
        let _ = child.kill();
        let _ = child.wait();
        let _ = out_handle.join();
        let _ = err_handle.join();
        let out = out_buf.lock().expect("stdout buffer lock poisoned").clone();
        let err = err_buf.lock().expect("stderr buffer lock poisoned").clone();
        panic!("expected {expected:?} on stdout within 60 s.\nstdout:\n{out}\nstderr:\n{err}");
    }

    // Send exactly ONE SIGTERM: the CLI's select arm handles TERM → graceful
    // shutdown → exit 0. A second signal would force exit 1.
    let kill_status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("failed to spawn `kill -TERM`");
    assert!(
        kill_status.success(),
        "`kill -TERM` returned non-zero: {kill_status:?}"
    );

    // Bounded graceful-exit wait: 10 s from the SIGTERM, force-kill past it.
    let exit_start = Instant::now();
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) => {
                if exit_start.elapsed() >= Duration::from_secs(10) {
                    let _ = child.kill();
                    let _ = child.wait();
                    break -1;
                }
                thread::sleep(step);
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    };

    let _ = out_handle.join();
    let _ = err_handle.join();

    assert_eq!(
        exit_code,
        0,
        "camel run must exit 0 after one SIGTERM; stdout:\n{}\nstderr:\n{}",
        out_buf.lock().expect("stdout buffer lock poisoned"),
        err_buf.lock().expect("stderr buffer lock poisoned")
    );

    // Exact parity: stdout carries ONLY the echoed lines — no logs, no prompt.
    let stdout_final = out_buf.lock().expect("stdout buffer lock poisoned").clone();
    assert_eq!(stdout_final, expected);
}
