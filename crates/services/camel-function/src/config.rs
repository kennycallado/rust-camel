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

impl FunctionConfig {
    pub fn validate(&self) -> Result<(), camel_api::CamelError> {
        if self.default_timeout_ms == 0 {
            return Err(camel_api::CamelError::Config(
                "default_timeout_ms must be > 0".to_string(),
            ));
        }
        if self.health_interval == std::time::Duration::ZERO {
            return Err(camel_api::CamelError::Config(
                "health_interval must be > 0".to_string(),
            ));
        }
        if self.boot_timeout == std::time::Duration::ZERO {
            return Err(camel_api::CamelError::Config(
                "boot_timeout must be > 0".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_valid_default() {
        let config = FunctionConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_zero_timeout_rejected() {
        let config = FunctionConfig {
            default_timeout_ms: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_zero_health_interval_rejected() {
        let config = FunctionConfig {
            health_interval: std::time::Duration::ZERO,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_zero_boot_timeout_rejected() {
        let config = FunctionConfig {
            boot_timeout: std::time::Duration::ZERO,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }
}
