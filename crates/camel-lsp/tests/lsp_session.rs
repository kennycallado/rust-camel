use std::sync::Arc;
use std::time::Duration;

use camel_lint::LintEngine;
use camel_lint::{CapabilityQuery, ComponentMetadata, ComponentMetadataCatalog};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Stub catalog
// ---------------------------------------------------------------------------

struct EmptyCatalog;

impl ComponentMetadataCatalog for EmptyCatalog {
    fn get_metadata(&self, _scheme: &str) -> Option<ComponentMetadata> {
        None
    }

    fn schemes(&self) -> Vec<String> {
        Vec::new()
    }

    fn all_metadata(&self) -> Vec<ComponentMetadata> {
        Vec::new()
    }

    fn query_capabilities(&self, _query: &CapabilityQuery) -> Vec<ComponentMetadata> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type ClientWrite = WriteHalf<tokio::io::DuplexStream>;
type ClientRead = ReadHalf<tokio::io::DuplexStream>;

/// Spawn an LSP server with the given lint engine, returning the client-side
/// IO halves and the server task handle.
async fn spawn_server(engine: LintEngine) -> (ClientWrite, ClientRead, JoinHandle<()>) {
    let (service, socket) =
        tower_lsp::LspService::new(move |client| camel_lsp::Backend::new(client, engine));

    let (client_side, server_side) = tokio::io::duplex(8192);
    let (server_read, server_write) = tokio::io::split(server_side);
    let (client_read, client_write) = tokio::io::split(client_side);

    let handle = tokio::spawn(async move {
        tower_lsp::Server::new(server_read, server_write, socket)
            .serve(service)
            .await;
    });

    (client_write, client_read, handle)
}

/// Write a JSON-RPC 2.0 request frame (Content-Length header + body).
async fn send_jsonrpc(
    writer: &mut (impl AsyncWrite + Unpin),
    method: &str,
    params: Value,
    id: i64,
) -> std::io::Result<()> {
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": id,
    });
    let body = serde_json::to_vec(&json)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    Ok(())
}

/// Write a JSON-RPC 2.0 notification frame (no id field).
async fn send_notification(
    writer: &mut (impl AsyncWrite + Unpin),
    method: &str,
    params: Value,
) -> std::io::Result<()> {
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    let body = serde_json::to_vec(&json)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    Ok(())
}

