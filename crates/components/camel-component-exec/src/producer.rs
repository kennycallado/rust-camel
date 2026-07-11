//! producer.rs — ExecProducer: Service<Exchange> running the enforcement flow.
//!
//! Outcomes: pre/during-spawn failures => Err. Post-spawn conditions with output
//! (timeout, exit-code mismatch) => Ok with result + headers (route branches via choice).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange};
use camel_component_api::RuntimeObservability;
use tokio::sync::Semaphore;
use tower::Service;

use crate::audit::{ExecAuditEvent, emit};
use crate::config::{ExecGlobalConfig, ExecProfile};
use crate::error::ExecError;
use crate::headers::*;
use crate::policy;
use crate::process::{self, RawResult};

pub struct ExecProducer {
    pub(crate) profile: Arc<ExecProfile>,
    pub(crate) global: Arc<ExecGlobalConfig>,
    pub(crate) route_id: String,
    pub(crate) host_env: Arc<HashMap<String, String>>,
    pub(crate) semaphore: Arc<Semaphore>,
    pub(crate) rt: Option<Arc<dyn RuntimeObservability>>,
}

impl Clone for ExecProducer {
    fn clone(&self) -> Self {
        Self {
            profile: Arc::clone(&self.profile),
            global: Arc::clone(&self.global),
            route_id: self.route_id.clone(),
            host_env: Arc::clone(&self.host_env),
            semaphore: Arc::clone(&self.semaphore),
            rt: self.rt.clone(),
        }
    }
}

impl Service<Exchange> for ExecProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let this = self.clone();
        Box::pin(async move { this.run(&mut exchange).await.map(|_| exchange) })
    }
}

impl ExecProducer {
    /// Create a new ExecProducer. Public for integration tests.
    pub fn new(
        profile: Arc<ExecProfile>,
        global: Arc<ExecGlobalConfig>,
        route_id: String,
        host_env: HashMap<String, String>,
        semaphore: Arc<Semaphore>,
        rt: Option<Arc<dyn RuntimeObservability>>,
    ) -> Self {
        Self {
            profile,
            global,
            route_id,
            host_env: Arc::new(host_env),
            semaphore,
            rt,
        }
    }

