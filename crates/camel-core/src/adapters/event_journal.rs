use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use camel_api::CamelError;

use crate::domain::RuntimeEvent;
use crate::ports::RuntimeEventJournalPort;

static JOURNAL_LOCKS: OnceLock<std::sync::Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    OnceLock::new();
const DEFAULT_COMMAND_COMPACTION_THRESHOLD_BYTES: u64 = 256 * 1024;

fn lock_for_path(path: &Path) -> Arc<Mutex<()>> {
    let map = JOURNAL_LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut guard = map
        .lock()
        .expect("runtime event journal lock map poisoned unexpectedly");
    guard
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[derive(Clone)]
pub struct FileRuntimeEventJournal {
    path: PathBuf,
    command_path: PathBuf,
    write_lock: Arc<Mutex<()>>,
    command_compaction_threshold_bytes: u64,
}

impl FileRuntimeEventJournal {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, CamelError> {
        Self::with_compaction_threshold(path, DEFAULT_COMMAND_COMPACTION_THRESHOLD_BYTES)
    }

    fn with_compaction_threshold(
        path: impl Into<PathBuf>,
        command_compaction_threshold_bytes: u64,
    ) -> Result<Self, CamelError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let command_path = path.with_extension(
            path.extension()
                .map(|ext| format!("{}{}", ext.to_string_lossy(), ".cmdids"))
                .unwrap_or_else(|| "cmdids".to_string()),
        );
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&command_path)?;
        let canonical_path = std::fs::canonicalize(&path).unwrap_or(path.clone());

        Ok(Self {
            path,
            command_path,
            write_lock: lock_for_path(&canonical_path),
            command_compaction_threshold_bytes,
        })
    }

    fn parse_seen_command_ids(data: &str) -> HashSet<String> {
        let mut seen = HashSet::new();
        for raw_line in data.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('+') {
                seen.insert(rest.to_string());
                continue;
            }
            if let Some(rest) = line.strip_prefix('-') {
                seen.remove(rest);
                continue;
            }
            // Backward compatibility for older plain-line format.
            seen.insert(line.to_string());
        }
        seen
    }

    async fn compact_command_log_if_needed(&self) -> Result<(), CamelError> {
        let metadata = match fs::metadata(&self.command_path).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(CamelError::from(err)),
        };

        if metadata.len() < self.command_compaction_threshold_bytes {
            return Ok(());
        }

        let data = match fs::read_to_string(&self.command_path).await {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(CamelError::from(err)),
        };

        let seen = Self::parse_seen_command_ids(&data);
        let mut ids: Vec<String> = seen.into_iter().collect();
        ids.sort();

        let mut compacted = String::new();
        for id in ids {
            compacted.push('+');
            compacted.push_str(&id);
            compacted.push('\n');
        }
        fs::write(&self.command_path, compacted).await?;
        Ok(())
    }
}

#[async_trait]
impl RuntimeEventJournalPort for FileRuntimeEventJournal {
    async fn append_batch(&self, events: &[RuntimeEvent]) -> Result<(), CamelError> {
        if events.is_empty() {
            return Ok(());
        }

        let _guard = self.write_lock.lock().await;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;

        for event in events {
            let encoded = serde_json::to_string(event).map_err(|err| {
                CamelError::RouteError(format!("event journal encode failed: {err}"))
            })?;
            file.write_all(encoded.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
        Ok(())
    }

    async fn load_all(&self) -> Result<Vec<RuntimeEvent>, CamelError> {
        let _guard = self.write_lock.lock().await;
        let data = match fs::read_to_string(&self.path).await {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(CamelError::from(err)),
        };

        let mut events = Vec::new();
        let stream = serde_json::Deserializer::from_str(&data).into_iter::<RuntimeEvent>();
        for event in stream {
            events.push(event.map_err(|err| {
                CamelError::RouteError(format!("event journal decode failed: {err}"))
            })?);
        }

        Ok(events)
    }

    async fn append_command_id(&self, command_id: &str) -> Result<(), CamelError> {
        let _guard = self.write_lock.lock().await;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.command_path)
            .await?;
        file.write_all(b"+").await?;
        file.write_all(command_id.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        self.compact_command_log_if_needed().await?;
        Ok(())
    }

    async fn remove_command_id(&self, command_id: &str) -> Result<(), CamelError> {
        let _guard = self.write_lock.lock().await;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.command_path)
            .await?;
        file.write_all(b"-").await?;
        file.write_all(command_id.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        self.compact_command_log_if_needed().await?;
        Ok(())
    }

    async fn load_command_ids(&self) -> Result<Vec<String>, CamelError> {
        let _guard = self.write_lock.lock().await;
        let data = match fs::read_to_string(&self.command_path).await {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(CamelError::from(err)),
        };

        let seen = Self::parse_seen_command_ids(&data);

        Ok(seen.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn file_runtime_event_journal_roundtrip() {
        let dir = tempdir().unwrap();
        let journal =
            FileRuntimeEventJournal::new(dir.path().join("runtime-events.jsonl")).unwrap();

        journal
            .append_batch(&[RuntimeEvent::RouteRegistered {
                route_id: "j1".to_string(),
            }])
            .await
            .unwrap();

        let events = journal.load_all().await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            RuntimeEvent::RouteRegistered { route_id } if route_id == "j1"
        ));

        journal.append_command_id("c1").await.unwrap();
        journal.append_command_id("c2").await.unwrap();
        journal.remove_command_id("c1").await.unwrap();
        let command_ids = journal.load_command_ids().await.unwrap();
        assert_eq!(command_ids, vec!["c2".to_string()]);
    }

    #[tokio::test]
    async fn load_all_accepts_concatenated_json_values() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("runtime-events.jsonl");
        std::fs::write(
            &path,
            "{\"RouteRegistered\":{\"route_id\":\"r1\"}}{\"RouteStarted\":{\"route_id\":\"r1\"}}\n",
        )
        .unwrap();
        let journal = FileRuntimeEventJournal::new(path).unwrap();

        let events = journal.load_all().await.unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            RuntimeEvent::RouteRegistered { route_id } if route_id == "r1"
        ));
        assert!(matches!(
            &events[1],
            RuntimeEvent::RouteStarted { route_id } if route_id == "r1"
        ));
    }

    #[tokio::test]
    async fn command_id_log_compacts_to_live_ids_only() {
        let dir = tempdir().unwrap();
        let journal = FileRuntimeEventJournal::with_compaction_threshold(
            dir.path().join("runtime-events.jsonl"),
            1,
        )
        .unwrap();

        for i in 0..20 {
            let cmd = format!("old-{i}");
            journal.append_command_id(&cmd).await.unwrap();
            journal.remove_command_id(&cmd).await.unwrap();
        }
        journal.append_command_id("live-1").await.unwrap();
        journal.append_command_id("live-2").await.unwrap();

        let command_ids = journal.load_command_ids().await.unwrap();
        let mut sorted = command_ids;
        sorted.sort();
        assert_eq!(sorted, vec!["live-1".to_string(), "live-2".to_string()]);

        let compacted = fs::read_to_string(&journal.command_path).await.unwrap();
        let compacted_lines: Vec<&str> = compacted
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(compacted_lines, vec!["+live-1", "+live-2"]);
    }
}
