#[derive(Debug, Clone, PartialEq)]
pub struct HttpConfig {
    pub connect_timeout_ms: u64,
    pub response_timeout_ms: u64,
    pub max_connections: usize,
    pub max_body_size: usize,
    pub max_request_body: usize,
    pub allow_private_ips: bool,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 5_000,
            response_timeout_ms: 30_000,
            max_connections: 100,
            max_body_size: 10_485_760,
            max_request_body: 2_097_152,
            allow_private_ips: false,
        }
    }
}

impl HttpConfig {
    pub fn with_connect_timeout_ms(mut self, ms: u64) -> Self {
        self.connect_timeout_ms = ms;
        self
    }
    pub fn with_response_timeout_ms(mut self, ms: u64) -> Self {
        self.response_timeout_ms = ms;
        self
    }
    pub fn with_max_connections(mut self, n: usize) -> Self {
        self.max_connections = n;
        self
    }
    pub fn with_max_body_size(mut self, n: usize) -> Self {
        self.max_body_size = n;
        self
    }
    pub fn with_max_request_body(mut self, n: usize) -> Self {
        self.max_request_body = n;
        self
    }
    pub fn with_allow_private_ips(mut self, allow: bool) -> Self {
        self.allow_private_ips = allow;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_config_defaults() {
        let cfg = HttpConfig::default();
        assert_eq!(cfg.connect_timeout_ms, 5_000);
        assert_eq!(cfg.response_timeout_ms, 30_000);
        assert_eq!(cfg.max_connections, 100);
        assert_eq!(cfg.max_body_size, 10_485_760);
        assert_eq!(cfg.max_request_body, 2_097_152);
        assert!(!cfg.allow_private_ips);
    }

    #[test]
    fn test_http_config_builder() {
        let cfg = HttpConfig::default()
            .with_connect_timeout_ms(1_000)
            .with_allow_private_ips(true);
        assert_eq!(cfg.connect_timeout_ms, 1_000);
        assert!(cfg.allow_private_ips);
        assert_eq!(cfg.response_timeout_ms, 30_000); // unchanged
    }
}
