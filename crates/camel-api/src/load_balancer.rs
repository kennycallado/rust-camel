#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LoadBalanceStrategy {
    #[default]
    RoundRobin,
    Random,
    Weighted(Vec<(String, u32)>),
    Failover,
}

#[derive(Debug, Clone)]
pub struct LoadBalancerConfig {
    pub strategy: LoadBalanceStrategy,
    pub parallel: bool,
}

impl LoadBalancerConfig {
    pub fn round_robin() -> Self {
        Self {
            strategy: LoadBalanceStrategy::RoundRobin,
            parallel: false,
        }
    }

    pub fn random() -> Self {
        Self {
            strategy: LoadBalanceStrategy::Random,
            parallel: false,
        }
    }

    pub fn weighted(weights: Vec<(String, u32)>) -> Self {
        Self {
            strategy: LoadBalanceStrategy::Weighted(weights),
            parallel: false,
        }
    }

    pub fn failover() -> Self {
        Self {
            strategy: LoadBalanceStrategy::Failover,
            parallel: false,
        }
    }

    pub fn parallel(mut self, p: bool) -> Self {
        self.parallel = p;
        self
    }
}
