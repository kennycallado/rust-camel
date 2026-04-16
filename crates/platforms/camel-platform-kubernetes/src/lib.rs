//! Kubernetes platform integration for Apache Camel Rust.
//!
//! Provides Kubernetes-native implementations of the platform SPI defined in `camel-api`:
//! - [`KubernetesPlatformIdentity`] — auto-detects pod identity from downward API env vars
//! - [`KubernetesLeaderElector`] — Lease-based leader election via `kube`
//! - [`KubernetesReadinessGate`] — patches pod readiness conditions via Kubernetes API

pub mod identity;
pub mod leader_elector;
pub mod readiness_gate;

pub use camel_api::platform::PlatformError;
pub use identity::KubernetesPlatformIdentity;
pub use leader_elector::{KubernetesLeaderElector, LeaderElectorConfig};
pub use readiness_gate::KubernetesReadinessGate;
