use std::collections::HashMap;

use btleplug::platform::{Adapter, Peripheral};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::storage::Repository;

pub struct AppState {
    pub sync_running: Mutex<bool>,
    pub sources: Mutex<Vec<SourceRecord>>,
    pub repository: Mutex<Repository>,
    pub ble_devices: Mutex<HashMap<String, String>>,
    pub ble_adapter: AsyncMutex<Option<Adapter>>,
    pub ble_peripherals: AsyncMutex<HashMap<String, Peripheral>>,
}

impl AppState {
    pub fn new() -> Self {
        let data_dir = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .map(|home| home.join("Library/Application Support/LLM E-Ink Dashboard"))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        std::fs::create_dir_all(&data_dir).expect("create dashboard application data directory");
        let database_path = data_dir.join("llm-eink-dashboard.sqlite");
        let repository = Repository::open(&database_path.to_string_lossy())
            .expect("open local dashboard database");
        Self {
            sync_running: Mutex::new(false),
            sources: Mutex::new(Vec::new()),
            repository: Mutex::new(repository),
            ble_devices: Mutex::new(HashMap::new()),
            ble_adapter: AsyncMutex::new(None),
            ble_peripherals: AsyncMutex::new(HashMap::new()),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub selected: bool,
    pub status: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: i64,
    pub occurred_at: String,
    pub level: String,
    pub action: String,
    pub message: String,
    pub details: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub items: Vec<LogEntry>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
    pub total_pages: u32,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub refresh_minutes: u32,
    pub launch_at_login: bool,
    pub quiet_hours_enabled: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            refresh_minutes: 15,
            launch_at_login: false,
            quiet_hours_enabled: false,
        }
    }
}
