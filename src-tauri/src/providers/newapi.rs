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
const DATA_QUERY_ATTEMPTS: usize = 3;
const MAX_DATA_QUERY_SECONDS: i64 = 30 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy)]
struct UsageTotal {
    tokens: u64,
    records: usize,
}

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

fn local_period_starts(
    observed_at: chrono::DateTime<Utc>,
) -> Result<(chrono::DateTime<Utc>, chrono::DateTime<Utc>)> {
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
    let month_start = Local
        .with_ymd_and_hms(local_now.year(), local_now.month(), 1, 0, 0, 0)
        .single()
        .context("无法计算 New API 本月统计起始时间")?
        .with_timezone(&Utc);
    Ok((today_start, month_start))
}

fn split_data_query_ranges(start: i64, end: i64) -> Result<Vec<(i64, i64)>> {
    if end < start {
        anyhow::bail!("New API 时间段查询起止时间无效");
    }
    let mut ranges = Vec::new();
    let mut range_start = start;
    loop {
        let range_end = range_start.saturating_add(MAX_DATA_QUERY_SECONDS).min(end);
        ranges.push((range_start, range_end));
        if range_end >= end {
            break;
        }
        // New API uses inclusive timestamp bounds; advance one second to avoid
        // counting records on a split boundary twice.
        range_start = range_end.saturating_add(1);
    }
    Ok(ranges)
}

async fn fetch_personal_data_chunk_tokens(
    client: &reqwest::Client,
    root: &str,
    key: &str,
    user_id: &str,
    start: i64,
    end: i64,
) -> Result<UsageTotal> {
    let mut best: Option<UsageTotal> = None;
    for attempt in 0..DATA_QUERY_ATTEMPTS {
        // New API deployments behind a CDN can return a stale/truncated aggregation.
        // A unique request key and no-cache directives ensure each attempt reaches origin.
        let request_id = format!(
            "{}-{attempt}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let body: serde_json::Value = client
            .get(format!("{root}/api/data/self"))
            .bearer_auth(key)
            .header(NEW_API_USER_HEADER, user_id)
            .header(reqwest::header::USER_AGENT, CLIENT_USER_AGENT)
            .header(reqwest::header::CACHE_CONTROL, "no-cache, no-store")
            .header(reqwest::header::PRAGMA, "no-cache")
            .query(&[
                ("start_timestamp", start.to_string()),
                ("end_timestamp", end.to_string()),
                ("default_time", "hour".to_string()),
                ("_request_id", request_id),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if !body
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            anyhow::bail!(
                "New API 查询时间段用量失败：{}",
                body.get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("查询失败")
            );
        }
        let data = body
            .get("data")
            .and_then(serde_json::Value::as_array)
            .context("New API 时间段用量响应缺少 data")?;
        let result = UsageTotal {
            tokens: data
                .iter()
                .filter_map(|item| item.get("token_used").and_then(json_u64))
                .sum(),
            records: data.len(),
        };
        if best.is_none_or(|current| result.tokens > current.tokens) {
            best = Some(result);
        }
        if attempt + 1 < DATA_QUERY_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    best.context("New API 时间段用量响应为空")
}

async fn fetch_personal_data_tokens(
    client: &reqwest::Client,
    root: &str,
    key: &str,
    user_id: &str,
    start: i64,
    end: i64,
) -> Result<UsageTotal> {
    let mut total = UsageTotal {
        tokens: 0,
        records: 0,
    };
    for (range_start, range_end) in split_data_query_ranges(start, end)? {
        let chunk =
            fetch_personal_data_chunk_tokens(client, root, key, user_id, range_start, range_end)
                .await?;
        total.tokens = total.tokens.saturating_add(chunk.tokens);
        total.records = total.records.saturating_add(chunk.records);
    }
    Ok(total)
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

            // Personal usage is defined by local calendar periods, independently of callers.
            let (today_start, month_start) = local_period_starts(observed_at)?;
            for (period, start) in [(Period::Day, today_start), (Period::Month, month_start)] {
                let total = fetch_personal_data_tokens(
                    &self.client,
                    &root,
                    key,
                    user_id,
                    start.timestamp(),
                    range.end.timestamp(),
                )
                .await?;
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
                    total_tokens: Some(total.tokens),
                    balance_amount: None,
                    balance_currency: None,
                    cost_amount: None,
                    cost_currency: None,
                    quota_used: None,
                    quota_limit: None,
                    confidence: DataConfidence::Exact,
                    provider_record_id: Some(format!(
                        "data/self start={} end={} records={}",
                        start.timestamp(),
                        range.end.timestamp(),
                        total.records
                    )),
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

#[cfg(test)]
mod tests {
    use super::{split_data_query_ranges, MAX_DATA_QUERY_SECONDS};

    #[test]
    fn keeps_ranges_at_or_below_the_provider_limit() {
        let ranges = split_data_query_ranges(100, 100 + MAX_DATA_QUERY_SECONDS).unwrap();

        assert_eq!(ranges, vec![(100, 100 + MAX_DATA_QUERY_SECONDS)]);
        assert!(ranges
            .iter()
            .all(|(start, end)| end - start <= MAX_DATA_QUERY_SECONDS));
    }

    #[test]
    fn splits_a_thirty_one_day_month_without_overlap() {
        let start = 1_000;
        let end = start + 31 * 24 * 60 * 60;
        let ranges = split_data_query_ranges(start, end).unwrap();

        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0], (start, start + MAX_DATA_QUERY_SECONDS));
        assert_eq!(ranges[1].0, ranges[0].1 + 1);
        assert_eq!(ranges[1].1, end);
        assert!(ranges
            .iter()
            .all(|(range_start, range_end)| range_end - range_start <= MAX_DATA_QUERY_SECONDS));
    }

    #[test]
    fn splits_long_ranges_into_contiguous_second_precision_ranges() {
        let start = 2_000;
        let end = start + 32 * 24 * 60 * 60;
        let ranges = split_data_query_ranges(start, end).unwrap();

        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges.first().unwrap().0, start);
        assert_eq!(ranges.last().unwrap().1, end);
        for pair in ranges.windows(2) {
            assert_eq!(pair[1].0, pair[0].1 + 1);
        }
        assert!(ranges
            .iter()
            .all(|(range_start, range_end)| range_end - range_start <= MAX_DATA_QUERY_SECONDS));
    }

    #[test]
    fn rejects_a_reversed_range() {
        assert!(split_data_query_ranges(2, 1).is_err());
    }
}
