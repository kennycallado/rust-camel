//! Kubernetes platform integration for Apache Camel Rust.
//!
//! Provides Kubernetes-native implementations of the platform SPI defined in `camel-api`:
//! - [`KubernetesPlatformIdentity`] — auto-detects pod identity from downward API env vars
//! - [`KubernetesLeadershipService`] — Lease-based per-lock leadership via `kube`
//! - [`KubernetesReadinessGate`] — patches pod readiness conditions via Kubernetes API
//! - [`KubernetesPlatformService`] — bundles identity/readiness/leadership ports

pub mod identity;
pub mod platform_service;
pub mod readiness_gate;

pub use camel_api::platform::PlatformError;
pub use identity::KubernetesPlatformIdentity;
pub use platform_service::{
    KubernetesLeadershipService, KubernetesPlatformConfig, KubernetesPlatformService,
};
pub use readiness_gate::KubernetesReadinessGate;
