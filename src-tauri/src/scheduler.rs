use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleConfig {
    pub interval_minutes: u32,
    pub retry_count: u8,
    pub enabled: bool,
}
impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            interval_minutes: 15,
            retry_count: 2,
            enabled: false,
        }
    }
}

pub fn next_run_after(last_success: DateTime<Utc>, config: &ScheduleConfig) -> DateTime<Utc> {
    last_success + Duration::minutes(config.interval_minutes.clamp(1, 24 * 60) as i64)
}
pub fn is_due(
    last_success: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    config: &ScheduleConfig,
) -> bool {
    config.enabled
        && last_success
            .map(|time| now >= next_run_after(time, config))
            .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_schedule_is_never_due() {
        let config = ScheduleConfig::default();
        assert!(!is_due(None, Utc::now(), &config));
    }
}
