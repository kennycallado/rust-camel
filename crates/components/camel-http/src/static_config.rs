use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use camel_component_api::{CamelError, UriComponents, UriConfig, parse_uri};
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

/// Configuration for an `http-static:` server endpoint.
///
/// Parsed from either a Camel URI (`http-static:/path?port=8080&spaFallback=true`)
/// or a Camel.toml profile section (`[default.components.http-static]`).
///
/// # Config Resolution
///
/// Camel.toml profile defaults are applied first, then URI params override
/// individual fields. Use [`HttpStaticConfig::from_uri_with_defaults`] for
/// the merged resolution.
#[derive(Debug, Clone, Deserialize)]
pub struct HttpStaticConfig {
    /// Root directory to serve static files from.
    pub dir: PathBuf,

    /// TCP port to listen on.
    #[serde(default = "default_port")]
    pub port: u16,

    /// Bind address.
    #[serde(default = "default_host")]
    pub host: String,

    /// Serve `index.html` for unmatched paths (SPA mode).
    #[serde(rename = "spaFallback", default)]
    pub spa_fallback: bool,

    /// Cache-Control header value applied to all static responses.
    #[serde(rename = "cacheControl", default = "default_cache_control")]
    pub cache_control: String,

    /// Custom error pages mapped by HTTP status code.
    #[serde(
        rename = "errorPages",
        default,
        deserialize_with = "deserialize_error_pages"
    )]
    pub error_pages: HashMap<u16, PathBuf>,

    /// URL path prefix this mount serves on (e.g. "/assets").
    /// Derived from the URI path in `http-static:/assets?dir=/var/www`.
    #[serde(default = "default_mount_path")]
    pub mount_path: String,
}

fn default_port() -> u16 {
    8080
}

/// Custom deserializer for `errorPages` that accepts both string and integer
/// keys. TOML bare keys are always strings (e.g. `404 = "404.html"`), so we
/// parse string keys to u16. Integer keys are accepted for robustness (e.g.
/// from other formats or inline tables).
fn deserialize_error_pages<'de, D>(deserializer: D) -> Result<HashMap<u16, PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ErrorPagesVisitor;

    impl<'de> Visitor<'de> for ErrorPagesVisitor {
        type Value = HashMap<u16, PathBuf>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map with integer or string keys representing HTTP status codes")
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut map = HashMap::new();
            while let Some(key) = access.next_key::<String>()? {
                let code: u16 = key.parse().map_err(de::Error::custom)?;
                let path: PathBuf = access.next_value()?;
                map.insert(code, path);
            }
            Ok(map)
        }
    }

    deserializer.deserialize_map(ErrorPagesVisitor)
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_cache_control() -> String {
    "public, max-age=0".to_string()
}

fn default_mount_path() -> String {
    "/".to_string()
}

impl Default for HttpStaticConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::new(),
            port: default_port(),
            host: default_host(),
            spa_fallback: false,
            cache_control: default_cache_control(),
            error_pages: HashMap::new(),
            mount_path: default_mount_path(),
        }
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

    fn from_components(parts: UriComponents) -> Result<Self, CamelError> {
        if parts.scheme != "http-static" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'http-static', got '{}'",
                parts.scheme
            )));
        }

        // The URI path becomes the mount_path (URL prefix).
        // If a `dir` query param is present, it overrides the directory.
        // Otherwise, the URI path itself is the directory (backward compat).
        let (dir, mount_path) = if parts.path.is_empty() {
            return Err(CamelError::InvalidUri(
                "http-static URI requires a path (e.g. http-static:/path or http-static:/prefix?dir=/var/www)"
                    .to_string(),
            ));
        } else if let Some(dir_param) = parts.params.get("dir") {
            // New style: URI path = mount_path, dir param = directory
            let mount_path = normalize_mount_path(&parts.path);
            (PathBuf::from(dir_param), mount_path)
        } else if parts.path == "/" {
            // Root mount path with no dir param: the URI path "/" is the mount
            // point, not the directory. Require explicit dir when serving from root.
            return Err(CamelError::InvalidUri(
                "http-static:/ requires a dir query parameter when mount_path is root \
                 (e.g. http-static:/?dir=/var/www)"
                    .to_string(),
            ));
        } else {
            // Old style (backward compat): URI path = directory, mount_path = "/"
            (PathBuf::from(&parts.path), default_mount_path())
        };

        let port = parts
            .params
            .get("port")
            .map(|v| {
                v.parse::<u16>()
                    .map_err(|e| CamelError::InvalidUri(format!("invalid value for port: {e}")))
            })
            .transpose()?
            .unwrap_or_else(default_port);

        let host = parts
            .params
            .get("host")
            .cloned()
            .unwrap_or_else(default_host);

        let spa_fallback = parts
            .params
            .get("spaFallback")
            .map(|v| parse_bool_param_static(v))
            .transpose()?
            .unwrap_or(false);

        let cache_control = parts
            .params
            .get("cacheControl")
            .cloned()
            .unwrap_or_else(default_cache_control);

        // errorPages is not supported as a URI param (it's a map);
        // it comes exclusively from Camel.toml.
        let error_pages = HashMap::new();

        Ok(Self {
            dir,
            port,
            host,
            spa_fallback,
            cache_control,
            error_pages,
            mount_path,
        })
    }
}

