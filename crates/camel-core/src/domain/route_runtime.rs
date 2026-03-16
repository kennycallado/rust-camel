use crate::CamelError;
use crate::domain::RuntimeEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteRuntimeState {
    Registered,
    Starting,
    Started,
    Suspended,
    Stopping,
    Stopped,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteLifecycleCommand {
    Start,
    Stop,
    Suspend,
    Resume,
    Reload,
    Fail(String),
}

#[derive(Debug, Clone)]
pub struct RouteRuntimeAggregate {
    route_id: String,
    state: RouteRuntimeState,
    version: u64,
}

impl RouteRuntimeAggregate {
    pub fn new(route_id: impl Into<String>) -> Self {
        Self {
            route_id: route_id.into(),
            state: RouteRuntimeState::Registered,
            version: 0,
        }
    }

    pub fn from_snapshot(
        route_id: impl Into<String>,
        state: RouteRuntimeState,
        version: u64,
    ) -> Self {
        Self {
            route_id: route_id.into(),
            state,
            version,
        }
    }

    pub fn state(&self) -> &RouteRuntimeState {
        &self.state
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn route_id(&self) -> &str {
        &self.route_id
    }

    pub fn apply_command(
        &mut self,
        cmd: RouteLifecycleCommand,
    ) -> Result<Vec<RuntimeEvent>, CamelError> {
        let invalid = |from: &RouteRuntimeState, to: &str| {
            CamelError::ProcessorError(format!("invalid transition: {from:?} -> {to}"))
        };

        let events = match cmd {
            RouteLifecycleCommand::Start => match self.state {
                RouteRuntimeState::Registered | RouteRuntimeState::Stopped => {
                    self.state = RouteRuntimeState::Started;
                    vec![
                        RuntimeEvent::RouteStartRequested {
                            route_id: self.route_id.clone(),
                        },
                        RuntimeEvent::RouteStarted {
                            route_id: self.route_id.clone(),
                        },
                    ]
                }
                _ => return Err(invalid(&self.state, "Started")),
            },
            RouteLifecycleCommand::Stop => match self.state {
                RouteRuntimeState::Started
                | RouteRuntimeState::Suspended
                | RouteRuntimeState::Failed(_) => {
                    self.state = RouteRuntimeState::Stopped;
                    vec![RuntimeEvent::RouteStopped {
                        route_id: self.route_id.clone(),
                    }]
                }
                _ => return Err(invalid(&self.state, "Stopped")),
            },
            RouteLifecycleCommand::Suspend => match self.state {
                RouteRuntimeState::Started => {
                    self.state = RouteRuntimeState::Suspended;
                    vec![RuntimeEvent::RouteSuspended {
                        route_id: self.route_id.clone(),
                    }]
                }
                _ => return Err(invalid(&self.state, "Suspended")),
            },
            RouteLifecycleCommand::Resume => match self.state {
                RouteRuntimeState::Suspended => {
                    self.state = RouteRuntimeState::Started;
                    vec![RuntimeEvent::RouteResumed {
                        route_id: self.route_id.clone(),
                    }]
                }
                _ => return Err(invalid(&self.state, "Started")),
            },
            RouteLifecycleCommand::Reload => match self.state {
                RouteRuntimeState::Started
                | RouteRuntimeState::Suspended
                | RouteRuntimeState::Stopped
                | RouteRuntimeState::Failed(_) => {
                    self.state = RouteRuntimeState::Started;
                    vec![RuntimeEvent::RouteReloaded {
                        route_id: self.route_id.clone(),
                    }]
                }
                _ => return Err(invalid(&self.state, "Started")),
            },
            RouteLifecycleCommand::Fail(error) => {
                self.state = RouteRuntimeState::Failed(error.clone());
                vec![RuntimeEvent::RouteFailed {
                    route_id: self.route_id.clone(),
                    error,
                }]
            }
        };

        self.version += 1;
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cannot_suspend_when_stopped() {
        let mut agg = RouteRuntimeAggregate::new("r1");
        agg.state = RouteRuntimeState::Stopped;
        let err = agg
            .apply_command(RouteLifecycleCommand::Suspend)
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }

    #[test]
    fn start_emits_route_started_event() {
        let mut agg = RouteRuntimeAggregate::new("r1");
        let events = agg.apply_command(RouteLifecycleCommand::Start).unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RuntimeEvent::RouteStarted { route_id } if route_id == "r1"))
        );
    }
}
