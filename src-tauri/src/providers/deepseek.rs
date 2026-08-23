use super::{ProviderAdapter, ProviderCapabilities, QueryRange, SourceConfig, ValidationReport};
use crate::domain::snapshot::{DataConfidence, Period, UsageSnapshot};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
pub struct DeepSeekAdapter {
    client: reqwest::Client,
}
impl Default for DeepSeekAdapter {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}
#[async_trait]
impl ProviderAdapter for DeepSeekAdapter {
    fn kind(&self) -> &'static str {
        "deepseek"
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            usage_api: false,
            balance_api: true,
            request_proxy: true,
            model_breakdown: true,
            historical_range: false,
        }
    }
    async fn validate(&self, config: &SourceConfig) -> Result<ValidationReport> {
        let key = config
            .config
            .get("apiKey")
            .and_then(|x| x.as_str())
            .context("DeepSeek API key is required")?;
        let url = config
            .config
            .get("baseUrl")
            .and_then(|x| x.as_str())
            .unwrap_or("https://api.deepseek.com");
        let response = self
            .client
            .get(format!("{url}/user/balance"))
            .bearer_auth(key)
            .send()
            .await?;
        if !response.status().is_success() {
            anyhow::bail!("DeepSeek returned {}", response.status())
        };
        Ok(ValidationReport {
            valid: true,
            message: "DeepSeek balance endpoint is reachable".into(),
            capabilities: self.capabilities(),
        })
    }
    async fn fetch(&self, config: &SourceConfig, _: QueryRange) -> Result<Vec<UsageSnapshot>> {
        let key = config
            .config
            .get("apiKey")
            .and_then(|x| x.as_str())
            .context("DeepSeek API key is required")?;
        let url = config
            .config
            .get("baseUrl")
            .and_then(|x| x.as_str())
            .unwrap_or("https://api.deepseek.com");
        let body: serde_json::Value = self
            .client
            .get(format!("{url}/user/balance"))
            .bearer_auth(key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let infos = body
            .get("balance_infos")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(infos
            .into_iter()
            .map(|item| UsageSnapshot {
                source_id: config.id.clone(),
                provider: "deepseek".into(),
                account_id: "default".into(),
                model: "balance".into(),
                observed_at: Utc::now(),
                period: Period::Instant,
                input_tokens: None,
                output_tokens: None,
                cached_tokens: None,
                total_tokens: None,
                balance_amount: item
                    .get("total_balance")
                    .and_then(|x| x.as_str())
                    .and_then(|x| x.parse::<Decimal>().ok()),
                balance_currency: item
                    .get("currency")
                    .and_then(|x| x.as_str())
                    .map(str::to_string),
                cost_amount: None,
                cost_currency: None,
                quota_used: None,
                quota_limit: None,
                confidence: DataConfidence::Exact,
                provider_record_id: None,
            })
            .collect())
    }
}