/// Normalize a mount path: ensure leading `/`, no trailing `/` (except root).
fn normalize_mount_path(path: &str) -> String {
    let mut path = path.to_string();
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    if path.len() > 1 {
        path = path.trim_end_matches('/').to_string();
    }
    if path.is_empty() {
        "/".to_string()
    } else {
        path
    }
}

impl HttpStaticConfig {
    /// Resolve config from a URI, with TOML-provided defaults applied first.
    ///
    /// Fields present in the URI params override the corresponding values from
    /// `toml_defaults`. Fields absent from the URI retain their TOML value.
    ///
    /// # Config Resolution Order
    ///
    /// 1. Start with `toml_defaults` (from Camel.toml profile)
    /// 2. Override individual fields where URI params are present
    /// 3. Apply hardcoded defaults for any remaining unset fields
    pub fn from_uri_with_defaults(uri: &str, toml_defaults: &Self) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;

        // Validate scheme to avoid accidentally accepting wrong URIs here.
        if parts.scheme != "http-static" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'http-static', got '{}'",
                parts.scheme
            )));
        }

        // Start from TOML defaults
        let mut config = toml_defaults.clone();

        // URI path and dir param handling
        if !parts.path.is_empty() {
            if let Some(dir_param) = parts.params.get("dir") {
                // New style: URI path = mount_path, dir param = directory
                config.dir = PathBuf::from(dir_param);
                config.mount_path = normalize_mount_path(&parts.path);
            } else if parts.path == "/" {
                // Root mount path with no dir param: preserve TOML dir,
                // only set mount_path (which already defaults to "/").
                // This handles routes like `http-static:/` where the TOML
                // config provides the directory (e.g. `dir = "public"`).
            } else {
                // Old style (backward compat): URI path = directory
                config.dir = PathBuf::from(&parts.path);
                config.mount_path = default_mount_path();
            }
        }

        // URI params override individual fields
        if let Some(v) = parts.params.get("port") {
            config.port = v
                .parse::<u16>()
                .map_err(|e| CamelError::InvalidUri(format!("invalid value for port: {e}")))?;
        }

        if let Some(v) = parts.params.get("host") {
            config.host = v.clone();
        }

        if let Some(v) = parts.params.get("spaFallback") {
            config.spa_fallback = parse_bool_param_static(v)?;
        }

        if let Some(v) = parts.params.get("cacheControl") {
            config.cache_control = v.clone();
        }

        // If URI had no path and TOML had no dir, apply hardcoded default
        if config.dir.as_os_str().is_empty() {
            return Err(CamelError::InvalidUri(
                "http-static requires a directory path (from URI or Camel.toml)".to_string(),
            ));
        }

        Ok(config)
    }
}

