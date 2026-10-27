//! Split metadata property keys.
//!
//! The eager Split EIP runs as [`crate::SplitSegment`]; these constants are
//! shared by the segment and by downstream metadata readers.

// ── Metadata property keys ─────────────────────────────────────────────

/// Property key for the zero-based index of a fragment within the split.
pub const CAMEL_SPLIT_INDEX: &str = "CamelSplitIndex";
/// Property key for the total number of fragments produced by the split.
pub const CAMEL_SPLIT_SIZE: &str = "CamelSplitSize";
/// Property key indicating whether this fragment is the last one.
pub const CAMEL_SPLIT_COMPLETE: &str = "CamelSplitComplete";
