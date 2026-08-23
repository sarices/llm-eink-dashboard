use super::{ProviderAdapter, ProviderCapabilities, QueryRange, SourceConfig, ValidationReport};
use crate::domain::snapshot::{DataConfidence, Period, UsageSnapshot};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{Datelike, Local, TimeZone, Utc};
use rust_decimal::Decimal;
use std::time::Duration;

const QUOTA_PER_USD: i64 = 500_000;
const NEW_API_USER_HEADER: &str = "New-Api-User";
const CLIENT_USER_AGENT: &str = "cc-switch/1.0";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);

pub struct NewApiAdapter {
    client: reqwest::Client,
}

impl Default for NewApiAdapter {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(REQUEST_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("构造 New API HTTP 客户端失败"),
        }
    }
}

fn api_root(config: &SourceConfig) -> Result<String> {
    let base = config
        .config
        .get("baseUrl")
        .and_then(|value| value.as_str())
        .context("baseUrl is required")?
        .trim_end_matches('/')
        .to_string();
    Ok(base.strip_suffix("/v1").unwrap_or(&base).to_string())
}

fn personal_user_id(config: &SourceConfig) -> Option<&str> {
    config
        .config
        .get("userId")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn quota_to_usd(quota: i64) -> Decimal {
    (Decimal::from(quota) / Decimal::from(QUOTA_PER_USD)).round_dp(2)
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| value.as_str().and_then(|number| number.parse().ok()))
}

