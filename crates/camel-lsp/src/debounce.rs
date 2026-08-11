//! Debounced lint scheduler for `didChange` notifications.
//!
//! [`DebouncedLinter`] coalesces rapid edits to the same document so the lint
//! engine runs at most once per debounce window. It also enforces
//! **version-ordered publication**: the scheduled task clones document text
//! under a brief read lock, runs lint WITHOUT holding the lock, then
//! re-acquires the lock for a single version check before publishing. This
//! ensures the lint pass never blocks concurrent writes (didChange, didOpen,
//! didClose) and that a stale result arriving after a newer edit is discarded.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tower_lsp::lsp_types::Url;

/// Pending lint task handle keyed by document URI.
type PendingMap = HashMap<Url, (i32, JoinHandle<()>)>;

/// Coalesces `didChange` events and publishes version-consistent diagnostics.
///
/// Clone-safe: the internal pending map lives behind an [`Arc`] so the same
/// [`DebouncedLinter`] can be cheaply cloned into each `Backend` instance that
/// tower-lsp spins up per request.
#[derive(Clone)]
pub struct DebouncedLinter {
    pending: Arc<Mutex<PendingMap>>,
}

impl DebouncedLinter {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Schedule a debounced lint for `uri` at `version`.
    ///
    /// Cancels any previously-scheduled task for the same URI, then spawns a
    /// task that:
    ///
    /// 1. Sleeps for `delay`.
    /// 2. Clones the document text under a brief read lock and confirms it is
    ///    still at `version` (staleness gate BEFORE lint).
    /// 3. Runs the synchronous lint pass WITHOUT holding any lock — does not
    ///    block concurrent writes.
    /// 4. Re-acquires the read lock for a single version check — if a newer
    ///    edit arrived DURING linting, the result is discarded.
    /// 5. Publishes diagnostics tagged with `version`.
    #[allow(clippy::too_many_arguments)]
    pub async fn schedule(
        &self,
        version: i32,
        uri: Url,
        documents: Arc<RwLock<super::DocumentState>>,
        client: tower_lsp::Client,
        engine: Arc<camel_lint::LintEngine>,
        delay: Duration,
    ) {
        // Cancel any in-flight task for this URI before scheduling a new one.
        let mut pending = self.pending.lock().await;
        if let Some((_, handle)) = pending.remove(&uri) {
            handle.abort();
        }

        let pending_clone = self.pending.clone();
        let task_uri = uri.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;

            // Clone document text under a brief read lock, then drop it.
            let (raw, ver_at_schedule) = {
                let docs = documents.read().await;
                match docs.get(&task_uri) {
                    Some((doc, ver)) => (doc.raw.clone(), *ver),
                    None => return, // document was closed
                }
            };

            // Staleness check before lint.
            if ver_at_schedule != Some(version) {
                return; // a newer edit superseded us
            }

            // Lint WITHOUT holding the read lock — does not block writers.
            let diags = engine.lint(&raw);

            // Re-acquire for a single version check; discard if stale.
            let still_current = {
                let docs = documents.read().await;
                match docs.get(&task_uri) {
                    Some((_, ver)) => *ver == Some(version),
                    None => false, // closed during lint
                }
            };

            if !still_current {
                return; // a newer edit arrived during lint — discard
            }

            let lsp_diags = super::diagnostics_to_lsp(&raw, diags);
            client
                .publish_diagnostics(task_uri.clone(), lsp_diags, Some(version))
                .await;

            // Clean up our entry so the next schedule sees an empty slot.
            let mut pending = pending_clone.lock().await;
            pending.remove(&task_uri);
        });

        pending.insert(uri, (version, handle));
    }

    /// Cancel any pending lint for `uri` (used on `didClose`).
    pub async fn cancel(&self, uri: &Url) {
        let mut pending = self.pending.lock().await;
        if let Some((_, handle)) = pending.remove(uri) {
            handle.abort();
        }
    }
}

impl Default for DebouncedLinter {
    fn default() -> Self {
        Self::new()
    }
}
