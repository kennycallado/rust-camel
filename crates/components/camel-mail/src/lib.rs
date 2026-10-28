//! SMTP and IMAP mail component for rust-camel.
//!
//! Schemes `smtp`, `smtps`, `imap` and `imaps` take `scheme://host[:port]`
//! URIs. Logins live in `[components.mail.accounts.<name>]` tables in
//! `Camel.toml`, never in the URI; secrets are wrapped in a redacting type
//! so they never reach `Debug` output or logs (ADR-0051, ADR-0076).
//!
//! The public surface is the bundle, the per-scheme component, the
//! configuration types and the exchange header names. Third-party mail
//! types stay private (ADR-0020, ADR-0060).
pub(crate) mod adapter;
pub(crate) mod address;
pub(crate) mod bundle;
pub(crate) mod component;
pub(crate) mod config;
pub(crate) mod consumer;
pub(crate) mod error;
pub(crate) mod fence;
pub mod headers;
pub(crate) mod health;
pub(crate) mod message;
pub(crate) mod metadata;
pub(crate) mod poll;
pub(crate) mod polling;
pub(crate) mod producer;
pub(crate) mod store;
#[cfg(feature = "test-support")]
pub mod testing;
pub(crate) mod tls;
pub(crate) mod transport;

pub use bundle::MailBundle;
pub use component::MailComponent;
pub use config::{MailAccountConfig, MailComponentConfig, MailScheme, SecretString};
pub use error::MailError;
