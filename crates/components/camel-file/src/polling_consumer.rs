//! On-demand pull-based consumer for the file component.
//!
//! Implements [`camel_component_api::endpoint::PollingConsumer`] by reusing
//! the shared `poll_one_file` helper from `poll_logic`. Used by the EIP-7
//! `pollEnrich` DSL verb (via `Endpoint::polling_consumer`) and by the WASM
//! `camel_poll` host function.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;

use camel_component_api::endpoint::PollingConsumer;
use camel_component_api::{CamelError, Exchange};

use crate::CompiledFilters;
use crate::FileConfig;
use crate::poll_logic::{apply_sort_and_limit, poll_one_file, scan_candidates};

/// Pull-based consumer that scans the configured directory on each call
/// to [`receive`](PollingConsumer::receive) and returns the first matching
/// file's exchange.
///
/// Lifecycle (delete, move, idempotency-mark, path traversal protection) is
/// applied **eagerly** inside `receive` before the exchange is returned,
/// matching the existing event-driven `FileConsumer` semantics.
pub(crate) struct FilePollingConsumer {
    config: FileConfig,
    seen: HashSet<PathBuf>,
    in_process_locks: Arc<DashMap<PathBuf, ()>>,
    idempotent_repo: Arc<tokio::sync::Mutex<HashSet<String>>>,
    filters: CompiledFilters,
}

impl FilePollingConsumer {
    pub(crate) fn new(
        config: FileConfig,
        in_process_locks: Arc<DashMap<PathBuf, ()>>,
        idempotent_repo: Arc<tokio::sync::Mutex<HashSet<String>>>,
        filters: CompiledFilters,
    ) -> Self {
        Self {
            config,
            seen: HashSet::new(),
            in_process_locks,
            idempotent_repo,
            filters,
        }
    }
}

#[async_trait]
impl PollingConsumer for FilePollingConsumer {
    async fn receive(&mut self, timeout: Duration) -> Result<Option<Exchange>, CamelError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let base_path = Path::new(&self.config.directory);

        loop {
            let scan_result =
                scan_candidates(&self.config, &self.filters, base_path, &mut self.seen).await?;

            let mut candidates = scan_result.candidates;
            apply_sort_and_limit(&mut candidates, &self.config);

            for candidate in candidates {
                if let Some(exchange) = poll_one_file(
                    &self.config,
                    candidate.path,
                    base_path,
                    &self.filters.include_re,
                    &self.filters.exclude_re,
                    &mut self.seen,
                    &self.in_process_locks,
                    &self.idempotent_repo,
                )
                .await?
                {
                    return Ok(Some(exchange));
                }
            }

            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(None);
            }

            let next = deadline.min(now + self.config.delay);
            tokio::time::sleep_until(next).await;
        }
    }
}
