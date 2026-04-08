/// Actions the coordinator can take per route.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReloadAction {
    /// Pipeline may have changed — atomic swap (zero-downtime).
    ///
    /// This action is taken when the route exists and `from_uri` is unchanged.
    /// Even if the pipeline is identical, swapping is harmless (atomic pointer swap).
    #[cfg_attr(not(test), allow(dead_code))]
    Swap {
        route_id: String,
    },
    /// Consumer (from_uri) changed — must stop and restart.
    Restart {
        route_id: String,
    },
    /// New route — add and start.
    Add {
        route_id: String,
    },
    /// Route removed from config — stop and delete.
    Remove {
        route_id: String,
    },
    Skip {
        route_id: String,
    },
}
