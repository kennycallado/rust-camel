# Platforms

Platforms connect rust-camel to the deployment environment. A platform
service implements the `PlatformService` trait from `camel-api` and
exposes three capabilities:

- **Identity**: node name, namespace, and labels. On Kubernetes, these
  come from the Downward API.
- **Leader election**: a `LeadershipService` that coordinates which pod
  owns a named lock. Routes with the `master:` scheme activate only on
  the leader.
- **Readiness gate**: a `ReadinessGate` that reports route readiness to
  the orchestrator. On Kubernetes, it patches `status.conditions`.

The default `NoopPlatformService` serves single-node deployments and
tests. Production on Kubernetes uses `KubernetesPlatformService`.

- [Kubernetes](kubernetes.md): leader election, readiness patching, and
  route activation
