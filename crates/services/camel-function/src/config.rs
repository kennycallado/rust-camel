#[derive(Debug, Clone)]
pub struct FunctionConfig {
    pub default_timeout_ms: u64,
    pub health_interval: std::time::Duration,
    pub boot_timeout: std::time::Duration,
    /// Outbound network (egress) allowlist for the Deno runner, as
    /// `host[:port]` entries. Empty (the default) denies all outbound
    /// connections: the runner may only bind its HTTP server, which is the
    /// pre-allowlist behavior.
    ///
    /// Semantics follow Deno `--allow-net` exact-host matching:
    /// - `host` allows any port on that host
    /// - `host:port` allows only that host and port
    /// - IPv6 literals must be bracketed: `[::1]` or `[::1]:443`
    pub egress_allowlist: Vec<String>,
}

impl Default for FunctionConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 5000,
            health_interval: std::time::Duration::from_secs(5),
            boot_timeout: std::time::Duration::from_secs(10),
            egress_allowlist: Vec::new(),
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
        for entry in &self.egress_allowlist {
            validate_egress_allowlist_entry(entry)?;
        }
        Ok(())
    }
}

/// Validate one `egress_allowlist` entry (`host[:port]`).
///
/// Fail-closed: any malformed entry is rejected, so a bad config can never
/// widen the runner's network permissions. The charset whitelist (no
/// commas, whitespace, schemes, wildcards, or paths) also prevents
/// injection into the comma-joined Deno `--allow-net` value.
pub fn validate_egress_allowlist_entry(entry: &str) -> Result<(), camel_api::CamelError> {
    let err = |reason: &str| {
        camel_api::CamelError::Config(format!("egress_allowlist entry '{entry}': {reason}"))
    };
    if entry.is_empty() {
        return Err(err("must not be empty"));
    }
    if entry.chars().any(char::is_whitespace) {
        return Err(err("must not contain whitespace"));
    }

    let (host, port) = if let Some(rest) = entry.strip_prefix('[') {
        // Bracketed IPv6 literal: `[addr]` or `[addr]:port`.
        let Some(close) = rest.find(']') else {
            return Err(err("unterminated IPv6 bracket"));
        };
        let inner = &rest[..close];
        let tail = &rest[close + 1..];
        if inner.is_empty() {
            return Err(err("empty IPv6 address"));
        }
        if !inner.chars().all(|c| c.is_ascii_hexdigit() || c == ':') {
            return Err(err("IPv6 literal may contain only hex digits and colons"));
        }
        let port = match tail {
            "" => None,
            t => {
                let Some(p) = t.strip_prefix(':') else {
                    return Err(err("unexpected characters after IPv6 bracket"));
                };
                Some(p)
            }
        };
        (None, port)
    } else {
        match entry.rsplit_once(':') {
            Some((h, p)) => {
                if h.is_empty() {
                    return Err(err("empty host"));
                }
                if p.is_empty() {
                    return Err(err("empty port"));
                }
                (Some(h), Some(p))
            }
            None => (Some(entry), None),
        }
    };

    if let Some(h) = host {
        if h.is_empty() {
            return Err(err("empty host"));
        }
        if !h
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        {
            return Err(err(
                "host may contain only letters, digits, dots, and hyphens \
                 (bracket IPv6 literals for IPv6 addresses)",
            ));
        }
        if h.starts_with('.') || h.ends_with('.') || h.contains("..") {
            return Err(err("malformed host"));
        }
    }
    if let Some(p) = port {
        // Canonical digits only: `+443` parses as u16 but is not a form any
        // resolver grants — reject so the validator charset stays the exact
        // grant set.
        if !p.chars().all(|c| c.is_ascii_digit()) {
            return Err(err("port must be an integer in the range 1..=65535"));
        }
        match p.parse::<u16>() {
            Ok(n) if n != 0 => {}
            _ => return Err(err("port must be an integer in the range 1..=65535")),
        }
    }
    Ok(())
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

    #[test]
    fn test_config_empty_allowlist_is_default_deny() {
        // Default (absent) allowlist must stay valid: deny-all egress is
        // the pinned pre-allowlist behavior.
        let config = FunctionConfig::default();
        assert!(config.egress_allowlist.is_empty());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_egress_entry_valid_forms() {
        for entry in [
            "api.example.com",
            "api.example.com:443",
            "internal",
            "sub.domain-2.io:8080",
            "10.0.0.5",
            "10.0.0.5:5432",
            "[::1]",
            "[::1]:443",
            "[2001:db8::1]:8443",
        ] {
            validate_egress_allowlist_entry(entry)
                .unwrap_or_else(|e| panic!("'{entry}' should be valid: {e}"));
        }
    }

    #[test]
    fn test_egress_entry_malformed_rejected() {
        for entry in [
            "",
            "   ",
            "host:",
            ":443",
            "bad host",
            "http://api.example.com",
            "api.example.com/path",
            "user@api.example.com",
            "api.example.com,evil.com",
            "*",
            "*.example.com",
            "host:0",
            "host:65536",
            "host:abc",
            "host:+443",
            "[::1",
            "[zz::1]",
            "[]",
            "[::1]junk",
            "..malformed..host",
        ] {
            let err = validate_egress_allowlist_entry(entry)
                .err()
                .unwrap_or_else(|| panic!("'{entry}' should be rejected"));
            assert!(
                err.to_string().contains("egress_allowlist"),
                "error for '{entry}' must name egress_allowlist, got: {err}"
            );
        }
    }

    #[test]
    fn test_config_validate_rejects_malformed_allowlist_entry() {
        let config = FunctionConfig {
            egress_allowlist: vec!["api.example.com:443".to_string(), "bad host".to_string()],
            ..Default::default()
        };
        let err = config.validate().err().expect("must be rejected");
        assert!(err.to_string().contains("egress_allowlist"));
    }
}
