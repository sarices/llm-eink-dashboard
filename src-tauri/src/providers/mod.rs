use crate::domain::snapshot::UsageSnapshot;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
pub mod deepseek;
pub mod newapi;
pub mod openai_compatible;
pub mod script;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub kind: String,
    pub config: serde_json::Value,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub usage_api: bool,
    pub balance_api: bool,
    pub request_proxy: bool,
    pub model_breakdown: bool,
    pub historical_range: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub message: String,
    pub capabilities: ProviderCapabilities,
}
#[derive(Debug, Clone)]
pub struct QueryRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn kind(&self) -> &'static str;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn validate(&self, config: &SourceConfig) -> Result<ValidationReport>;
    async fn fetch(&self, config: &SourceConfig, range: QueryRange) -> Result<Vec<UsageSnapshot>>;
}

pub fn with_api_key(mut config: SourceConfig, api_key: String) -> SourceConfig {
    let object = config
        .config
        .as_object_mut()
        .expect("source config must be an object");
    object.insert("apiKey".into(), serde_json::Value::String(api_key));
    config
}

pub async fn validate_source(config: &SourceConfig) -> Result<ValidationReport> {
    match config.kind.as_str() {
        "deepseek" => deepseek::DeepSeekAdapter::default().validate(config).await,
        "openai_compatible" => {
            openai_compatible::OpenAiCompatibleAdapter
                .validate(config)
                .await
        }
        "newapi" => newapi::NewApiAdapter::default().validate(config).await,
        "script" => script::ScriptAdapter.validate(config).await,
        kind => anyhow::bail!("unsupported provider kind: {kind}"),
    }
}

pub async fn fetch_source(config: &SourceConfig, range: QueryRange) -> Result<Vec<UsageSnapshot>> {
    match config.kind.as_str() {
        "deepseek" => {
            deepseek::DeepSeekAdapter::default()
                .fetch(config, range)
                .await
        }
        "openai_compatible" => {
            openai_compatible::OpenAiCompatibleAdapter
                .fetch(config, range)
                .await
        }
        "newapi" => newapi::NewApiAdapter::default().fetch(config, range).await,
        "script" => script::ScriptAdapter.fetch(config, range).await,
        kind => anyhow::bail!("unsupported provider kind: {kind}"),
    }
}
