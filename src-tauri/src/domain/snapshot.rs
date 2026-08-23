use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Instant,
    Day,
    Month,
    Total,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataConfidence {
    Exact,
    Estimated,
    Manual,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub source_id: String,
    pub provider: String,
    pub account_id: String,
    pub model: String,
    pub observed_at: DateTime<Utc>,
    pub period: Period,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub balance_amount: Option<Decimal>,
    pub balance_currency: Option<String>,
    pub cost_amount: Option<Decimal>,
    pub cost_currency: Option<String>,
    pub quota_used: Option<Decimal>,
    pub quota_limit: Option<Decimal>,
    pub confidence: DataConfidence,
    #[serde(default)]
    pub provider_record_id: Option<String>,
}

impl UsageSnapshot {
    pub fn effective_total_tokens(&self) -> Option<u64> {
        self.total_tokens
            .or_else(|| match (self.input_tokens, self.output_tokens) {
                (Some(input), Some(output)) => input.checked_add(output),
                _ => None,
            })
    }
}