/// Parse a boolean parameter, case-insensitively.
///
/// Accepts: "true"/"True"/"TRUE"/"1"/"yes" as true,
///          "false"/"False"/"FALSE"/"0"/"no" as false.
fn parse_bool_param_static(value: &str) -> Result<bool, CamelError> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(CamelError::InvalidUri(format!(
            "invalid boolean value: '{value}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // URI parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_uri_full() {
        let config =
            HttpStaticConfig::from_uri("http-static:/app/spa?port=3000&spaFallback=true").unwrap();
        assert_eq!(config.dir, PathBuf::from("/app/spa"));
        assert_eq!(config.port, 3000);
        assert_eq!(config.host, "0.0.0.0");
        assert!(config.spa_fallback);
        assert_eq!(config.cache_control, "public, max-age=0");
        assert!(config.error_pages.is_empty());
        assert_eq!(config.mount_path, "/");
    }

    #[test]
    fn test_parse_uri_defaults_when_params_omitted() {
        let config = HttpStaticConfig::from_uri("http-static:/var/www").unwrap();
        assert_eq!(config.dir, PathBuf::from("/var/www"));
        assert_eq!(config.port, 8080);
        assert_eq!(config.host, "0.0.0.0");
        assert!(!config.spa_fallback);
        assert_eq!(config.cache_control, "public, max-age=0");
        assert_eq!(config.mount_path, "/");
    }

    #[test]
    fn test_parse_uri_all_params() {
        let config = HttpStaticConfig::from_uri(
            "http-static:/app/dist?port=9090&host=127.0.0.1&spaFallback=true&cacheControl=no-cache",
        )
        .unwrap();
        assert_eq!(config.dir, PathBuf::from("/app/dist"));
        assert_eq!(config.port, 9090);
        assert_eq!(config.host, "127.0.0.1");
        assert!(config.spa_fallback);
        assert_eq!(config.cache_control, "no-cache");
        assert_eq!(config.mount_path, "/");
    }

    #[test]
    fn test_parse_uri_rejects_wrong_scheme() {
        let result = HttpStaticConfig::from_uri("http:/app/spa");
        assert!(result.is_err());
        if let Err(CamelError::InvalidUri(msg)) = result {
            assert!(msg.contains("expected scheme 'http-static'"));
            assert!(msg.contains("got 'http'"));
        } else {
            panic!("Expected InvalidUri error");
        }
    }

    #[test]
    fn test_parse_uri_rejects_empty_path() {
        let result = HttpStaticConfig::from_uri("http-static:");
        assert!(result.is_err());
        if let Err(CamelError::InvalidUri(msg)) = result {
            assert!(msg.contains("requires a path"));
        } else {
            panic!("Expected InvalidUri error for empty path");
        }
    }

    #[test]
    fn test_parse_uri_invalid_port() {
        let result = HttpStaticConfig::from_uri("http-static:/app?port=notanumber");
        assert!(result.is_err());
        if let Err(CamelError::InvalidUri(msg)) = result {
            assert!(msg.contains("invalid value for port"));
        } else {
            panic!("Expected InvalidUri error for invalid port");
        }
    }

    #[test]
    fn test_parse_uri_boolean_variants() {
        for val in &["true", "True", "TRUE", "1", "yes"] {
            let uri = format!("http-static:/app?spaFallback={val}");
            let config = HttpStaticConfig::from_uri(&uri).unwrap();
            assert!(
                config.spa_fallback,
                "spaFallback='{val}' should parse to true"
            );
        }
        for val in &["false", "False", "FALSE", "0", "no"] {
            let uri = format!("http-static:/app?spaFallback={val}");
            let config = HttpStaticConfig::from_uri(&uri).unwrap();
            assert!(
                !config.spa_fallback,
                "spaFallback='{val}' should parse to false"
            );
        }
    }

    #[test]
    fn test_parse_uri_invalid_boolean() {
        let result = HttpStaticConfig::from_uri("http-static:/app?spaFallback=maybe");
        assert!(result.is_err());
        if let Err(CamelError::InvalidUri(msg)) = result {
            assert!(msg.contains("invalid boolean value"));
        } else {
            panic!("Expected InvalidUri error for invalid boolean");
        }
    }

    // -----------------------------------------------------------------------
    // TOML / serde parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_toml_parsing_with_renamed_keys() {
        let toml_str = r#"
            dir = "/app/spa"
            port = 3000
            host = "127.0.0.1"
            spaFallback = true
            cacheControl = "no-cache"
        "#;
        let config: HttpStaticConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.dir, PathBuf::from("/app/spa"));
        assert_eq!(config.port, 3000);
        assert_eq!(config.host, "127.0.0.1");
        assert!(config.spa_fallback);
        assert_eq!(config.cache_control, "no-cache");
    }

    #[test]
    fn test_toml_error_pages_parsing() {
        let toml_str = r#"
            dir = "/app/spa"
            [errorPages]
            404 = "/app/errors/404.html"
            500 = "/app/errors/500.html"
        "#;
        let config: HttpStaticConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.dir, PathBuf::from("/app/spa"));
        assert_eq!(config.error_pages.len(), 2);
        assert_eq!(
            config.error_pages.get(&404),
            Some(&PathBuf::from("/app/errors/404.html"))
        );
        assert_eq!(
            config.error_pages.get(&500),
            Some(&PathBuf::from("/app/errors/500.html"))
        );
    }

    #[test]
    fn test_toml_defaults() {
        let toml_str = r#"
            dir = "/app/spa"
        "#;
        let config: HttpStaticConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.port, 8080);
        assert_eq!(config.host, "0.0.0.0");
        assert!(!config.spa_fallback);
        assert_eq!(config.cache_control, "public, max-age=0");
        assert!(config.error_pages.is_empty());
    }

    // -----------------------------------------------------------------------
    // from_uri_with_defaults tests (TOML → URI override)
    // -----------------------------------------------------------------------

    #[test]
    fn test_uri_overrides_toml_defaults() {
        let toml_defaults = HttpStaticConfig {
            dir: PathBuf::from("/default/dir"),
            port: 8080,
            host: "0.0.0.0".to_string(),
            spa_fallback: false,
            cache_control: "public, max-age=0".to_string(),
            error_pages: HashMap::new(),
            ..HttpStaticConfig::default()
        };

        let config = HttpStaticConfig::from_uri_with_defaults(
            "http-static:/override/dir?port=3000&spaFallback=true",
            &toml_defaults,
        )
        .unwrap();

        // URI overrides
        assert_eq!(config.dir, PathBuf::from("/override/dir"));
        assert_eq!(config.port, 3000);
        assert!(config.spa_fallback);

        // TOML defaults retained for unspecified fields
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.cache_control, "public, max-age=0");
        assert_eq!(config.mount_path, "/");
    }

    #[test]
    fn test_uri_preserves_toml_when_params_absent() {
        let toml_defaults = HttpStaticConfig {
            dir: PathBuf::from("/toml/dir"),
            port: 9090,
            host: "127.0.0.1".to_string(),
            spa_fallback: true,
            cache_control: "no-cache".to_string(),
            error_pages: HashMap::new(),
            ..HttpStaticConfig::default()
        };

        // URI only specifies the path (dir)
        let config =
            HttpStaticConfig::from_uri_with_defaults("http-static:/uri/dir", &toml_defaults)
                .unwrap();

        // dir comes from URI
        assert_eq!(config.dir, PathBuf::from("/uri/dir"));
        // Everything else from TOML
        assert_eq!(config.port, 9090);
        assert_eq!(config.host, "127.0.0.1");
        assert!(config.spa_fallback);
        assert_eq!(config.cache_control, "no-cache");
        assert_eq!(config.mount_path, "/");
    }

    #[test]
    fn test_uri_with_defaults_rejects_empty_dir() {
        let toml_defaults = HttpStaticConfig {
            dir: PathBuf::new(), // empty dir in TOML
            ..HttpStaticConfig::default()
        };

        let result = HttpStaticConfig::from_uri_with_defaults("http-static:", &toml_defaults);
        assert!(result.is_err());
        if let Err(CamelError::InvalidUri(msg)) = result {
            assert!(msg.contains("directory path"));
        } else {
            panic!("Expected InvalidUri error");
        }
    }

    #[test]
    fn test_uri_with_defaults_rejects_wrong_scheme() {
        let toml_defaults = HttpStaticConfig {
            dir: PathBuf::from("/default"),
            ..HttpStaticConfig::default()
        };

        let result = HttpStaticConfig::from_uri_with_defaults("http:/app", &toml_defaults);
        assert!(result.is_err());
        if let Err(CamelError::InvalidUri(msg)) = result {
            assert!(msg.contains("expected scheme 'http-static'"));
            assert!(msg.contains("got 'http'"));
        } else {
            panic!("Expected InvalidUri error for wrong scheme in from_uri_with_defaults");
        }
    }

    // -----------------------------------------------------------------------
    // mount_path tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_mount_path_from_uri_with_dir_param() {
        // New style: URI path = mount_path, dir param = directory
        let config = HttpStaticConfig::from_uri("http-static:/assets?dir=/var/www").unwrap();
        assert_eq!(config.dir, PathBuf::from("/var/www"));
        assert_eq!(config.mount_path, "/assets");
    }

    #[test]
    fn test_mount_path_root_when_no_dir_param() {
        // Old style: URI path = directory, mount_path = "/"
        let config = HttpStaticConfig::from_uri("http-static:/var/www").unwrap();
        assert_eq!(config.dir, PathBuf::from("/var/www"));
        assert_eq!(config.mount_path, "/");
    }

    #[test]
    fn test_mount_path_normalized_leading_slash() {
        let config = HttpStaticConfig::from_uri("http-static:assets?dir=/var/www").unwrap();
        assert_eq!(config.mount_path, "/assets");
    }

    #[test]
    fn test_mount_path_normalized_no_trailing_slash() {
        let config = HttpStaticConfig::from_uri("http-static:/assets/?dir=/var/www").unwrap();
        assert_eq!(config.mount_path, "/assets");
    }

    #[test]
    fn test_mount_path_root_stays_root() {
        let config = HttpStaticConfig::from_uri("http-static:/?dir=/var/www").unwrap();
        assert_eq!(config.mount_path, "/");
    }

    #[test]
    fn test_mount_path_nested() {
        let config = HttpStaticConfig::from_uri("http-static:/assets/sub?dir=/var/www").unwrap();
        assert_eq!(config.mount_path, "/assets/sub");
    }

    #[test]
    fn test_from_uri_with_defaults_mount_path_with_dir_param() {
        let toml_defaults = HttpStaticConfig {
            dir: PathBuf::from("/default/dir"),
            ..HttpStaticConfig::default()
        };

        let config = HttpStaticConfig::from_uri_with_defaults(
            "http-static:/assets?dir=/var/www&port=3000",
            &toml_defaults,
        )
        .unwrap();

        assert_eq!(config.dir, PathBuf::from("/var/www"));
        assert_eq!(config.mount_path, "/assets");
        assert_eq!(config.port, 3000);
    }

    // -----------------------------------------------------------------------
    // PathBuf parsing validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_dir_pathbuf_accepts_various_paths() {
        // Relative path
        let config = HttpStaticConfig::from_uri("http-static:./frontend/dist").unwrap();
        assert_eq!(config.dir, PathBuf::from("./frontend/dist"));
        assert_eq!(config.mount_path, "/");

        // Absolute path
        let config = HttpStaticConfig::from_uri("http-static:/var/www/html").unwrap();
        assert_eq!(config.dir, PathBuf::from("/var/www/html"));
        assert_eq!(config.mount_path, "/");

        // Path with spaces (percent-encoded)
        let config = HttpStaticConfig::from_uri("http-static:/app/my%20files").unwrap();
        assert_eq!(config.dir, PathBuf::from("/app/my files"));
        assert_eq!(config.mount_path, "/");
    }

    #[test]
    fn test_dir_nonexistent_is_detectable() {
        // Config parsing does NOT validate directory existence — that happens
        // at consumer start() via std::fs::canonicalize(). This test confirms
        // that a non-existent path parses successfully at the config layer.
        let config =
            HttpStaticConfig::from_uri("http-static:/nonexistent/path/that/does/not/exist")
                .unwrap();
        assert_eq!(
            config.dir,
            PathBuf::from("/nonexistent/path/that/does/not/exist")
        );
        // Validation happens at consumer start, not config parse time.
    }
}
