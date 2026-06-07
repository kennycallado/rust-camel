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
use regex::Regex;

use camel_component_api::endpoint::PollingConsumer;
use camel_component_api::{CamelError, Exchange};

use crate::FileConfig;
use crate::poll_logic::{list_files, poll_one_file};

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
    include_re: Option<Regex>,
    exclude_re: Option<Regex>,
}

impl FilePollingConsumer {
    pub(crate) fn new(
        config: FileConfig,
        in_process_locks: Arc<DashMap<PathBuf, ()>>,
        idempotent_repo: Arc<tokio::sync::Mutex<HashSet<String>>>,
    ) -> Self {
        // Compile regex patterns once at construction time.
        let include_re = config
            .include
            .as_ref()
            .map(|p| Regex::new(p))
            .transpose()
            .expect("invalid include regex in FileConfig");

        let exclude_re = config
            .exclude
            .as_ref()
            .map(|p| Regex::new(p))
            .transpose()
            .expect("invalid exclude regex in FileConfig");

        Self {
            config,
            seen: HashSet::new(),
            in_process_locks,
            idempotent_repo,
            include_re,
            exclude_re,
        }
    }
}

#[async_trait]
impl PollingConsumer for FilePollingConsumer {
    async fn receive(&mut self, timeout: Duration) -> Result<Option<Exchange>, CamelError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let base_path = Path::new(&self.config.directory);

        loop {
            let files = list_files(base_path, self.config.recursive).await?;

            for file_path in &files {
                if let Some(exchange) = poll_one_file(
                    &self.config,
                    file_path.clone(),
                    base_path,
                    &self.include_re,
                    &self.exclude_re,
                    &mut self.seen,
                    &self.in_process_locks,
                    &self.idempotent_repo,
                )
                .await?
                {
                    return Ok(Some(exchange));
                }
            }

            // No file matched this iteration — check timeout.
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(None);
            }

            // Sleep until deadline or the configured poll delay, whichever is sooner.
            let next = deadline.min(now + self.config.delay);
            tokio::time::sleep_until(next).await;
        }
    }
}
