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

#[cfg(test)]
mod tests {
    use super::*;

    fn always_true() -> FilterPredicate {
        Arc::new(|_| true)
    }

    #[test]
    fn loop_config_new_count() {
        let cfg = LoopConfig::new(LoopMode::Count(5));
        assert_eq!(cfg.mode_name(), "count");
    }

    #[test]
    fn loop_config_new_while() {
        let cfg = LoopConfig::new(LoopMode::While(always_true()));
        assert_eq!(cfg.mode_name(), "while");
    }

    #[test]
    fn loop_mode_debug_count() {
        let mode = LoopMode::Count(3);
        assert_eq!(format!("{mode:?}"), "Count(3)");
    }

    #[test]
    fn loop_mode_debug_while() {
        let mode = LoopMode::While(always_true());
        assert_eq!(format!("{mode:?}"), "While(<predicate>)");
    }

    #[test]
    fn loop_mode_clone_count() {
        let mode = LoopMode::Count(10);
        let cloned = mode.clone();
        assert_eq!(format!("{cloned:?}"), "Count(10)");
    }

    #[test]
    fn loop_config_clone() {
        let cfg = LoopConfig::new(LoopMode::Count(2));
        let cloned = cfg.clone();
        assert_eq!(cloned.mode_name(), "count");
    }

    #[test]
    fn max_loop_iterations_value() {
        assert_eq!(MAX_LOOP_ITERATIONS, 10_000);
    }
}
