use std::time::Duration;

#[derive(Debug, Clone)]
pub struct DelayConfig {
    pub delay_ms: u64,
    pub dynamic_header: Option<String>,
}

impl DelayConfig {
    pub fn new(delay_ms: u64) -> Self {
        Self {
            delay_ms,
            dynamic_header: None,
        }
    }

    pub fn with_dynamic_header(mut self, header: impl Into<String>) -> Self {
        self.dynamic_header = Some(header.into());
        self
    }

    pub fn from_duration(duration: Duration) -> Self {
        Self {
            delay_ms: duration.as_millis() as u64,
            dynamic_header: None,
        }
    }

    pub fn from_duration_with_header(duration: Duration, header: impl Into<String>) -> Self {
        Self {
            delay_ms: duration.as_millis() as u64,
            dynamic_header: Some(header.into()),
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
}
