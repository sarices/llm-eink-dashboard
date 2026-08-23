use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::snapshot::UsageSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingRule {
    pub model: String,
    pub currency: String,
    pub effective_at: DateTime<Utc>,
    pub input_per_million: Decimal,
    pub output_per_million: Decimal,
    pub cached_per_million: Decimal,
    pub source: String,
}

pub fn calculate_cost(snapshot: &UsageSnapshot, rule: &PricingRule) -> Decimal {
    let million = Decimal::from(1_000_000_u64);
    (Decimal::from(snapshot.input_tokens.unwrap_or(0)) * rule.input_per_million
        + Decimal::from(snapshot.output_tokens.unwrap_or(0)) * rule.output_per_million
        + Decimal::from(snapshot.cached_tokens.unwrap_or(0)) * rule.cached_per_million)
        / million
}
