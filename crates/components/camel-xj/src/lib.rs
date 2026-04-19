//! XML↔JSON conversion component powered by the xml-bridge.
//!
//! Endpoint URI format:
//! `xj:<stylesheet>?direction=xml2json|json2xml`
//!
//! Example:
//! ```rust,ignore
//! let xml_to_json = "xj:classpath:identity?direction=xml2json";
//! let json_to_xml = "xj:classpath:identity?direction=json2xml";
//! ```

mod component;
mod config;
mod endpoint;
mod error;
mod identity;
mod producer;

pub use component::XjComponent;
pub use config::{Direction, XjEndpointConfig};
pub use error::XjError;
pub use identity::{JSON_TO_XML_XSLT, XML_TO_JSON_XSLT};
