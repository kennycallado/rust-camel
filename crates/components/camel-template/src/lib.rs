//! External template component (ADR-0047 Stage 2).
//!
//! Phase 1 (this task) defines the public types — `TemplateReloadError` and
//! `ExternalTemplateLimitsConfig` — without the Component/Endpoint/lifecycle
//! implementation (those land in Phase 4).

pub mod bundle;
mod closure;
pub(crate) mod component;
pub(crate) mod endpoint;
pub(crate) mod lifecycle;
pub(crate) mod path_util;
pub(crate) mod producer;
pub(crate) mod reload;
pub(crate) mod template_set;
pub(crate) mod uri;

pub mod config;
pub mod error;

pub use bundle::{TemplateBundle, TemplateBundleConfig};
pub use component::TemplateComponent;
pub use config::{ExternalTemplateLimitsConfig, ResolvedExternalTemplateLimits};
pub use error::TemplateReloadError;