    async fn run(&self, ex: &mut Exchange) -> Result<(), CamelError> {
        let start = Instant::now();
        // 1. args
        let args = self.resolve_args(ex)?;
        // 2. arg-policy
        if let Err(e) = policy::eval_args(&args, &self.profile.args, &self.profile.deny_flags) {
            self.metric_denied("arg_policy");
            return Err(pack(&self.route_id, e));
        }
        // 3. shell reject
        let exe = self
            .profile
            .canonical_executable
            .clone()
            .unwrap_or_default();
        let exe_str = exe.to_string_lossy().to_string();
        if !self.profile.allow_shell && policy::is_known_shell(&exe_str) {
            self.metric_denied("shell");
            return Err(pack(
                &self.route_id,
                ExecError::ShellRejected {
                    executable: exe_str.clone(),
                },
            ));
        }
        // 4. env build (deny_env applied last, always wins)
        let env = policy::build_env(&self.profile.env, &self.global.deny_env, &self.host_env);
        // 5. cwd — use the STARTUP-PINNED canonical root (I-6: no "." fallback)
        let cwd = match &self.global.canonical_workspace_root {
            Some(root) => crate::config::confine(root, &self.profile.working_dir).map_err(|e| {
                self.metric_denied("workdir");
                pack(&self.route_id, ExecError::InvalidWorkDir { path: e })
            })?,
            None => {
                return Err(pack(
                    &self.route_id,
                    ExecError::InvalidWorkDir {
                        path: "workspace root not pinned at startup".into(),
                    },
                ));
            }
        };
        // 6. stdin (capped)
        let stdin =
            extract_stdin(ex, self.profile.stdin_max_bytes).map_err(|e| pack(&self.route_id, e))?;
        // 7. concurrency permit — acquired BEFORE spawn (held through spawn→collect)
        let _permit = self.semaphore.acquire().await.map_err(|_| {
            CamelError::ProcessorErrorWithSource(
                "exec semaphore closed".into(),
                Arc::new(ExecError::Spawn(std::io::Error::other("semaphore"))),
            )
        })?;
        // 8. spawn + timeout + drain (child held outside the timeout region — C-1)
        let raw = self
            .spawn_and_collect(&exe, &args, &env, &cwd, stdin)
            .await
            .map_err(|e| pack(&self.route_id, e))?;
        // 9. populate exchange (non-error outcome even on timeout / non-zero exit)
        let exit_code = raw.exit_code;
        let accepted = exit_code
            .map(|c| self.profile.accepted_exit_codes.contains(&c))
            .unwrap_or(false);
        let body_out = ExecResult {
            exit_code,
            stdout: b64(&raw.stdout),
            stderr: b64(&raw.stderr),
            stdout_truncated: raw.stdout_truncated,
            stderr_truncated: raw.stderr_truncated,
            timed_out: raw.timed_out,
            profile: self.profile.name.clone(),
            duration_ms: start.elapsed().as_millis() as u64,
        };
        ex.input.body = Body::Json(serde_json::to_value(&body_out).map_err(|e| {
            pack(
                &self.route_id,
                ExecError::InvalidArgs(format!("serialize result: {e}")),
            )
        })?);
        let h = &mut ex.input.headers;
        h.insert(
            CAMEL_EXEC_PROFILE.to_string(),
            serde_json::Value::String(self.profile.name.clone()),
        );
        // Always emit CamelExecExitAccepted (false when no exit code, e.g. timeout).
        if let Some(c) = exit_code {
            h.insert(CAMEL_EXEC_EXIT_CODE.to_string(), serde_json::Value::from(c));
            h.insert(
                CAMEL_EXEC_EXIT_ACCEPTED.to_string(),
                serde_json::Value::from(accepted),
            );
        } else {
            h.insert(
                CAMEL_EXEC_EXIT_ACCEPTED.to_string(),
                serde_json::Value::from(false),
            );
        }
        h.insert(
            CAMEL_EXEC_TIMED_OUT.to_string(),
            serde_json::Value::from(raw.timed_out),
        );
        h.insert(
            CAMEL_EXEC_STDOUT_TRUNCATED.to_string(),
            serde_json::Value::from(raw.stdout_truncated),
        );
        h.insert(
            CAMEL_EXEC_STDERR_TRUNCATED.to_string(),
            serde_json::Value::from(raw.stderr_truncated),
        );
        // Header CAMEL_EXEC_STDERR carries lossy-UTF8 for route predicates
        // (log:, choice()); body ExecResult.stderr carries byte-exact base64.
        // Dual repr is intentional.
        h.insert(
            CAMEL_EXEC_STDERR.to_string(),
            serde_json::Value::String(String::from_utf8_lossy(&raw.stderr).into_owned()),
        );
        // 10. metrics + audit
        self.metric_outcome(
            exit_code,
            raw.timed_out,
            raw.stdout_truncated,
            start.elapsed(),
        );
        emit(&ExecAuditEvent {
            route_id: &self.route_id,
            profile: &self.profile.name,
            canonical_executable: &exe_str,
            args: &args,
            env_keys: env.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
            cwd: &cwd.to_string_lossy(),
            exit_code,
            timed_out: raw.timed_out,
            stdout_truncated: raw.stdout_truncated,
            stderr_truncated: raw.stderr_truncated,
            duration: start.elapsed(),
        });
        Ok(())
    }

