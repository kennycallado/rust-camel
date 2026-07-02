use std::time::Duration;

/// Default cap for header-derived delays (1 hour). A header value larger than
/// this is clamped down to it before the cast to `u64`. The H12 vulnerability
/// was: a header `CamelDelayMs: 1e20` cast to `u64` saturates to `u64::MAX`,
/// then `Duration::from_millis(u64::MAX)` + `Instant` overflows and panics.
pub const DEFAULT_MAX_DELAY_MS: u64 = 3_600_000;

#[derive(Debug, Clone)]
pub struct DelayConfig {
    pub delay_ms: u64,
    pub dynamic_header: Option<String>,
    /// Hard cap for header-derived delay values. A header value larger than
    /// this is clamped to `max_delay_ms` before the cast. Defaults to
    /// `DEFAULT_MAX_DELAY_MS` (1 hour). The processor MUST consult this cap
    /// before constructing the `Duration` it sleeps on.
    pub max_delay_ms: u64,
}

impl DelayConfig {
    pub fn new(delay_ms: u64) -> Self {
        Self {
            delay_ms,
            dynamic_header: None,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
        }
    }

    pub fn with_dynamic_header(mut self, header: impl Into<String>) -> Self {
        self.dynamic_header = Some(header.into());
        self
    }

    /// Override the cap on header-derived delays. Pass a value larger than
    /// `DEFAULT_MAX_DELAY_MS` only when the operator has a real reason to
    /// sleep longer; the value is a hard ceiling, not a soft target.
    pub fn with_max_delay_ms(mut self, max_ms: u64) -> Self {
        self.max_delay_ms = max_ms;
        self
    }

    pub fn from_duration(duration: Duration) -> Self {
        Self {
            delay_ms: duration.as_millis() as u64,
            dynamic_header: None,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
        }
    }

    pub fn from_duration_with_header(duration: Duration, header: impl Into<String>) -> Self {
        Self {
            delay_ms: duration.as_millis() as u64,
            dynamic_header: Some(header.into()),
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_delay_no_header() {
        let cfg = DelayConfig::new(500);
        assert_eq!(cfg.delay_ms, 500);
        assert!(cfg.dynamic_header.is_none());
    }

    #[test]
    fn with_dynamic_header_sets_header() {
        let cfg = DelayConfig::new(200).with_dynamic_header("X-Delay");
        assert_eq!(cfg.delay_ms, 200);
        assert_eq!(cfg.dynamic_header.as_deref(), Some("X-Delay"));
    }

    #[test]
    fn from_duration_converts_ms() {
        let cfg = DelayConfig::from_duration(Duration::from_millis(1500));
        assert_eq!(cfg.delay_ms, 1500);
        assert!(cfg.dynamic_header.is_none());
    }

    #[test]
    fn from_duration_with_header() {
        let cfg = DelayConfig::from_duration_with_header(Duration::from_secs(2), "MyHeader");
        assert_eq!(cfg.delay_ms, 2000);
        assert_eq!(cfg.dynamic_header.as_deref(), Some("MyHeader"));
    }

    #[test]
    fn clone_preserves_values() {
        let cfg = DelayConfig::new(100).with_dynamic_header("H");
        let cloned = cfg.clone();
        assert_eq!(cloned.delay_ms, 100);
        assert_eq!(cloned.dynamic_header.as_deref(), Some("H"));
    }

    /// H12: `DelayConfig` carries a `max_delay_ms` field with a sensible
    /// default. The default is 3_600_000 (1 hour) so a typo or a malicious
    /// header value cannot pin a task to `u64::MAX` ms.
    #[test]
    fn test_delay_config_max_delay_ms_default() {
        let cfg = DelayConfig::new(100);
        assert_eq!(cfg.max_delay_ms, 3_600_000);
    }

    /// `with_max_delay_ms` overrides the default and is preserved by `clone`.
    #[test]
    fn test_delay_config_with_max_delay_ms() {
        let cfg = DelayConfig::new(100).with_max_delay_ms(50);
        assert_eq!(cfg.max_delay_ms, 50);
        let cloned = cfg.clone();
        assert_eq!(cloned.max_delay_ms, 50);
    }
}
