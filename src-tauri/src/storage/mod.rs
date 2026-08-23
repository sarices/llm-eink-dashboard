use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::snapshot::{Period, UsageSnapshot};
use crate::providers::SourceConfig;

pub struct Repository {
    conn: Connection,
}

impl Repository {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS sources (
              id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL,
              enabled INTEGER NOT NULL, config_json TEXT NOT NULL, secret_ref TEXT
            );
            CREATE TABLE IF NOT EXISTS snapshots (
              id INTEGER PRIMARY KEY, source_id TEXT NOT NULL, provider TEXT NOT NULL,
              account_id TEXT NOT NULL, model TEXT NOT NULL, observed_at TEXT NOT NULL,
              period TEXT NOT NULL, total_tokens INTEGER, payload TEXT NOT NULL,
              UNIQUE(source_id, account_id, model, period, observed_at)
            );
            CREATE TABLE IF NOT EXISTS sync_runs (
              sync_id TEXT PRIMARY KEY, started_at TEXT NOT NULL, ended_at TEXT,
              status TEXT NOT NULL, error_summary TEXT
            );
            CREATE TABLE IF NOT EXISTS logs (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              occurred_at TEXT NOT NULL,
              level TEXT NOT NULL,
              action TEXT NOT NULL,
              message TEXT NOT NULL,
              details TEXT
            );
            CREATE TABLE IF NOT EXISTS app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS idx_snapshots_time ON snapshots(observed_at DESC);
            CREATE INDEX IF NOT EXISTS idx_logs_time ON logs(occurred_at DESC);
            DELETE FROM logs WHERE julianday(occurred_at) < julianday('now', '-30 days');
            ",
        )?;
        Ok(Self { conn })
    }

    pub fn upsert_source(&self, source: &SourceConfig, secret_ref: Option<&str>) -> Result<()> {
        reject_inline_secret(&source.config)?;
        self.conn.execute(
            "INSERT INTO sources(id,name,kind,enabled,config_json,secret_ref) VALUES(?,?,?,?,?,?)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name,kind=excluded.kind,enabled=excluded.enabled,config_json=excluded.config_json,secret_ref=COALESCE(excluded.secret_ref,sources.secret_ref)",
            params![source.id, source.name, source.kind, source.enabled, serde_json::to_string(&source.config)?, secret_ref],
        )?;
        Ok(())
    }

    pub fn save_setting<T: serde::Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.conn.execute("INSERT INTO app_settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, serde_json::to_string(value)?])?;
        Ok(())
    }

    pub fn load_setting<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let value: Option<String> = self
            .conn
            .query_row("SELECT value FROM app_settings WHERE key=?", [key], |row| {
                row.get(0)
            })
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }

    pub fn save_snapshot(&self, snapshot: &UsageSnapshot) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO snapshots(source_id,provider,account_id,model,observed_at,period,total_tokens,payload) VALUES(?,?,?,?,?,?,?,?)",
            params![snapshot.source_id, snapshot.provider, snapshot.account_id, snapshot.model, snapshot.observed_at.to_rfc3339(), period_name(&snapshot.period), snapshot.effective_total_tokens(), serde_json::to_string(snapshot)?],
        )?;
        Ok(())
    }

    pub fn set_secret_ref(&self, id: &str, secret_ref: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sources SET secret_ref=? WHERE id=?",
            params![secret_ref, id],
        )?;
        Ok(())
    }

    pub fn secret_ref(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT secret_ref FROM sources WHERE id=?", [id], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn delete_source(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM sources WHERE id=?", [id])?;
        Ok(())
    }

    pub fn list_sources(&self) -> Result<Vec<SourceConfig>> {
        let mut statement = self
            .conn
            .prepare("SELECT id,name,kind,enabled,config_json FROM sources ORDER BY name")?;
        let rows = statement.query_map([], |row| {
            Ok(SourceConfig {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                enabled: row.get::<_, i64>(3)? != 0,
                config: serde_json::from_str(&row.get::<_, String>(4)?)
                    .unwrap_or(serde_json::Value::Object(Default::default())),
            })
        })?;
        let sources = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(sources)
    }

    pub fn recent_snapshots(&self, limit: usize) -> Result<Vec<UsageSnapshot>> {
        let mut statement = self
            .conn
            .prepare("SELECT payload FROM snapshots ORDER BY observed_at DESC LIMIT ?")?;
        let snapshots = statement
            .query_map([limit as i64], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str::<UsageSnapshot>(&row?)?))
            .collect::<Result<Vec<_>>>()?;
        Ok(snapshots)
    }

    pub fn start_sync(&self, sync_id: &str, started_at: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_runs(sync_id,started_at,status) VALUES(?,?,?)",
            params![sync_id, started_at.to_rfc3339(), "running"],
        )?;
        Ok(())
    }

    pub fn finish_sync(&self, sync_id: &str, status: &str, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE sync_runs SET ended_at=?,status=?,error_summary=? WHERE sync_id=?",
            params![Utc::now().to_rfc3339(), status, error, sync_id],
        )?;
        Ok(())
    }

    pub fn log_event(
        &self,
        level: &str,
        action: &str,
        message: &str,
        details: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO logs(occurred_at,level,action,message,details) VALUES(?,?,?,?,?)",
            params![Utc::now().to_rfc3339(), level, action, message, details],
        )?;
        Ok(())
    }

    pub fn recent_logs(&self, limit: usize) -> Result<Vec<crate::state::LogEntry>> {
        let mut statement = self.conn.prepare(
            "SELECT id,occurred_at,level,action,message,details FROM logs ORDER BY occurred_at DESC, id DESC LIMIT ?",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok(crate::state::LogEntry {
                id: row.get(0)?,
                occurred_at: row.get(1)?,
                level: row.get(2)?,
                action: row.get(3)?,
                message: row.get(4)?,
                details: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn logs_page(
        &self,
        page: u32,
        page_size: u32,
    ) -> Result<(Vec<crate::state::LogEntry>, u64)> {
        let total = self
            .conn
            .query_row("SELECT COUNT(*) FROM logs", [], |row| row.get::<_, i64>(0))?
            .max(0) as u64;
        let offset = page.saturating_sub(1) as i64 * page_size as i64;
        let mut statement = self.conn.prepare(
            "SELECT id,occurred_at,level,action,message,details FROM logs ORDER BY occurred_at DESC, id DESC LIMIT ? OFFSET ?",
        )?;
        let rows = statement.query_map([page_size as i64, offset], |row| {
            Ok(crate::state::LogEntry {
                id: row.get(0)?,
                occurred_at: row.get(1)?,
                level: row.get(2)?,
                action: row.get(3)?,
                message: row.get(4)?,
                details: row.get(5)?,
            })
        })?;
        Ok((rows.collect::<rusqlite::Result<Vec<_>>>()?, total))
    }
}

fn reject_inline_secret(config: &serde_json::Value) -> Result<()> {
    for key in ["apiKey", "api_key", "authorization", "token", "secret"] {
        if config
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            anyhow::bail!(
                "{} must be stored as a Keychain reference, not in SQLite",
                key
            );
        }
    }
    Ok(())
}

fn period_name(period: &Period) -> &'static str {
    match period {
        Period::Instant => "instant",
        Period::Day => "day",
        Period::Month => "month",
        Period::Total => "total",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::snapshot::{DataConfidence, Period};
    use tempfile::NamedTempFile;

    #[test]
    fn settings_round_trip() {
        let db = NamedTempFile::new().unwrap();
        let repository = Repository::open(db.path().to_str().unwrap()).unwrap();
        repository
            .save_setting("test", &serde_json::json!({"enabled":true}))
            .unwrap();
        let setting: serde_json::Value = repository.load_setting("test").unwrap().unwrap();
        assert_eq!(setting["enabled"], true);
    }

    #[test]
    fn rejects_inline_api_keys() {
        let db = NamedTempFile::new().unwrap();
        let repository = Repository::open(db.path().to_str().unwrap()).unwrap();
        let source = SourceConfig {
            id: "one".into(),
            name: "source".into(),
            kind: "script".into(),
            enabled: true,
            config: serde_json::json!({"apiKey":"sk-not-allowed"}),
        };
        assert!(repository.upsert_source(&source, None).is_err());
    }

    #[test]
    fn duplicate_snapshots_are_ignored() {
        let db = NamedTempFile::new().unwrap();
        let repository = Repository::open(db.path().to_str().unwrap()).unwrap();
        let snapshot = UsageSnapshot {
            source_id: "one".into(),
            provider: "mock".into(),
            account_id: "default".into(),
            model: "model".into(),
            observed_at: Utc::now(),
            period: Period::Instant,
            input_tokens: Some(2),
            output_tokens: Some(3),
            cached_tokens: None,
            total_tokens: Some(5),
            balance_amount: None,
            balance_currency: None,
            cost_amount: None,
            cost_currency: None,
            quota_used: None,
            quota_limit: None,
            confidence: DataConfidence::Exact,
            provider_record_id: None,
        };
        repository.save_snapshot(&snapshot).unwrap();
        repository.save_snapshot(&snapshot).unwrap();
        assert_eq!(repository.recent_snapshots(10).unwrap().len(), 1);
    }

    #[test]
    fn logs_round_trip_in_reverse_chronological_order() {
        let db = NamedTempFile::new().unwrap();
        let repository = Repository::open(db.path().to_str().unwrap()).unwrap();
        repository
            .log_event("info", "test.first", "first", None)
            .unwrap();
        repository
            .log_event("error", "test.second", "second", Some("details"))
            .unwrap();
        let logs = repository.recent_logs(10).unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].message, "second");
        assert_eq!(logs[0].details.as_deref(), Some("details"));
    }
}
