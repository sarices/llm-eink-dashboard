use std::collections::BTreeMap;

use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::Serialize;

use super::snapshot::{DataConfidence, UsageSnapshot};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageAggregate {
    pub provider: String,
    pub account_id: String,
    pub model: String,
    pub local_date: NaiveDate,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub confidence: DataConfidence,
}

pub fn local_day(observed_at: DateTime<Utc>) -> NaiveDate {
    observed_at.with_timezone(&Local).date_naive()
}

pub fn aggregate_daily(snapshots: &[UsageSnapshot]) -> Vec<UsageAggregate> {
    let mut groups: BTreeMap<(String, String, String, NaiveDate), UsageAggregate> = BTreeMap::new();
    for snapshot in snapshots {
        let key = (
            snapshot.provider.clone(),
            snapshot.account_id.clone(),
            snapshot.model.clone(),
            local_day(snapshot.observed_at),
        );
        let row = groups.entry(key.clone()).or_insert_with(|| UsageAggregate {
            provider: key.0.clone(),
            account_id: key.1.clone(),
            model: key.2.clone(),
            local_date: key.3,
            total_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            confidence: snapshot.confidence.clone(),
        });
        row.input_tokens = row
            .input_tokens
            .saturating_add(snapshot.input_tokens.unwrap_or(0));
        row.output_tokens = row
            .output_tokens
            .saturating_add(snapshot.output_tokens.unwrap_or(0));
        row.cached_tokens = row
            .cached_tokens
            .saturating_add(snapshot.cached_tokens.unwrap_or(0));
        row.total_tokens = row
            .total_tokens
            .saturating_add(snapshot.effective_total_tokens().unwrap_or(0));
        if !matches!(snapshot.confidence, DataConfidence::Exact) {
            row.confidence = snapshot.confidence.clone();
        }
    }
    groups.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::snapshot::{Period, UsageSnapshot};
    use chrono::Utc;

    #[test]
    fn vendor_total_is_not_overridden() {
        let snapshot = UsageSnapshot {
            source_id: "s".into(),
            provider: "p".into(),
            account_id: "a".into(),
            model: "m".into(),
            observed_at: Utc::now(),
            period: Period::Instant,
            input_tokens: Some(5),
            output_tokens: Some(9),
            cached_tokens: None,
            total_tokens: Some(20),
            balance_amount: None,
            balance_currency: None,
            cost_amount: None,
            cost_currency: None,
            quota_used: None,
            quota_limit: None,
            confidence: DataConfidence::Exact,
            provider_record_id: None,
        };
        assert_eq!(aggregate_daily(&[snapshot])[0].total_tokens, 20);
    }
}