async fn fetch_personal_log_tokens(
    client: &reqwest::Client,
    root: &str,
    key: &str,
    user_id: &str,
    start: i64,
    end: i64,
) -> Result<u64> {
    let mut total_tokens = 0_u64;
    // The fallback is only used when New API's aggregate endpoint is inconsistent.
    // Keep the request bounded so a large history cannot block synchronization.
    for page in 1..=5 {
        let body: serde_json::Value = client
            .get(format!("{root}/api/log/self"))
            .bearer_auth(key)
            .header(NEW_API_USER_HEADER, user_id)
            .header(reqwest::header::USER_AGENT, CLIENT_USER_AGENT)
            .query(&[
                ("p", page.to_string()),
                ("page_size", "100".to_string()),
                ("type", "2".to_string()),
                ("start_timestamp", start.to_string()),
                ("end_timestamp", end.to_string()),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let items = body
            .get("data")
            .and_then(|value| value.get("items"))
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        total_tokens = total_tokens.saturating_add(
            items
                .iter()
                .map(|item| {
                    item.get("prompt_tokens")
                        .and_then(json_u64)
                        .unwrap_or_default()
                        .saturating_add(
                            item.get("completion_tokens")
                                .and_then(json_u64)
                                .unwrap_or_default(),
                        )
                })
                .sum::<u64>(),
        );
        let total = body
            .get("data")
            .and_then(|value| value.get("total"))
            .and_then(json_u64)
            .unwrap_or(total_tokens);
        if items.is_empty() || (page as u64 * 100) >= total {
            break;
        }
    }
    Ok(total_tokens)
}

#[async_trait]
impl ProviderAdapter for NewApiAdapter {
    fn kind(&self) -> &'static str {
        "newapi"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            usage_api: true,
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
            .and_then(|value| value.as_str())
            .context("New API token is required")?;
        let response = if let Some(user_id) = personal_user_id(config) {
            self.client
                .get(format!("{}/api/user/self", api_root(config)?))
                .bearer_auth(key)
                .header(NEW_API_USER_HEADER, user_id)
                .header(reqwest::header::USER_AGENT, CLIENT_USER_AGENT)
                .send()
                .await?
        } else {
            let base = config
                .config
                .get("baseUrl")
                .and_then(|value| value.as_str())
                .context("baseUrl is required")?;
            self.client
                .get(format!("{}/models", base.trim_end_matches('/')))
                .bearer_auth(key)
                .send()
                .await?
        };
        if !response.status().is_success() {
            anyhow::bail!("New API returned {}", response.status())
        }
        if personal_user_id(config).is_some() {
            let body: serde_json::Value = response.json().await?;
            if !body
                .get("success")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                || body.get("data").is_none()
            {
                anyhow::bail!(
                    "New API 个人访问令牌无效：{}",
                    body.get("message")
                        .and_then(|value| value.as_str())
                        .unwrap_or("查询失败")
                );
            }
        }
        Ok(ValidationReport {
            valid: true,
            message: if personal_user_id(config).is_some() {
                "New API personal access token is valid".into()
            } else {
                "New API models endpoint is reachable".into()
            },
            capabilities: self.capabilities(),
        })
    }

    async fn fetch(&self, config: &SourceConfig, range: QueryRange) -> Result<Vec<UsageSnapshot>> {
        let key = config
            .config
            .get("apiKey")
            .and_then(|value| value.as_str())
            .context("New API token is required")?;
        let root = api_root(config)?;
        let mut snapshots = Vec::new();
        if let Some(user_id) = personal_user_id(config) {
            let observed_at = Utc::now();
            let body: serde_json::Value = self
                .client
                .get(format!("{root}/api/user/self"))
                .bearer_auth(key)
                .header(NEW_API_USER_HEADER, user_id)
                .header(reqwest::header::USER_AGENT, CLIENT_USER_AGENT)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            if !body
                .get("success")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                anyhow::bail!(
                    "New API 查询个人账户失败：{}",
                    body.get("message")
                        .and_then(|value| value.as_str())
                        .unwrap_or("未知错误")
                );
            }
            let data = body.get("data").context("New API 个人账户响应缺少 data")?;
            let quota = data
                .get("quota")
                .and_then(|value| value.as_i64())
                .context("New API 个人账户响应缺少 quota")?;
            let used_quota = data
                .get("used_quota")
                .and_then(|value| value.as_i64())
                .unwrap_or_default();
            snapshots.push(UsageSnapshot {
                source_id: config.id.clone(),
                provider: "newapi".into(),
                account_id: user_id.into(),
                model: "balance".into(),
                observed_at: Utc::now(),
                period: Period::Instant,
                input_tokens: None,
                output_tokens: None,
                cached_tokens: None,
                total_tokens: None,
                balance_amount: Some(quota_to_usd(quota)),
                balance_currency: Some("USD".into()),
                cost_amount: None,
                cost_currency: None,
                quota_used: Some(quota_to_usd(used_quota)),
                quota_limit: Some(quota_to_usd(quota.saturating_add(used_quota))),
                confidence: DataConfidence::Exact,
                provider_record_id: None,
            });

            let local_now = observed_at.with_timezone(&Local);
            let today_start = Local
                .with_ymd_and_hms(
                    local_now.year(),
                    local_now.month(),
                    local_now.day(),
                    0,
                    0,
                    0,
                )
                .single()
                .context("无法计算 New API 今日统计起始时间")?
                .with_timezone(&Utc);
            let mut day_tokens = 0_u64;
            for (period, start) in [(Period::Day, today_start), (Period::Month, range.start)] {
                let stat: serde_json::Value = self
                    .client
                    .get(format!("{root}/api/log/self/stat"))
                    .bearer_auth(key)
                    .header(NEW_API_USER_HEADER, user_id)
                    .header(reqwest::header::USER_AGENT, CLIENT_USER_AGENT)
                    .query(&[
                        ("type", "2".to_string()),
                        ("start_timestamp", start.timestamp().to_string()),
                        ("end_timestamp", range.end.timestamp().to_string()),
                    ])
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                if !stat
                    .get("success")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                {
                    anyhow::bail!(
                        "New API 查询个人 Token 用量失败：{}",
                        stat.get("message")
                            .and_then(|value| value.as_str())
                            .unwrap_or("查询失败")
                    );
                }
                let mut total_tokens = stat
                    .get("data")
                    .and_then(|value| value.get("tpm"))
                    .and_then(json_u64)
                    .context("New API 个人用量响应缺少 tpm")?;
                if total_tokens == 0 || (period == Period::Month && total_tokens < day_tokens) {
                    total_tokens = fetch_personal_log_tokens(
                        &self.client,
                        &root,
                        key,
                        user_id,
                        start.timestamp(),
                        range.end.timestamp(),
                    )
                    .await?;
                }
                if period == Period::Day {
                    day_tokens = total_tokens;
                }
                snapshots.push(UsageSnapshot {
                    source_id: config.id.clone(),
                    provider: "newapi".into(),
                    account_id: user_id.into(),
                    model: "usage".into(),
                    observed_at,
                    period,
                    input_tokens: None,
                    output_tokens: None,
                    cached_tokens: None,
                    total_tokens: Some(total_tokens),
                    balance_amount: None,
                    balance_currency: None,
                    cost_amount: None,
                    cost_currency: None,
                    quota_used: None,
                    quota_limit: None,
                    confidence: DataConfidence::Exact,
                    provider_record_id: None,
                });
            }
        } else if let Ok(response) = self
            .client
            .get(format!("{root}/api/usage/token/"))
            .bearer_auth(key)
            .send()
            .await
        {
            if let Ok(body) = response.json::<serde_json::Value>().await {
                if let Some(quota) = body
                    .get("data")
                    .and_then(|value| value.get("total_available"))
                    .and_then(|value| value.as_i64())
                {
                    snapshots.push(UsageSnapshot {
                        source_id: config.id.clone(),
                        provider: "newapi".into(),
                        account_id: "default".into(),
                        model: "balance".into(),
                        observed_at: Utc::now(),
                        period: Period::Instant,
                        input_tokens: None,
                        output_tokens: None,
                        cached_tokens: None,
                        total_tokens: None,
                        balance_amount: Some(Decimal::from(quota)),
                        balance_currency: Some("quota".into()),
                        cost_amount: None,
                        cost_currency: None,
                        quota_used: None,
                        quota_limit: None,
                        confidence: DataConfidence::Exact,
                        provider_record_id: None,
                    });
                }
            }
        }
        if personal_user_id(config).is_none() {
            let body: serde_json::Value = self
                .client
                .get(format!("{root}/api/log/token"))
                .bearer_auth(key)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let logs = body
                .get("data")
                .and_then(|value| value.as_array())
                .cloned()
                .unwrap_or_default();
            snapshots.extend(
                logs.into_iter()
                    .filter_map(|item| {
                        let timestamp = item.get("created_at").and_then(|value| value.as_i64())?;
                        let observed_at = Utc.timestamp_opt(timestamp, 0).single()?;
                        if observed_at < range.start || observed_at > range.end {
                            return None;
                        }
                        let input_tokens =
                            item.get("prompt_tokens").and_then(|value| value.as_u64());
                        let output_tokens = item
                            .get("completion_tokens")
                            .and_then(|value| value.as_u64());
                        let total_tokens = input_tokens
                            .zip(output_tokens)
                            .map(|(input, output)| input + output);
                        Some(UsageSnapshot {
                            source_id: config.id.clone(),
                            provider: "newapi".into(),
                            account_id: item
                                .get("token_name")
                                .and_then(|value| value.as_str())
                                .unwrap_or("default")
                                .into(),
                            model: item
                                .get("model_name")
                                .and_then(|value| value.as_str())
                                .unwrap_or("unknown")
                                .into(),
                            observed_at,
                            period: Period::Instant,
                            input_tokens,
                            output_tokens,
                            cached_tokens: None,
                            total_tokens,
                            balance_amount: None,
                            balance_currency: None,
                            cost_amount: None,
                            cost_currency: None,
                            quota_used: None,
                            quota_limit: None,
                            confidence: DataConfidence::Exact,
                            provider_record_id: item
                                .get("id")
                                .and_then(|value| value.as_i64())
                                .map(|value| value.to_string()),
                        })
                    })
                    .collect::<Vec<_>>(),
            );
        }
        Ok(snapshots)
    }
}
