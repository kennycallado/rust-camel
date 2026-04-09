use crate::lifecycle::domain::DomainError;
use crate::lifecycle::domain::RuntimeEvent;

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
    Remove,
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

    /// Constructor that returns both the aggregate and the initial `RouteRegistered` event.
    /// Used by command handlers — avoids duplicating event construction outside the domain.
    pub fn register(route_id: impl Into<String>) -> (Self, Vec<RuntimeEvent>) {
        let route_id = route_id.into();
        let aggregate = Self::new(route_id.clone());
        let events = vec![RuntimeEvent::RouteRegistered { route_id }];
        (aggregate, events)
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

    /// Map a persisted RuntimeEvent to the resulting RouteRuntimeState.
    /// Used for event replay — avoids duplicating the state machine outside the domain.
    pub fn state_from_event(event: &RuntimeEvent) -> Option<RouteRuntimeState> {
        match event {
            RuntimeEvent::RouteRegistered { .. } => Some(RouteRuntimeState::Registered),
            RuntimeEvent::RouteStartRequested { .. } => Some(RouteRuntimeState::Starting),
            RuntimeEvent::RouteStarted { .. } => Some(RouteRuntimeState::Started),
            RuntimeEvent::RouteStopped { .. } => Some(RouteRuntimeState::Stopped),
            RuntimeEvent::RouteSuspended { .. } => Some(RouteRuntimeState::Suspended),
            RuntimeEvent::RouteResumed { .. } => Some(RouteRuntimeState::Started),
            RuntimeEvent::RouteFailed { error, .. } => {
                Some(RouteRuntimeState::Failed(error.clone()))
            }
            RuntimeEvent::RouteReloaded { .. } => Some(RouteRuntimeState::Started),
            RuntimeEvent::RouteRemoved { .. } => None,
        }
    }

    pub fn apply_command(
        &mut self,
        cmd: RouteLifecycleCommand,
    ) -> Result<Vec<RuntimeEvent>, DomainError> {
        let invalid = |from: &RouteRuntimeState, to: &str| DomainError::InvalidTransition {
            from: format!("{from:?}"),
            to: to.to_string(),
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
            RouteLifecycleCommand::Remove => match self.state {
                RouteRuntimeState::Registered | RouteRuntimeState::Stopped => {
                    vec![RuntimeEvent::RouteRemoved {
                        route_id: self.route_id.clone(),
                    }]
                }
                _ => {
                    return Err(invalid(&self.state, "Removed"));
                }
            },
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
        assert!(events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::RouteStarted { route_id } if route_id == "r1")));
    }

    #[test]
    fn register_returns_aggregate_and_event() {
        let (agg, events) = RouteRuntimeAggregate::register("r1");
        assert_eq!(agg.route_id(), "r1");
        assert_eq!(agg.state(), &RouteRuntimeState::Registered);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            RuntimeEvent::RouteRegistered { route_id } if route_id == "r1"
        ));
    }

    #[test]
    fn remove_from_registered_emits_removed() {
        let mut agg = RouteRuntimeAggregate::new("r1");
        let events = agg.apply_command(RouteLifecycleCommand::Remove).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            RuntimeEvent::RouteRemoved { route_id } if route_id == "r1"
        ));
    }

    #[test]
    fn remove_from_started_is_invalid() {
        let mut agg = RouteRuntimeAggregate::new("r1");
        agg.apply_command(RouteLifecycleCommand::Start).unwrap();
        let err = agg
            .apply_command(RouteLifecycleCommand::Remove)
            .unwrap_err();
        assert!(err.to_string().contains("invalid transition"));
    }

    #[test]
    fn remove_from_stopped_emits_removed() {
        let mut agg = RouteRuntimeAggregate::new("r1");
        agg.apply_command(RouteLifecycleCommand::Start).unwrap();
        agg.apply_command(RouteLifecycleCommand::Stop).unwrap();
        let events = agg.apply_command(RouteLifecycleCommand::Remove).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            RuntimeEvent::RouteRemoved { route_id } if route_id == "r1"
        ));
    }

    #[test]
    fn state_from_event_maps_all_variants() {
        assert_eq!(
            RouteRuntimeAggregate::state_from_event(&RuntimeEvent::RouteRegistered {
                route_id: "r".into()
            }),
            Some(RouteRuntimeState::Registered)
        );
        assert_eq!(
            RouteRuntimeAggregate::state_from_event(&RuntimeEvent::RouteStartRequested {
                route_id: "r".into()
            }),
            Some(RouteRuntimeState::Starting)
        );
        assert_eq!(
            RouteRuntimeAggregate::state_from_event(&RuntimeEvent::RouteStarted {
                route_id: "r".into()
            }),
            Some(RouteRuntimeState::Started)
        );
        assert_eq!(
            RouteRuntimeAggregate::state_from_event(&RuntimeEvent::RouteStopped {
                route_id: "r".into()
            }),
            Some(RouteRuntimeState::Stopped)
        );
        assert_eq!(
            RouteRuntimeAggregate::state_from_event(&RuntimeEvent::RouteSuspended {
                route_id: "r".into()
            }),
            Some(RouteRuntimeState::Suspended)
        );
        assert_eq!(
            RouteRuntimeAggregate::state_from_event(&RuntimeEvent::RouteResumed {
                route_id: "r".into()
            }),
            Some(RouteRuntimeState::Started)
        );
        assert_eq!(
            RouteRuntimeAggregate::state_from_event(&RuntimeEvent::RouteFailed {
                route_id: "r".into(),
                error: "e".into()
            }),
            Some(RouteRuntimeState::Failed("e".into()))
        );
        assert_eq!(
            RouteRuntimeAggregate::state_from_event(&RuntimeEvent::RouteReloaded {
                route_id: "r".into()
            }),
            Some(RouteRuntimeState::Started)
        );
        assert_eq!(
            RouteRuntimeAggregate::state_from_event(&RuntimeEvent::RouteRemoved {
                route_id: "r".into()
            }),
            None
        );
    }
}
