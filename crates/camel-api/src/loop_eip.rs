use crate::filter::FilterPredicate;

/// How the loop terminates.
#[derive(Clone)]
pub enum LoopMode {
    /// Fixed iteration count.
    Count(usize),
    /// While a runtime predicate evaluates to true.
    While(FilterPredicate),
}

impl std::fmt::Debug for LoopMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoopMode::Count(n) => write!(f, "Count({n})"),
            LoopMode::While(_) => write!(f, "While(<predicate>)"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoopConfig {
    pub mode: LoopMode,
}

impl LoopConfig {
    pub fn new(mode: LoopMode) -> Self {
        Self { mode }
    }

    pub fn mode_name(&self) -> &'static str {
        match &self.mode {
            LoopMode::Count(_) => "count",
            LoopMode::While(_) => "while",
        }
    }
}

/// Maximum iterations for while-mode loops. Prevents infinite loops.
pub const MAX_LOOP_ITERATIONS: usize = 10_000;
