//! Resequencer EIP contract types.
//!
//! Batch/stream policy configs (Tasks 2/3).

/// Window-based completion trigger for batch resequencing.
///
/// Narrowed to size and/or timeout — NOT the general `CompletionCondition`
/// (which carries a `Predicate` variant semantically wrong for a
/// resequencer window). Timeout values are in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
pub enum BatchCompletion {
    /// Emit when `size` exchanges accumulate for a correlation key.
    Size(usize),
    /// Emit after `timeout_ms` since the first exchange for a correlation key.
    Timeout(u64),
    /// Emit when EITHER condition is met first.
    SizeOrTimeout(usize, u64),
}

/// What to do when a stream resequencer gap timer fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
pub enum GapPolicy {
    /// Emit the contiguous run from `next_expected` and advance past the gap.
    EmitPartial,
    /// Drop all held exchanges with a warning log (no dead-letter sink wired).
    DropAndLog,
}

/// What to do when the stream resequencer priority queue reaches capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
pub enum CapacityPolicy {
    /// Log a warning and drop the incoming exchange (no dead-letter sink wired).
    LogAndDrop,
    /// Drop the oldest exchange from the queue to make room.
    DropOldest,
}

/// Configurable resequencing policy mode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
pub enum ResequenceMode {
    /// Window-based batch resequencing.
    Batch {
        /// Simple-language expression for the correlation key
        /// (e.g. `"${header.region}"`).
        correlation: String,
        /// Simple-language expression for the sort key
        /// (e.g. `"${header.sequence}"`).
        sort: String,
        /// Window completion trigger.
        completion: BatchCompletion,
    },
    /// Stream resequencing — bounded priority queue with gap detection.
    Stream {
        /// Simple-language expression for the sequence number
        /// (e.g. `"${header.seqNum}"`). Must evaluate to a u64.
        sequence: String,
        /// Maximum queue size (default 1000).
        capacity: usize,
        /// Gap timeout in milliseconds (default 5000).
        gap_timeout: u64,
        /// What to do when a gap timer fires.
        on_gap: GapPolicy,
        /// What to do when the queue reaches capacity.
        on_capacity_exceeded: CapacityPolicy,
        /// When true, duplicate/late sequence numbers are ignored.
        /// Default false (Camel 4.x behavior: duplicates are inserted).
        dedup: bool,
    },
}

/// Configuration for the Resequencer EIP.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema, ts_rs::TS))]
pub struct ResequencePolicyConfig {
    pub mode: ResequenceMode,
}

impl Default for ResequencePolicyConfig {
    /// Safe-ish defaults: batch mode by correlation + sort on `header.id`,
    /// 100-size window with 30s timeout fallback.
    fn default() -> Self {
        Self {
            mode: ResequenceMode::Batch {
                correlation: "header.id".into(),
                sort: "header.id".into(),
                completion: BatchCompletion::SizeOrTimeout(100, 30_000),
            },
        }
    }
}
