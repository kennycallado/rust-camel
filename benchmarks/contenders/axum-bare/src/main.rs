//! rc-u034 reference contender: bare axum + hyper + tokio HTTP server with
//! ZERO camel dependencies.
//!
//! Purpose: isolate the HTTP-stack tax (axum/tower/hyper/tokio) from the
//! camel-tax in the benchmark suite. It is measured by the existing harness
//! at the next canonical run; its numbers are never a published record
//! until then.
//!
//! Marker contract: after the listener binds, exactly one `BENCH_ROUTE_READY`
//! line is written to stdout and explicitly flushed.
//!
//! Route contract (T3): any-method `/bench` returns 200
//! `text/plain; charset=utf-8` with body `pong`, fully draining the
//! request body so keep-alive reuse works. Minimal-bare per e_opus
//! ruling D1 (2026-09-16; bd rc-h42s6): NO per-request stdout lines —
//! the drain is inherent keep-alive stack work, not observable work,
//! and is retained.
//!
//! Port: 8080 by default, overridable via `BENCH_AXUM_BARE_PORT`.

use std::io::Write;
use std::process::ExitCode;

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::any;
use axum::Router;

const DEFAULT_PORT: u16 = 8080;
/// 1 MiB cap for the drain of a single request body.
const MAX_BODY: usize = 1024 * 1024;

#[tokio::main]
async fn main() -> ExitCode {
    // Mirror of the loadgen devnull port-pick pattern: absent or unparseable
    // env var falls back to the default; a parse failure warns on stderr.
    let port: u16 = std::env::var("BENCH_AXUM_BARE_PORT")
        .ok()
        .and_then(|s| match s.parse::<u16>() {
            Ok(p) => Some(p),
            Err(_) => {
                eprintln!(
                    "axum-bare: warning: invalid BENCH_AXUM_BARE_PORT '{s}', using {DEFAULT_PORT}"
                );
                None
            }
        })
        .unwrap_or(DEFAULT_PORT);

    let app = Router::new().route("/bench", any(handle));

    match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
        Ok(listener) => {
            // Spawn the serve future so the marker is printed as soon as the
            // listener exists, not after the first request completes.
            // `axum::serve(..)` is IntoFuture, so wrap it in an async block.
            let server = tokio::spawn(async move { axum::serve(listener, app).await });
            println!("BENCH_ROUTE_READY");
            // LineWriter already emits the line; the explicit flush is the
            // marker family convention.
            let _ = std::io::stdout().flush();
            match server.await {
                Ok(Ok(())) => ExitCode::SUCCESS,
                Ok(Err(e)) => {
                    eprintln!("axum-bare: error: {e}");
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("axum-bare: error: serve task panicked: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(e) => {
            eprintln!("axum-bare: error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn handle(body: Body) -> impl IntoResponse {
    // Drain the request body so keep-alive connection reuse works
    // (inherent keep-alive stack work — no observable side effects).
    let _drained = axum::body::to_bytes(body, MAX_BODY).await;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "pong",
    )
}
