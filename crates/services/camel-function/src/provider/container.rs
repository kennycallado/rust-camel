pub struct ContainerProvider {
    docker: bollard::Docker,
}

impl ContainerProvider {
    pub fn new() -> Result<Self, super::ProviderError> {
        let docker = bollard::Docker::connect_with_local_defaults()
            .map_err(|e| super::ProviderError::SpawnFailed(format!("docker connect: {e}")))?;
        Ok(Self { docker })
    }
}