    async fn spawn_and_collect(
        &self,
        exe: &std::path::Path,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: &std::path::Path,
        stdin: Bytes,
    ) -> Result<RawResult, ExecError> {
        // C-1: spawn OUTSIDE the timeout so the Child handle survives a timeout
        // and kill_tree can fire. kill_on_drop(true) is a belt-and-suspenders guard.
        let mut child = process::spawn(exe, args, env, cwd)?;
        let stdin_handle = child.stdin.take();
        let out = child.stdout.take();
        let err = child.stderr.take();
        // drain + stdin tasks run concurrently; they complete when pipes close.
        let out_task = tokio::spawn(drain_join(out, self.profile.stdout_max_bytes));
        let err_task = tokio::spawn(drain_join(err, self.profile.stderr_max_bytes));
        let stdin_task = tokio::spawn(async move {
            if let Some(mut sin) = stdin_handle {
                use tokio::io::AsyncWriteExt;
                let _ = sin.write_all(&stdin).await;
                let _ = sin.shutdown().await;
            }
        });

        let timeout = Duration::from_secs(self.profile.timeout_secs);
        // select! keeps `child` accessible after either branch (the losing branch's
        // future is dropped, releasing the &mut borrow). Timeout bounds stdin-write
        // + wait together (a child that never reads stdin no longer hangs unbounded).
        let (exit_code, timed_out) = tokio::select! {
            biased;
            status = child.wait() => match status {
                Ok(s) => (s.code(), false),
                Err(e) => return Err(ExecError::Spawn(e)),
            },
            _ = tokio::time::sleep(timeout) => {
                process::kill_tree(&child);
                let _ = child.wait().await; // reap the killed tree
                (None, true)
            }
        };
        // After success or timeout, collect whatever the drain tasks produced.
        // (On timeout the pipes close after kill → tasks finish quickly.)
        let _ = tokio::time::timeout(Duration::from_secs(2), stdin_task).await; // best-effort
        let (stdout, stdout_tr) = out_task.await.unwrap_or((Bytes::new(), false));
        let (stderr, stderr_tr) = err_task.await.unwrap_or((Bytes::new(), false));
        Ok(RawResult {
            stdout,
            stderr,
            stdout_truncated: stdout_tr,
            stderr_truncated: stderr_tr,
            exit_code,
            timed_out,
        })
    }

    fn resolve_args(&self, ex: &Exchange) -> Result<Vec<String>, CamelError> {
        if let Some(v) = ex.input.headers.get(CAMEL_EXEC_ARGS) {
            serde_json::from_value::<Vec<String>>(v.clone())
                .map_err(|e| pack(&self.route_id, ExecError::InvalidArgs(e.to_string())))
        } else {
            Ok(Vec::new())
        }
    }

    fn metric_denied(&self, reason: &'static str) {
        if let Some(rt) = &self.rt {
            rt.metrics().record_counter(
                "exec_policy_denials_total",
                1.0,
                &[("reason", reason), ("route", &self.route_id)],
            );
        }
    }

    fn metric_outcome(
        &self,
        exit_code: Option<i32>,
        timed_out: bool,
        stdout_truncated: bool,
        dur: Duration,
    ) {
        let Some(rt) = &self.rt else { return };
        rt.metrics().record_histogram(
            "exec_duration_secs",
            dur.as_secs_f64(),
            &[("route", &self.route_id)],
        );
        if let Some(c) = exit_code {
            let code_str = c.to_string();
            rt.metrics().record_counter(
                "exec_exit_code",
                1.0,
                &[("code", &code_str), ("route", &self.route_id)],
            );
        }
        if timed_out {
            rt.metrics()
                .record_counter("exec_timeouts_total", 1.0, &[("route", &self.route_id)]);
        }
        if stdout_truncated {
            rt.metrics().record_counter(
                "exec_stdout_truncated_total",
                1.0,
                &[("route", &self.route_id)],
            );
        }
    }
}

/// Drain an Option<ChildStdout/Stderr> to (Bytes, truncated). Spawned as a task.
async fn drain_join<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    r: Option<R>,
    cap: usize,
) -> (Bytes, bool) {
    match r {
        Some(reader) => process::drain_with_cap(reader, cap).await,
        None => (Bytes::new(), false),
    }
}

/// Materialized result placed in the body. stdout/stderr are base64-encoded
/// Strings (C-2: Bytes is not Serialize without a serde feature, and raw byte
/// arrays would be a pathological JSON). `#[non_exhaustive]`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct ExecResult {
    pub exit_code: Option<i32>,
    pub stdout: String, // base64
    pub stderr: String, // base64
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
    pub profile: String,
    pub duration_ms: u64,
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn extract_stdin(ex: &Exchange, cap: usize) -> Result<Bytes, ExecError> {
    let b = match &ex.input.body {
        Body::Text(s) => Bytes::copy_from_slice(s.as_bytes()),
        Body::Bytes(b) => Bytes::copy_from_slice(b),
        Body::Json(v) => Bytes::from(serde_json::to_vec(v).unwrap_or_default()),
        _ => Bytes::new(),
    };
    if b.len() > cap {
        return Err(ExecError::StdinTooLarge {
            size: b.len(),
            max: cap,
        });
    }
    Ok(b)
}

