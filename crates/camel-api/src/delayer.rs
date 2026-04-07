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
