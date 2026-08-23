use super::{ProviderAdapter, ProviderCapabilities, QueryRange, SourceConfig, ValidationReport};
use crate::domain::snapshot::UsageSnapshot;
use anyhow::{Context, Result};
use async_trait::async_trait;
pub struct OpenAiCompatibleAdapter;
#[async_trait]
impl ProviderAdapter for OpenAiCompatibleAdapter {
    fn kind(&self) -> &'static str {
        "openai_compatible"
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            usage_api: false,
            balance_api: false,
            request_proxy: true,
            model_breakdown: true,
            historical_range: false,
        }
    }
    async fn validate(&self, config: &SourceConfig) -> Result<ValidationReport> {
        let url = config
            .config
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .context("baseUrl is required")?;
        let key = config
            .config
            .get("apiKey")
            .and_then(|v| v.as_str())
            .context("apiKey is required")?;
        let client = reqwest::Client::new();
        let response = client
            .get(format!("{}/models", url.trim_end_matches('/')))
            .bearer_auth(key)
            .send()
            .await?;
        if !response.status().is_success() {
            anyhow::bail!("compatible API returned {}", response.status())
        };
        Ok(ValidationReport {
            valid: true,
            message: "Compatible API models endpoint is reachable".into(),
            capabilities: self.capabilities(),
        })
    }
    async fn fetch(&self, _: &SourceConfig, _: QueryRange) -> Result<Vec<UsageSnapshot>> {
        Ok(vec![])
    }
}
