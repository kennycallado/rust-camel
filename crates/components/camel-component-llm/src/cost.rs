//! Pricing configuration for LLM cost tracking.
//!
//! Provides a config-driven pricing table that computes estimated
//! USD cost from input/output token counts.
//!
//! Missing pricing → no cost, no failure (silent skip).

/// Per-model pricing for input and output tokens.
///
/// Prices are expressed as USD per 1,000 tokens (the standard LLM
/// billing unit). The `cost_usd()` method computes the total cost
/// given actual token counts.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PricingTable {
    /// Cost in USD per 1,000 input (prompt) tokens.
    pub input_per_1k_tokens: f64,
    /// Cost in USD per 1,000 output (completion) tokens.
    pub output_per_1k_tokens: f64,
}

impl PricingTable {
    /// Compute the estimated cost in USD for a given token usage.
    ///
    /// Returns `None` if either price is negative (misconfiguration).
    /// Otherwise returns `Some(cost)` where cost is rounded to
    /// 6 decimal places.
    pub fn cost_usd(&self, tokens_in: u32, tokens_out: u32) -> Option<f64> {
        if self.input_per_1k_tokens < 0.0 || self.output_per_1k_tokens < 0.0 {
            return None;
        }
        let cost = (tokens_in as f64 * self.input_per_1k_tokens
            + tokens_out as f64 * self.output_per_1k_tokens)
            / 1000.0;
        Some((cost * 1_000_000.0).round() / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_usd_with_valid_pricing() {
        let table = PricingTable {
            input_per_1k_tokens: 0.0025,
            output_per_1k_tokens: 0.01,
        };
        let cost = table.cost_usd(1000, 500).unwrap();
        // (1000 * 0.0025 + 500 * 0.01) / 1000 = (2.5 + 5.0) / 1000 = 0.0075
        assert!((cost - 0.0075).abs() < 1e-9);
    }

    #[test]
    fn cost_usd_zero_tokens() {
        let table = PricingTable {
            input_per_1k_tokens: 0.0025,
            output_per_1k_tokens: 0.01,
        };
        let cost = table.cost_usd(0, 0).unwrap();
        assert!((cost - 0.0).abs() < 1e-9);
    }

    #[test]
    fn cost_usd_negative_pricing_returns_none() {
        let table = PricingTable {
            input_per_1k_tokens: -0.01,
            output_per_1k_tokens: 0.01,
        };
        assert!(table.cost_usd(100, 100).is_none());
    }

    #[test]
    fn cost_usd_rounds_to_six_decimals() {
        let table = PricingTable {
            input_per_1k_tokens: 0.000001,
            output_per_1k_tokens: 0.000001,
        };
        // (5000 * 0.000001 + 3000 * 0.000001) / 1000 = 0.000008
        let cost = table.cost_usd(5000, 3000).unwrap();
        assert!((cost - 0.000008).abs() < 1e-9);
    }
}
