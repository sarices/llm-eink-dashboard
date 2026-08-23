use super::{ProviderAdapter, ProviderCapabilities, QueryRange, SourceConfig, ValidationReport};
use crate::domain::snapshot::{DataConfidence, Period, UsageSnapshot};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio::{
    process::Command,
    time::{timeout, Duration},
};
const MAX_OUTPUT: usize = 1_000_000;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScriptResult {
    schema_version: u8,
    source: String,
    updated_at: DateTime<Utc>,
    accounts: Vec<ScriptAccount>,
}
#[derive(Deserialize)]
struct ScriptAccount {
    id: String,
    #[allow(dead_code)]
    label: Option<String>,
    balance: Option<Amount>,
    models: Vec<ScriptModel>,
}
#[derive(Deserialize)]
struct Amount {
    amount: Decimal,
    currency: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScriptModel {
    id: String,
    period: Period,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_tokens: Option<u64>,
    total_tokens: Option<u64>,
    cost: Option<Amount>,
    confidence: DataConfidence,
}
pub struct ScriptAdapter;

fn parse_output(output: &[u8], source_id: &str) -> Result<Vec<UsageSnapshot>> {
    if output.len() > MAX_OUTPUT {
        anyhow::bail!("script output exceeds 1 MB")
    };
    let parsed: ScriptResult =
        serde_json::from_slice(output).context("script stdout must be valid JSON")?;
    if parsed.schema_version != 1 {
        anyhow::bail!("unsupported script schema version")
    };
    let provider = parsed.source;
    let observed_at = parsed.updated_at;
    Ok(parsed
        .accounts
        .into_iter()
        .flat_map(|account| {
            let balance = account.balance;
            let provider = provider.clone();
            let account_id = account.id;
            account.models.into_iter().map(move |model| UsageSnapshot {
                source_id: source_id.into(),
                provider: provider.clone(),
                account_id: account_id.clone(),
                model: model.id,
                observed_at,
                period: model.period,
                input_tokens: model.input_tokens,
                output_tokens: model.output_tokens,
                cached_tokens: model.cached_tokens,
                total_tokens: model.total_tokens,
                balance_amount: balance.as_ref().map(|x| x.amount),
                balance_currency: balance.as_ref().map(|x| x.currency.clone()),
                cost_amount: model.cost.as_ref().map(|x| x.amount),
                cost_currency: model.cost.as_ref().map(|x| x.currency.clone()),
                quota_used: None,
                quota_limit: None,
                confidence: model.confidence,
                provider_record_id: None,
            })
        })
        .collect())
}

impl ScriptAdapter {
    async fn execute(
        &self,
        config: &SourceConfig,
        range: &QueryRange,
    ) -> Result<Vec<UsageSnapshot>> {
        let path = config
            .config
            .get("path")
            .and_then(|v| v.as_str())
            .context("script path is required")?;
        let output = timeout(
            Duration::from_secs(30),
            Command::new(path)
                .env_clear()
                .env("LLM_DASHBOARD_SOURCE_ID", &config.id)
                .env("LLM_DASHBOARD_RANGE_START", range.start.to_rfc3339())
                .env("LLM_DASHBOARD_RANGE_END", range.end.to_rfc3339())
                .env("LLM_DASHBOARD_CONFIG_JSON", config.config.to_string())
                .output(),
        )
        .await
        .context("script timed out")??;
        if !output.status.success() {
            anyhow::bail!(
                "script failed: {}",
                String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)])
            )
        };
        parse_output(&output.stdout, &config.id)
    }
}
#[async_trait]
impl ProviderAdapter for ScriptAdapter {
    fn kind(&self) -> &'static str {
        "script"
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            usage_api: true,
            balance_api: true,
            request_proxy: false,
            model_breakdown: true,
            historical_range: true,
        }
    }
    async fn validate(&self, config: &SourceConfig) -> Result<ValidationReport> {
        self.execute(
            config,
            &QueryRange {
                start: Utc::now(),
                end: Utc::now(),
            },
        )
        .await?;
        Ok(ValidationReport {
            valid: true,
            message: "Script JSON passed schema validation".into(),
            capabilities: self.capabilities(),
        })
    }
    async fn fetch(&self, config: &SourceConfig, range: QueryRange) -> Result<Vec<UsageSnapshot>> {
        self.execute(config, &range).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_fixture_contract() {
        let snapshots = parse_output(
            include_bytes!("../../../fixtures/script-valid.json"),
            "fixture-source",
        )
        .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].source_id, "fixture-source");
        assert_eq!(snapshots[0].effective_total_tokens(), Some(4600));
    }
    #[test]
    fn rejects_unsupported_schema_fixture() {
        assert!(parse_output(
            include_bytes!("../../../fixtures/script-invalid.json"),
            "fixture-source"
        )
        .is_err());
    }
}