fn pack(route_id: &str, e: ExecError) -> CamelError {
    CamelError::ProcessorErrorWithSource(format!("exec producer ({route_id}): {e}"), Arc::new(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ArgPolicy, ExecGlobalConfig, ExecProfile};
    use camel_api::{Body, Exchange};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    /// Minimal PATH lookup (mirrors config::which, but does NOT canonicalize).
    /// Tests need the symlink name (e.g., "echo") preserved as argv[0] for multicall binaries.
    fn which(name: &str) -> PathBuf {
        let path = std::env::var_os("PATH").expect("PATH must be set");
        for dir in std::env::split_paths(&path) {
            let cand = dir.join(name);
            if cand.is_file() {
                return cand; // Do NOT canonicalize — preserve symlink name for argv[0]
            }
        }
        panic!("{name} not found on PATH");
    }

    fn make_global() -> Arc<ExecGlobalConfig> {
        let root = std::env::temp_dir().canonicalize().unwrap();
        Arc::new(ExecGlobalConfig {
            canonical_workspace_root: Some(root),
            ..ExecGlobalConfig::default()
        })
    }

    fn make_profile(
        exe: &str,
        args_policy: ArgPolicy,
        accepted: Vec<i32>,
        timeout: u64,
    ) -> ExecProfile {
        // Provide minimal env: set PATH explicitly so binaries can be found.
        let mut env = crate::config::EnvPolicy::default();
        if let Ok(path) = std::env::var("PATH") {
            env.set.insert("PATH".into(), path);
        }

        ExecProfile {
            name: "test".into(),
            executable: exe.into(),
            args: args_policy,
            deny_flags: Default::default(),
            allow_shell: true, // allow sh for tests
            env,
            working_dir: ".".into(),
            timeout_secs: timeout,
            accepted_exit_codes: accepted,
            concurrency: 1,
            stdin_max_bytes: 1 << 20,
            stdout_max_bytes: 10 << 20,
            stderr_max_bytes: 1 << 20,
            sandbox: Default::default(),
            output_mode: Default::default(),
            canonical_executable: Some(which(exe)),
        }
    }

    fn make_producer(profile: ExecProfile) -> ExecProducer {
        ExecProducer {
            profile: Arc::new(profile),
            global: make_global(),
            route_id: "test".into(),
            host_env: Arc::new(HashMap::new()),
            semaphore: Arc::new(Semaphore::new(1)),
            rt: None,
        }
    }

    /// Helper: set CamelExecArgs header on the exchange.
    fn set_args(ex: &mut Exchange, args: Vec<&str>) {
        let v: Vec<String> = args.into_iter().map(String::from).collect();
        ex.input.headers.insert(
            crate::headers::CAMEL_EXEC_ARGS.to_string(),
            serde_json::to_value(v).unwrap(),
        );
    }

    /// Helper: deserialize the ExecResult JSON from the exchange body.
    fn result_from_body(ex: &Exchange) -> ExecResult {
        let v = match &ex.input.body {
            Body::Json(v) => v.clone(),
            other => panic!("expected Body::Json, got {other:?}"),
        };
        serde_json::from_value(v).expect("deserialize ExecResult")
    }

    #[tokio::test]
    async fn echo_happy_path() {
        let profile = make_profile("echo", ArgPolicy::Any, vec![0], 30);
        let producer = make_producer(profile);
        let mut ex = Exchange::default();
        set_args(&mut ex, vec!["hello"]);

        producer.run(&mut ex).await.expect("echo must succeed");

        let result = result_from_body(&ex);
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
        // stdout is base64-encoded "hello\n"
        let stdout_bytes = base64::engine::general_purpose::STANDARD
            .decode(&result.stdout)
            .unwrap();
        assert_eq!(stdout_bytes, b"hello\n");
        // Header assertions
        assert_eq!(
            ex.input
                .headers
                .get(crate::headers::CAMEL_EXEC_EXIT_ACCEPTED),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            ex.input.headers.get(crate::headers::CAMEL_EXEC_TIMED_OUT),
            Some(&serde_json::Value::Bool(false))
        );
    }

    #[tokio::test]
    async fn timeout_is_non_error_outcome() {
        let profile = make_profile("sleep", ArgPolicy::Any, vec![0], 1);
        let producer = make_producer(profile);
        let mut ex = Exchange::default();
        set_args(&mut ex, vec!["30"]);

        // KEY ASSERTION: timeout returns Ok(()), NOT Err.
        let res = producer.run(&mut ex).await;
        assert!(
            res.is_ok(),
            "timeout must be non-error outcome, got {res:?}"
        );

        let result = result_from_body(&ex);
        assert_eq!(result.exit_code, None, "killed process has no exit code");
        assert!(result.timed_out, "must be marked as timed out");
        // CamelExecExitAccepted must be false when no exit code
        assert_eq!(
            ex.input
                .headers
                .get(crate::headers::CAMEL_EXEC_EXIT_ACCEPTED),
            Some(&serde_json::Value::Bool(false))
        );
        assert_eq!(
            ex.input.headers.get(crate::headers::CAMEL_EXEC_TIMED_OUT),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[tokio::test]
    async fn non_zero_exit_is_non_error_outcome() {
        // Case 1: accepted_exit_codes=[0], actual exit=2 → accepted=false
        let profile = make_profile("sh", ArgPolicy::Any, vec![0], 30);
        let producer = make_producer(profile);
        let mut ex = Exchange::default();
        set_args(&mut ex, vec!["-c", "exit 2"]);

        let res = producer.run(&mut ex).await;
        assert!(res.is_ok(), "non-zero exit must be non-error outcome");

        let result = result_from_body(&ex);
        assert_eq!(result.exit_code, Some(2));
        assert!(!result.timed_out);
        assert_eq!(
            ex.input
                .headers
                .get(crate::headers::CAMEL_EXEC_EXIT_ACCEPTED),
            Some(&serde_json::Value::Bool(false)),
            "exit 2 not in accepted_exit_codes=[0]"
        );

        // Case 2: accepted_exit_codes=[2], actual exit=2 → accepted=true
        let profile2 = make_profile("sh", ArgPolicy::Any, vec![2], 30);
        let producer2 = make_producer(profile2);
        let mut ex2 = Exchange::default();
        set_args(&mut ex2, vec!["-c", "exit 2"]);

        producer2.run(&mut ex2).await.expect("must succeed");
        assert_eq!(
            ex2.input
                .headers
                .get(crate::headers::CAMEL_EXEC_EXIT_ACCEPTED),
            Some(&serde_json::Value::Bool(true)),
            "exit 2 in accepted_exit_codes=[2]"
        );
    }

    #[test]
    fn clone_shares_host_env() {
        let producer = make_producer(make_profile("echo", ArgPolicy::Any, vec![0], 30));
        let cloned = producer.clone();
        assert!(
            Arc::ptr_eq(&producer.host_env, &cloned.host_env),
            "clone must share host_env Arc, not deep-copy"
        );
    }

    #[tokio::test]
    async fn drain_with_cap_stores_prefix_on_overflow() {
        // &[u8] delivers all 100 bytes in a single read (tmp buf is 8192),
        // so this exercises the n > space branch (store space, flip truncated).
        let data: &[u8] = &[b'x'; 100];
        let (buf, truncated) = process::drain_with_cap(data, 50).await;
        assert_eq!(buf.len(), 50, "buf must contain exactly cap bytes");
        assert!(truncated);
    }

    #[tokio::test]
    async fn drain_with_cap_multi_read_boundary() {
        // Chain two slices to force two reads: 30 bytes then 30 bytes.
        // First read fits (buf=30), second read overflows (space=20 → store 20, flip).
        use tokio::io::AsyncReadExt;
        let r1: &[u8] = &[b'x'; 30];
        let r2: &[u8] = &[b'y'; 30];
        let chained = r1.chain(r2);
        let (buf, truncated) = process::drain_with_cap(chained, 50).await;
        assert_eq!(buf.len(), 50);
        assert!(truncated);
    }

    #[tokio::test]
    async fn drain_with_cap_exact_fit_not_truncated() {
        let data: &[u8] = &[b'x'; 50];
        let (buf, truncated) = process::drain_with_cap(data, 50).await;
        assert_eq!(buf.len(), 50);
        assert!(!truncated);
    }
}
