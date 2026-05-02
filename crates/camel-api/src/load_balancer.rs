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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_factory() {
        let cfg = LoadBalancerConfig::round_robin();
        assert_eq!(cfg.strategy, LoadBalanceStrategy::RoundRobin);
        assert!(!cfg.parallel);
    }

    #[test]
    fn random_factory() {
        let cfg = LoadBalancerConfig::random();
        assert_eq!(cfg.strategy, LoadBalanceStrategy::Random);
    }

    #[test]
    fn weighted_factory() {
        let weights = vec![("a".to_string(), 3), ("b".to_string(), 1)];
        let cfg = LoadBalancerConfig::weighted(weights.clone());
        assert_eq!(cfg.strategy, LoadBalanceStrategy::Weighted(weights));
    }

    #[test]
    fn failover_factory() {
        let cfg = LoadBalancerConfig::failover();
        assert_eq!(cfg.strategy, LoadBalanceStrategy::Failover);
    }

    #[test]
    fn parallel_builder() {
        let cfg = LoadBalancerConfig::round_robin().parallel(true);
        assert!(cfg.parallel);
    }

    #[test]
    fn default_strategy_is_round_robin() {
        assert_eq!(LoadBalanceStrategy::default(), LoadBalanceStrategy::RoundRobin);
    }

    #[test]
    fn strategy_equality() {
        assert_eq!(LoadBalanceStrategy::RoundRobin, LoadBalanceStrategy::RoundRobin);
        assert_ne!(LoadBalanceStrategy::RoundRobin, LoadBalanceStrategy::Random);
    }

    #[test]
    fn clone_preserves_strategy() {
        let cfg = LoadBalancerConfig::failover().parallel(true);
        let cloned = cfg.clone();
        assert_eq!(cloned.strategy, LoadBalanceStrategy::Failover);
        assert!(cloned.parallel);
    }
}
