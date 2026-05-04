/// Static descriptor for a bridge binary (JMS, XML, or future bridges).
/// All fields are `&'static str` so constants can be declared at compile time.
#[derive(Debug, Clone, Copy)]
pub struct BridgeSpec {
    /// Human-readable name used in log messages and error strings.
    pub name: &'static str,
    /// Environment variable that overrides the binary path (e.g. `CAMEL_JMS_BRIDGE_BINARY_PATH`).
    pub env_binary_path: &'static str,
    /// Environment variable that overrides the GitHub Releases base URL.
    pub env_release_url: &'static str,
    /// Sub-directory under the shared rust-camel cache root (e.g. `"jms-bridge"`).
    pub cache_subdir: &'static str,
    /// Prefix used in GitHub release tag names (e.g. `"jms-bridge-v"`).
    pub release_tag_prefix: &'static str,
    /// Default version string used when no version is specified by the caller.
    pub default_version: &'static str,
    /// Template for the stderr log file name; `{pid}` is replaced at runtime.
    /// Example: `"jms-bridge-{pid}.log"`
    pub log_file_template: &'static str,
    /// Whether pre-built binaries are available for macOS.
    /// JMS bridge: false (requires Docker build). XML bridge: true (GraalVM native-image).
    pub macos_supported: bool,
}

/// Spec for the JMS bridge (`bridges/jms`).
pub const JMS_BRIDGE: BridgeSpec = BridgeSpec {
    name: "jms-bridge",
    env_binary_path: "CAMEL_JMS_BRIDGE_BINARY_PATH",
    env_release_url: "CAMEL_JMS_BRIDGE_RELEASE_URL",
    cache_subdir: "jms-bridge",
    release_tag_prefix: "jms-bridge-v",
    default_version: env!("CARGO_PKG_VERSION"),
    log_file_template: "jms-bridge-{pid}.log",
    macos_supported: false,
};

/// Spec for the XML bridge (`bridges/xml`).
pub const XML_BRIDGE: BridgeSpec = BridgeSpec {
    name: "xml-bridge",
    env_binary_path: "CAMEL_XML_BRIDGE_BINARY_PATH",
    env_release_url: "CAMEL_XML_BRIDGE_RELEASE_URL",
    cache_subdir: "xml-bridge",
    release_tag_prefix: "xml-bridge-v",
    default_version: env!("CARGO_PKG_VERSION"),
    log_file_template: "xml-bridge-{pid}.log",
    macos_supported: true,
};

/// Spec for the CXF bridge (`bridges/cxf`).
pub const CXF_BRIDGE: BridgeSpec = BridgeSpec {
    name: "cxf-bridge",
    env_binary_path: "CAMEL_CXF_BRIDGE_BINARY_PATH",
    env_release_url: "CAMEL_CXF_BRIDGE_RELEASE_URL",
    cache_subdir: "cxf-bridge",
    release_tag_prefix: "cxf-bridge-v",
    default_version: env!("CARGO_PKG_VERSION"),
    log_file_template: "cxf-bridge-{pid}.log",
    macos_supported: true,
};
