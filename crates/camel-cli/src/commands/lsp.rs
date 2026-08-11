//! `camel lsp` — start the LSP server over stdio.
//!
//! Builds the production lint engine, constructs a tower-lsp `LspService` backed
//! by [`camel_lsp::Backend`], and runs the server on stdin/stdout.

/// Start the LSP server over stdio.
///
/// Returns the exit code: 0 on clean shutdown, 2 on engine init failure.
pub async fn run() -> i32 {
    let engine = match crate::commands::lint::production_engine().await {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let (service, socket) =
        tower_lsp::LspService::new(move |client| camel_lsp::Backend::new(client, engine));
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    tower_lsp::Server::new(stdin, stdout, socket)
        .serve(service)
        .await;
    0
}
