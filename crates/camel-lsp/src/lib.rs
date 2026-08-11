pub mod debounce;
pub mod position;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use camel_lint::diagnostic::Severity;
use camel_lint::{Document, LintEngine};
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

pub(crate) type DocumentState = HashMap<Url, (Document, Option<i32>)>;

/// Debounce window for `didChange`-triggered lint passes.
const DEBOUNCE_DELAY: Duration = Duration::from_millis(50);

/// Map camel-lint diagnostics to LSP `Diagnostic` values.
fn diagnostics_to_lsp(
    source: &str,
    diags: Vec<camel_lint::diagnostic::Diagnostic>,
) -> Vec<Diagnostic> {
    diags
        .into_iter()
        .map(|d| {
            let severity = match d.severity {
                Severity::Error => Some(DiagnosticSeverity::ERROR),
                Severity::Warning => Some(DiagnosticSeverity::WARNING),
                Severity::Info => Some(DiagnosticSeverity::INFORMATION),
            };
            let range = Range {
                start: position::byte_offset_to_lsp(source, d.span.start),
                end: position::byte_offset_to_lsp(source, d.span.end),
            };
            Diagnostic {
                range,
                severity,
                code: Some(NumberOrString::String(d.code.to_string())),
                source: Some("camel-lint".to_string()),
                message: d.message,
                ..Default::default()
            }
        })
        .collect()
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct Backend {
    client: Client,
    engine: Arc<LintEngine>,
    documents: Arc<RwLock<DocumentState>>,
    debouncer: debounce::DebouncedLinter,
}

impl Backend {
    pub fn new(client: Client, engine: LintEngine) -> Self {
        Self {
            client,
            engine: Arc::new(engine),
            documents: Arc::new(RwLock::new(HashMap::new())),
            debouncer: debounce::DebouncedLinter::new(),
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncKind::INCREMENTAL.into()),
                completion_provider: Some(CompletionOptions::default()),
                hover_provider: Some(true.into()),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "camel-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // no-op
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;

        let doc = Document::parse(&text);
        let diags = self.engine.lint(&doc.raw);
        let lsp_diags = diagnostics_to_lsp(&doc.raw, diags);

        self.documents
            .write()
            .await
            .insert(uri.clone(), (doc, Some(version)));
        self.client
            .publish_diagnostics(uri, lsp_diags, Some(version))
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;

        // Apply all changes under a single write lock so the document state
        // stays internally consistent between edits.
        let mut docs = self.documents.write().await;
        let Some((doc, stored_ver)) = docs.get_mut(&uri) else {
            return; // edit for an unopened document — ignore
        };

        for change in params.content_changes {
            if let Some(range) = change.range {
                let start = position::lsp_to_byte_offset(&doc.raw, range.start);
                let end = position::lsp_to_byte_offset(&doc.raw, range.end);
                // apply_edit is best-effort for malformed ranges from a buggy
                // client: skip the change but keep processing the rest.
                let _ = doc.apply_edit(start, end, &change.text);
            } else {
                // No range → full document replacement.
                *doc = Document::parse(&change.text);
            }
        }
        *stored_ver = Some(version);
        drop(docs);

        // Schedule the debounced lint. The DebouncedLinter re-checks the
        // version before publishing so rapid edits only surface the final
        // diagnostics.
        self.debouncer
            .schedule(
                version,
                uri,
                self.documents.clone(),
                self.client.clone(),
                self.engine.clone(),
                DEBOUNCE_DELAY,
            )
            .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let raw = {
            let docs = self.documents.read().await;
            match docs.get(&uri) {
                Some((doc, _)) => doc.raw.clone(),
                None => return, // save for an unopened document — ignore
            }
        };
        let diags = self.engine.lint(&raw);
        self.client
            .publish_diagnostics(uri, diagnostics_to_lsp(&raw, diags), None)
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.debouncer.cancel(&uri).await;
        self.documents.write().await.remove(&uri);
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let doc = {
            let docs = self.documents.read().await;
            match docs.get(&uri) {
                Some((doc, _)) => doc.clone(),
                None => return Ok(None),
            }
        };

        let byte_offset = position::lsp_to_byte_offset(&doc.raw, position);
        let items = self.engine.complete_at(&doc, byte_offset);

        if items.is_empty() {
            return Ok(None);
        }

        let lsp_items: Vec<CompletionItem> = items
            .into_iter()
            .map(|ci| CompletionItem {
                label: ci.label,
                detail: ci.detail,
                ..Default::default()
            })
            .collect();

        Ok(Some(CompletionResponse::Array(lsp_items)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let doc = {
            let docs = self.documents.read().await;
            match docs.get(&uri) {
                Some((doc, _)) => doc.clone(),
                None => return Ok(None),
            }
        };

        let byte_offset = position::lsp_to_byte_offset(&doc.raw, position);
        let info = self.engine.hover_at(&doc, byte_offset);

        match info {
            Some(info) => {
                let mut parts = Vec::new();
                if let Some(desc) = info.description {
                    parts.push(desc);
                }
                if let Some(reason) = info.deprecated {
                    parts.push(format!("⚠ Deprecated: {reason}"));
                }
                if info.secret {
                    parts.push("🔒 Secret option".to_string());
                }
                let markdown = parts.join("\n\n");
                Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }),
                    range: None,
                }))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use camel_lint::{CapabilityQuery, ComponentMetadata, ComponentMetadataCatalog};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

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

    /// Write a JSON-RPC 2.0 frame (Content-Length header + body) to `w`.
    async fn write_frame(
        w: &mut (impl AsyncWrite + Unpin),
        content: &serde_json::Value,
    ) -> std::io::Result<()> {
        let body = serde_json::to_vec(content).unwrap(); // allow-unwrap — test helper
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        w.write_all(header.as_bytes()).await?;
        w.write_all(&body).await?;
        Ok(())
    }

    /// Read a single JSON-RPC 2.0 frame from `r`, returning the JSON body.
    async fn read_frame(r: &mut (impl AsyncRead + Unpin)) -> serde_json::Value {
        // Read until "\r\n\r\n"
        let mut raw = Vec::new();
        loop {
            let mut buf = [0u8; 1];
            r.read_exact(&mut buf).await.unwrap(); // allow-unwrap — test helper
            raw.push(buf[0]);
            if raw.len() >= 4 && raw[raw.len() - 4..] == *b"\r\n\r\n" {
                break;
            }
        }
        let header = String::from_utf8_lossy(&raw[..raw.len() - 4]); // strip trailer
        let content_length: usize = header
            .lines()
            .find(|l| l.to_lowercase().starts_with("content-length:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().parse().ok())
            .expect("Content-Length header required"); // allow-unwrap — test helper

        let mut body = vec![0u8; content_length];
        r.read_exact(&mut body).await.unwrap(); // allow-unwrap — test helper
        serde_json::from_slice(&body).unwrap() // allow-unwrap — test helper
    }

    #[tokio::test]
    async fn lsp_initialize_handshake() {
        let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(EmptyCatalog);
        let engine = LintEngine::new(catalog);
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Send initialize request
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": null,
                    "capabilities": {}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Read initialize response
        let response = read_frame(&mut client_read).await;
        let result = &response["result"];
        let caps = &result["capabilities"];

        assert_eq!(response["id"], 1);
        assert_eq!(caps["textDocumentSync"], 2); // TextDocumentSyncKind::INCREMENTAL = 2
        assert!(caps["completionProvider"].is_object());
        assert_eq!(caps["hoverProvider"], true);
        assert!(
            caps["diagnosticProvider"].is_null()
                || !caps.as_object().unwrap().contains_key("diagnosticProvider")
        );

        let server_info = &result["serverInfo"];
        assert_eq!(server_info["name"], "camel-lsp");
        assert!(server_info["version"].is_string());

        // Shutdown
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "shutdown"
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        let shutdown_resp = read_frame(&mut client_read).await;
        assert_eq!(shutdown_resp["id"], 2);
        assert!(shutdown_resp["result"].is_null());

        // Exit notification (no response expected)
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "exit"
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Server should shut down cleanly
        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    #[tokio::test]
    async fn lsp_shutdown_exits_clean() {
        let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(EmptyCatalog);
        let engine = LintEngine::new(catalog);
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Send initialize
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": null,
                    "capabilities": {}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init_resp = read_frame(&mut client_read).await;

        // Send "initialized" notification
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Shutdown
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "shutdown"
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let shutdown_resp = read_frame(&mut client_read).await;
        assert!(shutdown_resp["result"].is_null());

        // Exit
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "exit"
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        drop(client_write);
        drop(client_read);

        server_handle
            .await
            .expect("server task panicked during shutdown");
    }

    #[tokio::test]
    async fn did_open_valid_publishes_empty() {
        let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(EmptyCatalog);
        let engine = LintEngine::new(catalog);
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": null,
                    "capabilities": {}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init_resp = read_frame(&mut client_read).await;

        // Send initialized notification
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Send didOpen with valid route text (YAML with no route endpoints,
        // so the lint engine finds nothing to report).
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {
                    "textDocument": {
                        "uri": "file:///test.route.yaml",
                        "languageId": "camel-route",
                        "version": 1,
                        "text": "---\nkey: value\n"
                    }
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Read publishDiagnostics notification
        let notif = read_frame(&mut client_read).await;
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        let diags = &notif["params"]["diagnostics"];
        assert!(
            diags.is_array() && diags.as_array().unwrap().is_empty(),
            "expected empty diagnostics, got: {diags}"
        );
        assert_eq!(notif["params"]["uri"], "file:///test.route.yaml");
        assert_eq!(notif["params"]["version"], 1);

        // Shutdown + exit
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "shutdown"
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _shutdown_resp = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    #[tokio::test]
    async fn did_open_syntax_error_publishes_diagnostic() {
        let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(EmptyCatalog);
        let engine = LintEngine::new(catalog).with_default_rules();
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": null,
                    "capabilities": {}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init_resp = read_frame(&mut client_read).await;

        // Send initialized notification
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Send didOpen with text containing unclosed `[`
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {
                    "textDocument": {
                        "uri": "file:///test.route.yaml",
                        "languageId": "camel-route",
                        "version": 1,
                        "text": "steps:\n  - to: timer:foo\n  bad: ["
                    }
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Read publishDiagnostics notification
        let notif = read_frame(&mut client_read).await;
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        let diags = notif["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics should be an array");
        assert!(!diags.is_empty(), "expected at least one diagnostic");

        let has_error = diags.iter().any(|d| d["severity"] == 1); // DiagnosticSeverity::ERROR = 1
        assert!(has_error, "expected at least one Error-severity diagnostic");
        assert_eq!(notif["params"]["uri"], "file:///test.route.yaml");
        assert_eq!(notif["params"]["version"], 1);

        // Shutdown + exit
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "shutdown"
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _shutdown_resp = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    // ---- partial-input smoke tests (Phase 2 exit-criteria) ----

    #[tokio::test]
    async fn partial_input_empty_no_panic() {
        let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(EmptyCatalog);
        let engine = LintEngine::new(catalog).with_default_rules();
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init_resp = read_frame(&mut client_read).await;

        // Send initialized notification
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // didOpen with empty string — must not panic
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {
                    "textDocument": {
                        "uri": "file:///empty.route.yaml",
                        "languageId": "camel-route",
                        "version": 1,
                        "text": ""
                    }
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        let notif = read_frame(&mut client_read).await;
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        assert!(
            notif["params"]["diagnostics"].is_array(),
            "diagnostics should be an array"
        );

        // Shutdown + exit
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _shutdown_resp = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    #[tokio::test]
    async fn partial_input_truncated_yaml_no_panic() {
        let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(EmptyCatalog);
        let engine = LintEngine::new(catalog).with_default_rules();
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init_resp = read_frame(&mut client_read).await;

        // Send initialized notification
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // didOpen with truncated YAML value — must not panic
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {
                    "textDocument": {
                        "uri": "file:///truncated.route.yaml",
                        "languageId": "camel-route",
                        "version": 1,
                        "text": "from: timer:tick?period="
                    }
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        let notif = read_frame(&mut client_read).await;
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        let diags = notif["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics should be an array");
        assert!(
            !diags.is_empty(),
            "expected at least one diagnostic for truncated input"
        );

        // Shutdown + exit
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _shutdown_resp = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    #[tokio::test]
    async fn partial_input_syntax_error_at_multibyte_char_no_panic() {
        // Regression: a tab-indentation syntax error whose parser location lands
        // ON a multi-byte char (`だ`, bytes 20..23) makes R-SYN emit a span whose
        // end (`start + 1` = 21) is mid-char. Mapping that span through
        // `diagnostics_to_lsp` -> `byte_offset_to_lsp` previously panicked on the
        // `source[..21]` slice, crashing the handler. The server must publish
        // diagnostics and stay alive.
        let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(EmptyCatalog);
        let engine = LintEngine::new(catalog).with_default_rules();
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init_resp = read_frame(&mut client_read).await;

        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Syntax error whose location falls on a multi-byte char.
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {
                    "textDocument": {
                        "uri": "file:///multibyte.route.yaml",
                        "languageId": "camel-route",
                        "version": 1,
                        "text": "root:\n  child: val\n\tだめ: x"
                    }
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        let notif = read_frame(&mut client_read).await;
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        let diags = notif["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics should be an array");
        assert!(
            !diags.is_empty(),
            "expected at least one syntax diagnostic for the tab-indent error"
        );

        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _shutdown_resp = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    // ---- did_change helpers + tests ----

    const DOC_URI: &str = "file:///t.route.yaml";

    fn make_engine() -> LintEngine {
        let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(EmptyCatalog);
        LintEngine::new(catalog).with_default_rules()
    }

    /// Spawn a server on an in-memory duplex, run the initialize handshake,
    /// and open a document with `initial_text` at version 1.
    ///
    /// Returns the client IO halves, the server task handle, and a clone of
    /// the `Backend` whose `documents` field is shared (via `Arc`) with the
    /// live server — so tests can inspect stored document state directly.
    #[allow(clippy::type_complexity)]
    async fn start_server_with_open_doc(
        initial_text: &str,
    ) -> (
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
        tokio::io::ReadHalf<tokio::io::DuplexStream>,
        tokio::task::JoinHandle<()>,
        Backend,
    ) {
        let engine = make_engine();
        let captured = Arc::new(std::sync::Mutex::new(None::<Backend>));
        let cap = captured.clone();
        let (service, socket) = tower_lsp::LspService::new(move |client| {
            let backend = Backend::new(client, engine);
            *cap.lock().unwrap() = Some(backend.clone()); // allow-unwrap — test helper
            backend
        });
        // LspService::new calls the closure synchronously, so the clone is
        // available now. Extract it; the documents Arc is shared with the
        // server's copy.
        let backend = captured.lock().unwrap().take().unwrap(); // allow-unwrap — test helper

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);
        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize handshake
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // didOpen at version 1
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {"textDocument": {
                    "uri": DOC_URI, "languageId": "camel-route",
                    "version": 1, "text": initial_text
                }}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
                   // Consume the didOpen publishDiagnostics so it doesn't sit in the buffer.
        let _diags = read_frame(&mut client_read).await;

        (client_write, client_read, server_handle, backend)
    }

    /// Send shutdown + exit, drop the client IO (closing the read end on the
    /// server), and wait for the server task to terminate. Taking ownership
    /// of the IO halves is essential — the server's serve loop only returns
    /// after its read half sees EOF.
    async fn finish_server(
        mut client_write: impl AsyncWrite + Unpin,
        client_read: impl AsyncRead + Unpin,
        server_handle: tokio::task::JoinHandle<()>,
    ) {
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "id": 99, "method": "shutdown"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    #[tokio::test]
    async fn did_change_range_edit_updates_document() {
        // "from: direct:start\n"
        //  byte layout: from: =0-5, direct: =6-12, start =13-17, \n=18
        // LSP position for ASCII: line 0, character == byte offset.
        let (mut cw, cr, handle, backend) =
            start_server_with_open_doc("from: direct:start\n").await;

        // Replace "start" (line 0, chars 13–18) with "end".
        write_frame(
            &mut cw,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didChange",
                "params": {
                    "textDocument": {"uri": DOC_URI, "version": 2},
                    "contentChanges": [{
                        "range": {"start": {"line": 0, "character": 13},
                                  "end":   {"line": 0, "character": 18}},
                        "text": "end"
                    }]
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        finish_server(cw, cr, handle).await;

        // Inspect stored document state (Arc still alive via backend clone).
        let docs = backend.documents.read().await;
        let (doc, ver) = docs
            .get(&Url::parse(DOC_URI).unwrap()) // allow-unwrap — test constant
            .expect("document should be open");
        assert_eq!(doc.raw, "from: direct:end\n");
        assert_eq!(*ver, Some(2));
    }

    #[tokio::test]
    async fn did_change_full_replacement_replaces() {
        let (mut cw, cr, handle, backend) =
            start_server_with_open_doc("from: direct:start\n").await;

        // Full replacement (no range).
        write_frame(
            &mut cw,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didChange",
                "params": {
                    "textDocument": {"uri": DOC_URI, "version": 2},
                    "contentChanges": [{"text": "---\nkey: value\n"}]
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        finish_server(cw, cr, handle).await;

        let docs = backend.documents.read().await;
        let (doc, ver) = docs
            .get(&Url::parse(DOC_URI).unwrap()) // allow-unwrap — test constant
            .expect("document should be open");
        assert_eq!(doc.raw, "---\nkey: value\n");
        assert_eq!(*ver, Some(2));
    }

    #[tokio::test]
    async fn did_change_rapid_sequence_final_state() {
        // Open "aaaaa\n"; apply 5 sequential single-char range edits to turn
        // each 'a' into 'b', one per didChange (versions 2–6). Each edit
        // depends on the prior state, so all five must be processed in order
        // for the final document to be "bbbbb\n".
        let (mut cw, cr, handle, backend) = start_server_with_open_doc("aaaaa\n").await;

        for i in 0..5i32 {
            let version = 2 + i;
            let char_start = i;
            let char_end = i + 1;
            write_frame(
                &mut cw,
                &serde_json::json!({
                    "jsonrpc": "2.0", "method": "textDocument/didChange",
                    "params": {
                        "textDocument": {"uri": DOC_URI, "version": version},
                        "contentChanges": [{
                            "range": {
                                "start": {"line": 0, "character": char_start},
                                "end":   {"line": 0, "character": char_end}
                            },
                            "text": "b"
                        }]
                    }
                }),
            )
            .await
            .unwrap(); // allow-unwrap — test helper
        }

        finish_server(cw, cr, handle).await;

        let docs = backend.documents.read().await;
        let (doc, ver) = docs
            .get(&Url::parse(DOC_URI).unwrap()) // allow-unwrap — test constant
            .expect("document should be open");
        assert_eq!(doc.raw, "bbbbb\n");
        assert_eq!(*ver, Some(6));
    }

    // ---- did_save tests ----

    #[tokio::test]
    async fn did_save_republishes_diagnostics() {
        // Open a document with a syntax error so we can verify diagnostics
        // are re-published on save.
        let (mut cw, mut cr, handle, _backend) =
            start_server_with_open_doc("steps:\n  - to: timer:foo\n  bad: [").await;

        // Send didSave
        write_frame(
            &mut cw,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didSave",
                "params": {
                    "textDocument": {"uri": DOC_URI}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Read publishDiagnostics notification — didSave triggers a fresh
        // publish even if the diagnostics are empty (the engine used by
        // start_server_with_open_doc has no default rules).
        let notif = read_frame(&mut cr).await;
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        assert!(
            notif["params"]["diagnostics"].is_array(),
            "diagnostics should be an array"
        );
        assert_eq!(notif["params"]["uri"], DOC_URI);
        // didSave publishes without a version
        assert!(notif["params"].get("version").is_none() || notif["params"]["version"].is_null());

        finish_server(cw, cr, handle).await;
    }

    #[tokio::test]
    async fn did_save_unopened_document_does_not_publish() {
        // Send didSave for a document that was never opened.
        let engine = make_engine();
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Send didSave for an unopened document — should be silently ignored.
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didSave",
                "params": {
                    "textDocument": {"uri": "file:///never.opened.yaml"}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // No notification should arrive. Use a short timeout to verify.
        let result =
            tokio::time::timeout(Duration::from_millis(200), read_frame(&mut client_read)).await;
        assert!(
            result.is_err(),
            "expected no notification for unopened document save"
        );

        finish_server(client_write, client_read, server_handle).await;
    }

    // ---- did_close tests ----

    #[tokio::test]
    async fn did_close_clears_diagnostics() {
        let (mut cw, mut cr, handle, backend) =
            start_server_with_open_doc("steps:\n  - to: timer:foo\n  bad: [").await;

        // Send didClose
        write_frame(
            &mut cw,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didClose",
                "params": {
                    "textDocument": {"uri": DOC_URI}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Read publishDiagnostics — should be empty array
        let notif = read_frame(&mut cr).await;
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        let diags = notif["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics should be an array");
        assert!(diags.is_empty(), "expected empty diagnostics on close");
        assert_eq!(notif["params"]["uri"], DOC_URI);

        // Document should be removed from the map
        let docs = backend.documents.read().await;
        assert!(
            !docs.contains_key(&Url::parse(DOC_URI).unwrap()), // allow-unwrap — test constant
            "document should be removed after close"
        );

        finish_server(cw, cr, handle).await;
    }

    #[tokio::test]
    async fn did_close_cancels_pending_lint() {
        let (mut cw, mut cr, handle, _backend) =
            start_server_with_open_doc("from: direct:start\n").await;

        // Send didChange to schedule a debounced lint (50ms debounce).
        write_frame(
            &mut cw,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didChange",
                "params": {
                    "textDocument": {"uri": DOC_URI, "version": 2},
                    "contentChanges": [{"text": "from: direct:end\n"}]
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Immediately send didClose — should cancel the pending lint.
        write_frame(
            &mut cw,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didClose",
                "params": {
                    "textDocument": {"uri": DOC_URI}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Read the empty diagnostics from didClose.
        let notif = read_frame(&mut cr).await;
        assert_eq!(notif["method"], "textDocument/publishDiagnostics");
        assert!(
            notif["params"]["diagnostics"]
                .as_array()
                .expect("diagnostics should be an array")
                .is_empty(),
            "expected empty diagnostics on close"
        );

        // Wait past the debounce delay (50ms) to ensure the cancelled task
        // does not publish stale diagnostics.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // No more notifications should arrive. Use a short timeout.
        let result = tokio::time::timeout(Duration::from_millis(200), read_frame(&mut cr)).await;
        assert!(
            result.is_err(),
            "expected no diagnostics after didClose — pending lint was cancelled"
        );

        finish_server(cw, cr, handle).await;
    }

    // ---- completion tests ----

    struct StubCatalog {
        schemes: HashMap<String, ComponentMetadata>,
    }

    impl StubCatalog {
        fn new() -> Self {
            Self {
                schemes: HashMap::new(),
            }
        }

        fn with(mut self, scheme: &str, meta: ComponentMetadata) -> Self {
            self.schemes.insert(scheme.to_string(), meta);
            self
        }
    }

    impl ComponentMetadataCatalog for StubCatalog {
        fn get_metadata(&self, scheme: &str) -> Option<ComponentMetadata> {
            self.schemes.get(scheme).cloned()
        }

        fn schemes(&self) -> Vec<String> {
            self.schemes.keys().cloned().collect()
        }

        fn all_metadata(&self) -> Vec<ComponentMetadata> {
            self.schemes.values().cloned().collect()
        }

        fn query_capabilities(&self, _query: &CapabilityQuery) -> Vec<ComponentMetadata> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn completion_in_scheme_position_returns_candidates() {
        let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(
            StubCatalog::new()
                .with("timer", ComponentMetadata::minimal("timer"))
                .with("log", ComponentMetadata::minimal("log"))
                .with("direct", ComponentMetadata::minimal("direct")),
        );
        let engine = LintEngine::new(catalog);
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Open doc "from: tim"
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {"textDocument": {
                    "uri": "file:///t.route.yaml", "languageId": "camel-route",
                    "version": 1, "text": "from: tim"
                }}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _diags = read_frame(&mut client_read).await;

        // Send completion at byte 7 (inside "tim")
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
                "params": {
                    "textDocument": {"uri": "file:///t.route.yaml"},
                    "position": {"line": 0, "character": 7}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        let response = read_frame(&mut client_read).await;
        assert_eq!(response["id"], 2);
        let items = response["result"]
            .as_array()
            .expect("result should be an array");
        let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert!(
            labels.contains(&"timer"),
            "expected 'timer' in completions; got: {labels:?}"
        );
        assert!(
            labels.contains(&"log"),
            "expected 'log' in completions; got: {labels:?}"
        );
        assert!(
            labels.contains(&"direct"),
            "expected 'direct' in completions; got: {labels:?}"
        );

        // Shutdown + exit
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "id": 99, "method": "shutdown"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    #[tokio::test]
    async fn completion_outside_uri_returns_none() {
        let catalog: Arc<dyn ComponentMetadataCatalog> =
            Arc::new(StubCatalog::new().with("timer", ComponentMetadata::minimal("timer")));
        let engine = LintEngine::new(catalog);
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Open doc with YAML key outside URI span
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {"textDocument": {
                    "uri": "file:///t.route.yaml", "languageId": "camel-route",
                    "version": 1, "text": "id: r1\nfrom: timer:foo\n"
                }}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _diags = read_frame(&mut client_read).await;

        // Send completion at byte 1 (inside "id" key — outside URI span)
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
                "params": {
                    "textDocument": {"uri": "file:///t.route.yaml"},
                    "position": {"line": 0, "character": 1}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        let response = read_frame(&mut client_read).await;
        assert_eq!(response["id"], 2);
        assert!(
            response["result"].is_null(),
            "expected null result for cursor outside URI span; got: {}",
            response["result"]
        );

        // Shutdown + exit
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "id": 99, "method": "shutdown"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    #[tokio::test]
    async fn completion_on_closed_doc_returns_none() {
        let catalog: Arc<dyn ComponentMetadataCatalog> =
            Arc::new(StubCatalog::new().with("timer", ComponentMetadata::minimal("timer")));
        let engine = LintEngine::new(catalog);
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize only — no document opened
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Send completion for a URI that was never opened
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
                "params": {
                    "textDocument": {"uri": "file:///never.opened.yaml"},
                    "position": {"line": 0, "character": 0}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        let response = read_frame(&mut client_read).await;
        assert_eq!(response["id"], 2);
        assert!(
            response["result"].is_null(),
            "expected null result for closed doc; got: {}",
            response["result"]
        );

        // Shutdown + exit
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "id": 99, "method": "shutdown"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    // ---- hover tests ----

    #[tokio::test]
    async fn hover_on_documented_option_returns_markdown() {
        use camel_lint::{OptionKind, UriOption};

        let catalog: Arc<dyn ComponentMetadataCatalog> = Arc::new(StubCatalog::new().with(
            "timer",
            ComponentMetadata::minimal("timer").with_uri_options(vec![UriOption::new(
                "period",
                "The tick interval",
                OptionKind::Duration,
            )]),
        ));
        let engine = LintEngine::new(catalog);
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Open doc "from: timer:tick?period=1s"
        //  byte layout: from: =0-5, timer =6-10, :tick =11-15, ? =16, period =17-22, =1s =23-25
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {"textDocument": {
                    "uri": "file:///t.route.yaml", "languageId": "camel-route",
                    "version": 1, "text": "from: timer:tick?period=1s"
                }}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _diags = read_frame(&mut client_read).await;

        // Send hover at character 17 (inside "period" key)
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "textDocument/hover",
                "params": {
                    "textDocument": {"uri": "file:///t.route.yaml"},
                    "position": {"line": 0, "character": 17}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        let response = read_frame(&mut client_read).await;
        assert_eq!(response["id"], 2);
        let hover = response["result"]
            .as_object()
            .expect("expected non-null hover result");
        let contents = &hover["contents"];
        assert_eq!(contents["kind"], "markdown");
        let value = contents["value"]
            .as_str()
            .expect("markdown value should be a string");
        assert!(
            value.contains("The tick interval"),
            "expected description in hover markdown; got: {value}"
        );

        // Shutdown + exit
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "id": 99, "method": "shutdown"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }

    #[tokio::test]
    async fn hover_outside_option_returns_none() {
        let catalog: Arc<dyn ComponentMetadataCatalog> =
            Arc::new(StubCatalog::new().with("timer", ComponentMetadata::minimal("timer")));
        let engine = LintEngine::new(catalog);
        let (service, socket) =
            tower_lsp::LspService::new(move |client| Backend::new(client, engine));

        let (client_side, server_side) = tokio::io::duplex(8192);
        let (server_read, server_write) = tokio::io::split(server_side);
        let (mut client_read, mut client_write) = tokio::io::split(client_side);

        let server_handle = tokio::spawn(async move {
            tower_lsp::Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });

        // Initialize
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _init = read_frame(&mut client_read).await;
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        // Open doc "from: timer:tick?period=1s"
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {"textDocument": {
                    "uri": "file:///t.route.yaml", "languageId": "camel-route",
                    "version": 1, "text": "from: timer:tick?period=1s"
                }}
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        let _diags = read_frame(&mut client_read).await;

        // Send hover at character 0 (inside scheme "from" — outside option key)
        write_frame(
            &mut client_write,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "textDocument/hover",
                "params": {
                    "textDocument": {"uri": "file:///t.route.yaml"},
                    "position": {"line": 0, "character": 0}
                }
            }),
        )
        .await
        .unwrap(); // allow-unwrap — test helper

        let response = read_frame(&mut client_read).await;
        assert_eq!(response["id"], 2);
        assert!(
            response["result"].is_null(),
            "expected null result for cursor outside option key; got: {}",
            response["result"]
        );

        // Shutdown + exit
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "id": 99, "method": "shutdown"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        write_frame(
            &mut client_write,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        )
        .await
        .unwrap(); // allow-unwrap — test helper
        drop(client_write);
        drop(client_read);
        let _ = tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server did not shut down within 5s");
    }
}