/// Read a single JSON-RPC 2.0 frame, returning the parsed JSON body,
/// or `None` on EOF or parse error.
async fn read_jsonrpc(reader: &mut (impl AsyncRead + Unpin)) -> Option<Value> {
    // Read until "\r\n\r\n"
    let mut raw = Vec::new();
    loop {
        let mut buf = [0u8; 1];
        match reader.read_exact(&mut buf).await {
            Ok(_) => {}
            Err(_) => return None, // EOF or I/O error
        }
        raw.push(buf[0]);
        if raw.len() >= 4 && raw[raw.len() - 4..] == *b"\r\n\r\n" {
            break;
        }
    }
    let header = String::from_utf8_lossy(&raw[..raw.len() - 4]);
    let content_length: usize = header
        .lines()
        .find(|l| l.to_lowercase().starts_with("content-length:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok())?;

    let mut body = vec![0u8; content_length];
    match reader.read_exact(&mut body).await {
        Ok(_) => {}
        Err(_) => return None,
    }
    serde_json::from_slice(&body).ok()
}

/// Shut down the server and wait for it to terminate.
async fn shutdown_server(mut cw: ClientWrite, cr: ClientRead, handle: JoinHandle<()>) {
    send_jsonrpc(&mut cw, "shutdown", serde_json::json!({}), 99)
        .await
        .unwrap(); // allow-unwrap — test helper
    send_notification(&mut cw, "exit", serde_json::json!({}))
        .await
        .unwrap(); // allow-unwrap — test helper
    drop(cw);
    drop(cr);
    let _ = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("server did not shut down within 5s");
}

// ---------------------------------------------------------------------------
// Engine factory
// ---------------------------------------------------------------------------

fn make_engine() -> LintEngine {
    let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(EmptyCatalog);
    LintEngine::new(catalog).with_default_rules()
}

const DOC_URI: &str = "file:///t.route.yaml";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Full editor lifecycle: open → change (error) → change (fix) → save → close.
#[tokio::test]
async fn session_open_change_save_close() {
    let engine = make_engine();
    let (mut cw, mut cr, handle) = spawn_server(engine).await;

    // ── 1. Initialize ──────────────────────────────────────────────────────
    send_jsonrpc(
        &mut cw,
        "initialize",
        serde_json::json!({
            "processId": null,
            "rootUri": null,
            "capabilities": {}
        }),
        1,
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    let init_resp = read_jsonrpc(&mut cr).await.expect("initialize response");
    let caps = &init_resp["result"]["capabilities"];
    assert_eq!(caps["textDocumentSync"], 2); // TextDocumentSyncKind::INCREMENTAL
    assert!(caps["completionProvider"].is_object());
    assert_eq!(caps["hoverProvider"], true);
    assert_eq!(init_resp["result"]["serverInfo"]["name"], "camel-lsp");

    // ── 2. Initialized ─────────────────────────────────────────────────────
    send_notification(&mut cw, "initialized", serde_json::json!({}))
        .await
        .unwrap(); // allow-unwrap — test helper

    // ── 3. didOpen (valid route) ───────────────────────────────────────────
    send_notification(
        &mut cw,
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": DOC_URI,
                "languageId": "camel-route",
                "version": 1,
                "text": "from: direct:start\n"
            }
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    let diag_open = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after didOpen");
    assert_eq!(diag_open["method"], "textDocument/publishDiagnostics");
    let open_diags = diag_open["params"]["diagnostics"].as_array().unwrap(); // allow-unwrap — test helper
                                                                             // Default rules may produce known diagnostics (e.g., missing `id`,
                                                                             // unregistered scheme). Accept any diagnostics — just confirm the
                                                                             // notification arrived.
    let _ = open_diags;

    // ── 4. didChange: syntax error (full replacement) ──────────────────────
    send_notification(
        &mut cw,
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": DOC_URI, "version": 2},
            "contentChanges": [{"text": "steps:\n  - to: timer:foo\n  bad: ["}]
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    // Debounce delay is 50ms; wait past it.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let diag_err = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after syntax-error change");
    assert_eq!(diag_err["method"], "textDocument/publishDiagnostics");
    let err_diags = diag_err["params"]["diagnostics"].as_array().unwrap(); // allow-unwrap — test helper
    assert!(
        !err_diags.is_empty(),
        "expected diagnostic for syntax error; got empty array"
    );

    // ── 5. didChange: fix the error ────────────────────────────────────────
    send_notification(
        &mut cw,
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": DOC_URI, "version": 3},
            "contentChanges": [{"text": "from: direct:start\n"}]
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    tokio::time::sleep(Duration::from_millis(100)).await;

    let diag_fix = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after fix change");
    assert_eq!(diag_fix["method"], "textDocument/publishDiagnostics");
    // Accept known diagnostics — the important thing is no new errors were
    // introduced by the edit cycle.
    let _ = diag_fix["params"]["diagnostics"];

    // ── 6. didSave ─────────────────────────────────────────────────────────
    send_notification(
        &mut cw,
        "textDocument/didSave",
        serde_json::json!({
            "textDocument": {"uri": DOC_URI}
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    let diag_save = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after didSave");
    assert_eq!(diag_save["method"], "textDocument/publishDiagnostics");
    assert!(
        diag_save["params"]["diagnostics"].is_array(),
        "diagnostics should be an array"
    );

    // ── 7. didClose ────────────────────────────────────────────────────────
    send_notification(
        &mut cw,
        "textDocument/didClose",
        serde_json::json!({
            "textDocument": {"uri": DOC_URI}
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    let diag_close = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after didClose");
    assert_eq!(diag_close["method"], "textDocument/publishDiagnostics");
    let close_diags = diag_close["params"]["diagnostics"].as_array().unwrap(); // allow-unwrap — test helper
    assert!(
        close_diags.is_empty(),
        "expected empty diagnostics after close; got: {close_diags:?}"
    );

    // ── Cleanup ────────────────────────────────────────────────────────────
    shutdown_server(cw, cr, handle).await;
}

/// Open an empty document — must not panic.
#[tokio::test]
async fn session_partial_input_empty() {
    let engine = make_engine();
    let (mut cw, mut cr, handle) = spawn_server(engine).await;

    // Initialize
    send_jsonrpc(
        &mut cw,
        "initialize",
        serde_json::json!({"processId": null, "rootUri": null, "capabilities": {}}),
        1,
    )
    .await
    .unwrap(); // allow-unwrap — test helper
    let _init = read_jsonrpc(&mut cr).await.expect("init response");
    send_notification(&mut cw, "initialized", serde_json::json!({}))
        .await
        .unwrap(); // allow-unwrap — test helper

    // didOpen with empty string
    send_notification(
        &mut cw,
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": "file:///empty.route.yaml",
                "languageId": "camel-route",
                "version": 1,
                "text": ""
            }
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    let notif = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after empty open");
    assert_eq!(notif["method"], "textDocument/publishDiagnostics");
    assert!(
        notif["params"]["diagnostics"].is_array(),
        "diagnostics should be an array"
    );

    shutdown_server(cw, cr, handle).await;
}

/// Open a truncated YAML fragment — must not panic.
#[tokio::test]
async fn session_partial_input_truncated_yaml() {
    let engine = make_engine();
    let (mut cw, mut cr, handle) = spawn_server(engine).await;

    // Initialize
    send_jsonrpc(
        &mut cw,
        "initialize",
        serde_json::json!({"processId": null, "rootUri": null, "capabilities": {}}),
        1,
    )
    .await
    .unwrap(); // allow-unwrap — test helper
    let _init = read_jsonrpc(&mut cr).await.expect("init response");
    send_notification(&mut cw, "initialized", serde_json::json!({}))
        .await
        .unwrap(); // allow-unwrap — test helper

    // didOpen with truncated YAML value
    send_notification(
        &mut cw,
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": "file:///truncated.route.yaml",
                "languageId": "camel-route",
                "version": 1,
                "text": "from: timer:tick?period="
            }
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    let notif = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after truncated open");
    assert_eq!(notif["method"], "textDocument/publishDiagnostics");
    assert!(
        notif["params"]["diagnostics"].is_array(),
        "diagnostics should be an array"
    );

    shutdown_server(cw, cr, handle).await;
}

/// Non-ASCII Unicode route text must not panic. Also exercises completion
/// inside an option key.
#[tokio::test]
async fn session_non_ascii_unicode() {
    let engine = make_engine();
    let (mut cw, mut cr, handle) = spawn_server(engine).await;

    // Initialize
    send_jsonrpc(
        &mut cw,
        "initialize",
        serde_json::json!({"processId": null, "rootUri": null, "capabilities": {}}),
        1,
    )
    .await
    .unwrap(); // allow-unwrap — test helper
    let _init = read_jsonrpc(&mut cr).await.expect("init response");
    send_notification(&mut cw, "initialized", serde_json::json!({}))
        .await
        .unwrap(); // allow-unwrap — test helper

    // didOpen with café, Japanese, and emoji
    send_notification(
        &mut cw,
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": "file:///unicode.route.yaml",
                "languageId": "camel-route",
                "version": 1,
                "text": "from: timer:café?note=こんにちは🌟"
            }
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    let _diags = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after unicode open");

    // Completion at a position inside the option key "note".
    // Byte layout: "from: timer:café?note=こんにちは🌟"
    //   f=0 r=1 o=2 m=3 :=4 space=5 t=6 i=7 m=8 e=9 r=10 :=11
    //   c=12 a=13 f=14 é=15-16 ?=17 n=18 o=19 t=20 e=21 ==22
    // Position at character 19 (byte 19 → 'o' in "note").
    send_jsonrpc(
        &mut cw,
        "textDocument/completion",
        serde_json::json!({
            "textDocument": {"uri": "file:///unicode.route.yaml"},
            "position": {"line": 0, "character": 19}
        }),
        2,
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    let completion_resp = read_jsonrpc(&mut cr).await.expect("completion response");
    assert_eq!(completion_resp["id"], 2);
    // Response may be null or empty — just assert no panic.
    let _ = completion_resp["result"];

    shutdown_server(cw, cr, handle).await;
}

/// Completion on a partially-typed URI must not panic.
#[tokio::test]
async fn session_completion_partial_uri() {
    let engine = make_engine();
    let (mut cw, mut cr, handle) = spawn_server(engine).await;

    // Initialize
    send_jsonrpc(
        &mut cw,
        "initialize",
        serde_json::json!({"processId": null, "rootUri": null, "capabilities": {}}),
        1,
    )
    .await
    .unwrap(); // allow-unwrap — test helper
    let _init = read_jsonrpc(&mut cr).await.expect("init response");
    send_notification(&mut cw, "initialized", serde_json::json!({}))
        .await
        .unwrap(); // allow-unwrap — test helper

    // didOpen with incomplete route "from: " (trailing space, no scheme)
    send_notification(
        &mut cw,
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": "file:///partial.route.yaml",
                "languageId": "camel-route",
                "version": 1,
                "text": "from: "
            }
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    let _diags = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after partial open");

    // Completion after the space (character 6, past end of "from: " — 6 bytes).
    // The position module should clamp to end of line.
    send_jsonrpc(
        &mut cw,
        "textDocument/completion",
        serde_json::json!({
            "textDocument": {"uri": "file:///partial.route.yaml"},
            "position": {"line": 0, "character": 6}
        }),
        2,
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    let completion_resp = read_jsonrpc(&mut cr).await.expect("completion response");
    assert_eq!(completion_resp["id"], 2);
    // Response may be null or have completions — just assert no panic.
    let _ = completion_resp["result"];

    shutdown_server(cw, cr, handle).await;
}

// ---------------------------------------------------------------------------
// Debounce and version-ordering tests
// ---------------------------------------------------------------------------

/// Send 5 rapid didChange events (versions 2–6) without yielding between
/// them, then wait past the 50 ms debounce window. Assert that only ONE
/// diagnostics notification was published and that it targets version 6.
#[tokio::test]
async fn debounce_publishes_only_final_version() {
    let engine = make_engine();
    let (mut cw, mut cr, handle) = spawn_server(engine).await;

    // Initialize + didOpen
    send_jsonrpc(
        &mut cw,
        "initialize",
        serde_json::json!({"processId": null, "rootUri": null, "capabilities": {}}),
        1,
    )
    .await
    .unwrap(); // allow-unwrap — test helper
    let _init = read_jsonrpc(&mut cr).await.expect("init response");
    send_notification(&mut cw, "initialized", serde_json::json!({}))
        .await
        .unwrap(); // allow-unwrap — test helper

    send_notification(
        &mut cw,
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": DOC_URI,
                "languageId": "camel-route",
                "version": 1,
                "text": "a"
            }
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper
    let _diags_open = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after didOpen");

    // Send 5 rapid didChange (versions 2–6) in the same async block — no
    // yield points between them, so they all hit before the debounce fires.
    for version in 2..=6 {
        send_notification(
            &mut cw,
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": DOC_URI, "version": version},
                "contentChanges": [{"text": format!("{}", (b'a' + (version - 2) as u8) as char)}]
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
    }

    // Wait past the 50 ms debounce.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Collect all diagnostics published within a follow-up window.
    let mut notifications: Vec<Value> = Vec::new();
    while let Ok(Some(v)) =
        tokio::time::timeout(Duration::from_millis(100), read_jsonrpc(&mut cr)).await
    {
        if v["method"] == "textDocument/publishDiagnostics" {
            notifications.push(v);
        }
    }

    // Exactly one diagnostics notification should have been published.
    assert_eq!(
        notifications.len(),
        1,
        "expected exactly 1 publishDiagnostics notification, got {}",
        notifications.len(),
    );
    assert_eq!(
        notifications[0]["params"]["version"], 6,
        "expected diagnostics for version 6 (the latest)"
    );

    shutdown_server(cw, cr, handle).await;
}

/// Version 2 introduces a syntax error, but version 3 (sent before
/// debounce fires) fixes it. The final published diagnostics must reflect
/// version 3 (the fixed document), not version 2 (the broken one).
#[tokio::test]
async fn debounce_stale_result_discarded() {
    let engine = make_engine();
    let (mut cw, mut cr, handle) = spawn_server(engine).await;

    // Initialize + didOpen with valid text.
    send_jsonrpc(
        &mut cw,
        "initialize",
        serde_json::json!({"processId": null, "rootUri": null, "capabilities": {}}),
        1,
    )
    .await
    .unwrap(); // allow-unwrap — test helper
    let _init = read_jsonrpc(&mut cr).await.expect("init response");
    send_notification(&mut cw, "initialized", serde_json::json!({}))
        .await
        .unwrap(); // allow-unwrap — test helper

    send_notification(
        &mut cw,
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": DOC_URI,
                "languageId": "camel-route",
                "version": 1,
                "text": "from: direct:start\n"
            }
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper
    let _diags_open = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after didOpen");

    // Version 2: introduce a syntax error (unclosed `[`).
    send_notification(
        &mut cw,
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": DOC_URI, "version": 2},
            "contentChanges": [{"text": "steps:\n  - to: timer:foo\n  bad: ["}]
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    // Version 3: immediately (before debounce fires) fix the error.
    send_notification(
        &mut cw,
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": DOC_URI, "version": 3},
            "contentChanges": [{"text": "from: direct:start\n"}]
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    // Wait past the 50 ms debounce.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Collect all diagnostics.
    let mut notifications: Vec<Value> = Vec::new();
    while let Ok(Some(v)) =
        tokio::time::timeout(Duration::from_millis(100), read_jsonrpc(&mut cr)).await
    {
        if v["method"] == "textDocument/publishDiagnostics" {
            notifications.push(v);
        }
    }

    // Exactly one diagnostics notification — for version 3 (the fix).
    assert_eq!(
        notifications.len(),
        1,
        "expected exactly 1 publishDiagnostics notification, got {}",
        notifications.len(),
    );
    assert_eq!(
        notifications[0]["params"]["version"], 3,
        "expected diagnostics for version 3 (the fix)"
    );
    // The diagnostics must be benign — no syntax error from the stale v2 draft.
    let diags = notifications[0]["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics should be an array");
    let has_syntax_error = diags
        .iter()
        .any(|d| d["code"].as_str().is_some_and(|c| c.contains("parse")));
    assert!(
        !has_syntax_error,
        "stale version-2 syntax error leaked into version-3 diagnostics"
    );

    shutdown_server(cw, cr, handle).await;
}

/// Open a doc, send didChange (schedules debounced lint), immediately
/// send didClose (cancels pending lint + publishes empty diagnostics).
/// After the didClose notification, no further diagnostics must arrive.
#[tokio::test]
async fn did_close_cancels_pending_lint() {
    let engine = make_engine();
    let (mut cw, mut cr, handle) = spawn_server(engine).await;

    // Initialize + didOpen.
    send_jsonrpc(
        &mut cw,
        "initialize",
        serde_json::json!({"processId": null, "rootUri": null, "capabilities": {}}),
        1,
    )
    .await
    .unwrap(); // allow-unwrap — test helper
    let _init = read_jsonrpc(&mut cr).await.expect("init response");
    send_notification(&mut cw, "initialized", serde_json::json!({}))
        .await
        .unwrap(); // allow-unwrap — test helper

    send_notification(
        &mut cw,
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": DOC_URI,
                "languageId": "camel-route",
                "version": 1,
                "text": "from: direct:start\n"
            }
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper
    let _diags_open = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after didOpen");

    // Send didChange to schedule a debounced lint (50 ms debounce).
    send_notification(
        &mut cw,
        "textDocument/didChange",
        serde_json::json!({
            "textDocument": {"uri": DOC_URI, "version": 2},
            "contentChanges": [{"text": "from: direct:end\n"}]
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    // Immediately send didClose — cancels pending lint + publishes empty.
    send_notification(
        &mut cw,
        "textDocument/didClose",
        serde_json::json!({
            "textDocument": {"uri": DOC_URI}
        }),
    )
    .await
    .unwrap(); // allow-unwrap — test helper

    // Read the empty diagnostics from didClose.
    let notif = read_jsonrpc(&mut cr)
        .await
        .expect("publishDiagnostics after didClose");
    assert_eq!(notif["method"], "textDocument/publishDiagnostics");
    let close_diags = notif["params"]["diagnostics"].as_array().unwrap(); // allow-unwrap — test helper
    assert!(
        close_diags.is_empty(),
        "expected empty diagnostics on close; got: {close_diags:?}"
    );

    // Wait past debounce to confirm no further diagnostics arrive.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let result = tokio::time::timeout(Duration::from_millis(200), read_jsonrpc(&mut cr)).await;
    assert!(
        result.is_err(),
        "expected no further diagnostics after didClose — pending lint was cancelled"
    );

    shutdown_server(cw, cr, handle).await;
}
