#[derive(Debug, Clone)]
pub struct FunctionConfig {
    pub default_timeout_ms: u64,
    pub health_interval: std::time::Duration,
    pub boot_timeout: std::time::Duration,
}

impl Default for FunctionConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 5000,
            health_interval: std::time::Duration::from_secs(5),
            boot_timeout: std::time::Duration::from_secs(10),
        }
    }
}
