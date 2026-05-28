//! XML↔JSON conversion component powered by the xml-bridge.
//!
//! Endpoint URI format:
//! `xj:<stylesheet>?direction=xml2json|json2xml[&transformDirection=XML2JSON|JSON2XML][&resourceUri=<uri>]`
//!
//! Example:
//! ```rust,ignore
//! let xml_to_json = "xj:classpath:identity?direction=xml2json";
//! let json_to_xml = "xj:classpath:identity?direction=json2xml";
//! let with_options = "xj:file:///tmp/transform.xslt?direction=xml2json&transformDirection=XML2JSON&resourceUri=classpath:extra.xslt";
//! ```

mod component;
mod config;
mod endpoint;
mod error;
pub mod health;
mod identity;
mod producer;

pub use component::{XjBridgeRuntime, XjComponent, XjComponentConfig};
pub use config::{Direction, XjEndpointConfig};
pub use error::XjError;
pub use health::XjHealthCheck;
pub use identity::{JSON_TO_XML_XSLT, XML_TO_JSON_XSLT};
