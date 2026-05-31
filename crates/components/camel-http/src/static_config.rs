use std::collections::HashMap;
use std::path::PathBuf;

use camel_component_api::{CamelError, UriComponents, UriConfig, parse_uri};
use serde::Deserialize;

fn default_port() -> u16 {
    8080
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_index_file() -> String {
    "index.html".to_string()
}

fn default_cache_control() -> String {
    "public, max-age=0".to_string()
}

fn default_spa_fallback() -> bool {
    false
}

/// Configuration for an HTTP static file serving endpoint.
///
/// # URI Format
///
/// ```text
/// http-static:/path/to/static/dir?port=8080&spaFallback=true&cacheControl=public,max-age=3600
/// ```
///
/// # Config Resolution
///
/// Camel.toml profile defaults → URI params override.
#[derive(Debug, Clone, Deserialize)]
pub struct HttpStaticConfig {
    /// Root directory to serve files from.
    /// Set via URI path component or Camel.toml.
    #[serde(default)]
    pub dir: PathBuf,

    /// Port to serve on (shared with `http:` consumers).
    #[serde(default = "default_port")]
    pub port: u16,

    /// Bind address.
    #[serde(default = "default_host")]
    pub host: String,

    /// Serve index.html for unmatched paths (SPA mode).
    #[serde(rename = "spaFallback", default = "default_spa_fallback")]
    pub spa_fallback: bool,

    /// Default file for directories.
    #[allow(dead_code)]
    #[serde(rename = "indexFile", default = "default_index_file")]
    pub index_file: String,

    /// Cache-Control header value.
    #[serde(rename = "cacheControl", default = "default_cache_control")]
    pub cache_control: String,

    /// Custom error pages: status code → file path.
    #[serde(rename = "errorPages", default)]
    pub error_pages: HashMap<u16, PathBuf>,
}

impl Default for HttpStaticConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::new(),
            port: default_port(),
            host: default_host(),
            spa_fallback: default_spa_fallback(),
            index_file: default_index_file(),
            cache_control: default_cache_control(),
            error_pages: HashMap::new(),
        }
    }
}

impl HttpStaticConfig {
    /// Apply TOML-provided defaults, then override with URI params.
    pub fn with_defaults(mut self, defaults: &Self) -> Self {
        if self.dir.as_os_str().is_empty() && !defaults.dir.as_os_str().is_empty() {
            self.dir = defaults.dir.clone();
        }
        if self.host == default_host() && defaults.host != default_host() {
            self.host = defaults.host.clone();
        }
        self
    }
}

impl UriConfig for HttpStaticConfig {
    fn scheme() -> &'static str {
        "http-static"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        Self::from_components(parts)
    }

    fn from_components(components: UriComponents) -> Result<Self, CamelError>
    where
        Self: Sized,
    {
        // The path component becomes the dir (keep leading / for absolute paths)
        let dir = if components.path.is_empty() || components.path == "/" {
            PathBuf::from("/")
        } else {
            PathBuf::from(&components.path)
        };

        // Deserialize from params
        let mut config: Self = serde_urlencoded::from_str(
            &components
                .params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&"),
        )
        .map_err(|e| CamelError::Config(format!("http-static: invalid URI params: {e}")))?;

        config.dir = dir;
        Ok(config)
    }

    fn validate(self) -> Result<Self, CamelError> {
        if self.dir.as_os_str().is_empty() {
            return Err(CamelError::Config(
                "http-static: dir is required".to_string(),
            ));
        }
        if !self.dir.exists() {
            return Err(CamelError::Config(format!(
                "http-static: directory does not exist: {}",
                self.dir.display()
            )));
        }
        if !self.dir.is_dir() {
            return Err(CamelError::Config(format!(
                "http-static: path is not a directory: {}",
                self.dir.display()
            )));
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_config_defaults() {
        let config = HttpStaticConfig::default();
        assert_eq!(config.port, 8080);
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.index_file, "index.html");
        assert_eq!(config.cache_control, "public, max-age=0");
        assert!(!config.spa_fallback);
        assert!(config.error_pages.is_empty());
    }

    #[test]
    fn test_static_config_from_uri() {
        let components = UriComponents {
            scheme: "http-static".to_string(),
            path: "/app/spa".to_string(),
            params: std::collections::HashMap::from([
                ("port".to_string(), "3000".to_string()),
                ("spaFallback".to_string(), "true".to_string()),
            ]),
        };
        // Note: from_components doesn't validate (dir doesn't exist), so use from_uri
        // which also validates. For this test, just check parsing without validation.
        let config = HttpStaticConfig::from_components(components).unwrap();
        assert_eq!(config.dir, PathBuf::from("/app/spa"));
        assert_eq!(config.port, 3000);
        assert!(config.spa_fallback);
    }

    #[test]
    fn test_static_config_validate_empty_dir() {
        let config = HttpStaticConfig::default();
        assert!(config.clone().validate().is_err());
    }

    #[test]
    fn test_static_config_validate_nonexistent() {
        let config = HttpStaticConfig {
            dir: PathBuf::from("/nonexistent/path_xyz123"),
            ..Default::default()
        };
        assert!(config.clone().validate().is_err());
    }

    #[test]
    fn test_static_config_validate_existing_dir() {
        let config = HttpStaticConfig {
            dir: PathBuf::from("/tmp"),
            ..Default::default()
        };
        assert!(config.clone().validate().is_ok());
    }

    #[test]
    fn test_static_config_with_defaults() {
        let defaults = HttpStaticConfig {
            dir: PathBuf::from("/default/dir"),
            host: "127.0.0.1".to_string(),
            ..Default::default()
        };
        let uri_config = HttpStaticConfig {
            port: 3000,
            ..Default::default()
        };
        let merged = uri_config.with_defaults(&defaults);
        assert_eq!(merged.dir, PathBuf::from("/default/dir"));
        assert_eq!(merged.host, "127.0.0.1");
        assert_eq!(merged.port, 3000); // URI wins
    }
}
