use btleplug::api::Peripheral as _;
use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_autostart::ManagerExt;
use uuid::Uuid;

use crate::{
    ble::{
        connect_nrf_epd, create_adapter, scan_nrf_epd, transfer_epd_image_on_peripheral,
        BleConnectionInfo, BleDevice, TransferResult,
    },
    domain::snapshot::UsageSnapshot,
    epd::{chunk_image, DeviceConfig, BW_LAYER},
    providers::{fetch_source, validate_source, with_api_key, QueryRange, SourceConfig},
    render::{render_mono_bitmap, render_svg, render_tricolor_bitmaps, DashboardViewModel},
    scheduler::{is_due, ScheduleConfig},
    secrets,
    state::{AppSettings, AppState, LogPage, SourceRecord},
};

const LAST_EPD_DEVICE_NAME_SETTING: &str = "last_epd_device_name";
const SELECTED_SOURCE_ID_SETTING: &str = "selected_source_id";

fn record_log(state: &AppState, level: &str, action: &str, message: &str, details: Option<&str>) {
    let _ = state
        .repository
        .lock()
        .log_event(level, action, message, details);
}

#[tauri::command]
pub fn list_sources(state: State<'_, AppState>) -> Result<Vec<SourceRecord>, String> {
    let repository = state.repository.lock();
    let sources = repository
        .list_sources()
        .map_err(|error| error.to_string())?;
    let selected_id = selected_source_id(&repository, &sources)?;
    Ok(source_records(sources, selected_id.as_deref()))
}

fn source_records(sources: Vec<SourceConfig>, selected_id: Option<&str>) -> Vec<SourceRecord> {
    sources
        .into_iter()
        .map(|source| {
            let selected = selected_id == Some(source.id.as_str());
            SourceRecord {
                id: source.id,
                name: source.name,
                kind: source.kind,
                enabled: source.enabled,
                selected,
                status: if selected {
                    "当前读取".into()
                } else if source.enabled {
                    "已配置".into()
                } else {
                    "已停用".into()
                },
            }
        })
        .collect()
}

fn selected_source_id(
    repository: &crate::storage::Repository,
    sources: &[SourceConfig],
) -> Result<Option<String>, String> {
    let selected_id = repository
        .load_setting::<String>(SELECTED_SOURCE_ID_SETTING)
        .map_err(|error| error.to_string())?;
    if selected_id.as_ref().is_some_and(|id| {
        sources
            .iter()
            .any(|source| source.id == *id && source.enabled)
    }) {
        return Ok(selected_id);
    }
    let fallback = sources
        .iter()
        .find(|source| source.enabled)
        .map(|source| source.id.clone());
    if let Some(source_id) = fallback.as_ref() {
        repository
            .save_setting(SELECTED_SOURCE_ID_SETTING, source_id)
            .map_err(|error| error.to_string())?;
    }
    Ok(fallback)
}

fn selected_source(repository: &crate::storage::Repository) -> Result<SourceConfig, String> {
    let sources = repository
        .list_sources()
        .map_err(|error| error.to_string())?;
    let selected_id = selected_source_id(repository, &sources)?
        .ok_or("请先在“数据源”中选择一个已启用的数据源")?;
    sources
        .into_iter()
        .find(|source| source.id == selected_id && source.enabled)
        .ok_or_else(|| "当前读取的数据源不可用；请重新选择".to_string())
}

#[tauri::command]
pub fn save_source(state: State<'_, AppState>, source: SourceRecord) -> Result<(), String> {
    let config = SourceConfig {
        id: source.id.clone(),
        name: source.name.clone(),
        kind: source.kind.clone(),
        enabled: source.enabled,
        config: serde_json::json!({}),
    };
    state
        .repository
        .lock()
        .upsert_source(&config, None)
        .map_err(|error| error.to_string())?;
    let mut sources = state.sources.lock();
    if let Some(current) = sources.iter_mut().find(|item| item.id == source.id) {
        *current = source;
    } else {
        sources.push(source);
    }
    Ok(())
}

#[tauri::command]
pub fn save_source_config(state: State<'_, AppState>, source: SourceConfig) -> Result<(), String> {
    let source_id = source.id.clone();
    state
        .repository
        .lock()
        .upsert_source(&source, None)
        .map_err(|error| error.to_string())?;
    let record = SourceRecord {
        id: source.id,
        name: source.name,
        kind: source.kind,
        enabled: source.enabled,
        selected: false,
        status: "已保存".into(),
    };
    let mut sources = state.sources.lock();
    if let Some(current) = sources.iter_mut().find(|item| item.id == record.id) {
        *current = record;
    } else {
        sources.push(record);
    }
    record_log(
        &state,
        "info",
        "source.save",
        "数据源配置已保存",
        Some(&source_id),
    );
    Ok(())
}

#[tauri::command]
pub fn select_source(state: State<'_, AppState>, id: String) -> Result<SourceRecord, String> {
    let repository = state.repository.lock();
    let sources = repository
        .list_sources()
        .map_err(|error| error.to_string())?;
    let source = sources
        .iter()
        .find(|source| source.id == id)
        .ok_or("未找到数据源")?;
    if !source.enabled {
        return Err("不能选择已停用的数据源".into());
    }
    repository
        .save_setting(SELECTED_SOURCE_ID_SETTING, &id)
        .map_err(|error| error.to_string())?;
    drop(repository);
    record_log(
        &state,
        "info",
        "source.select",
        "当前读取数据源已更新",
        Some(&id),
    );
    Ok(SourceRecord {
        id: source.id.clone(),
        name: source.name.clone(),
        kind: source.kind.clone(),
        enabled: source.enabled,
        selected: true,
        status: "当前读取".into(),
    })
}

#[tauri::command]
pub fn save_source_secret(
    state: State<'_, AppState>,
    source_id: String,
    secret_ref: String,
    secret: String,
) -> Result<(), String> {
    secrets::save_secret(&secret_ref, &secret).map_err(|error| error.to_string())?;
    state
        .repository
        .lock()
        .set_secret_ref(&source_id, &secret_ref)
        .map_err(|error| error.to_string())?;
    record_log(
        &state,
        "info",
        "source.secret",
        "数据源凭据已更新",
        Some(&source_id),
    );
    Ok(())
}

#[tauri::command]
pub async fn test_source(state: State<'_, AppState>, source_id: String) -> Result<String, String> {
    let source = state
        .repository
        .lock()
        .list_sources()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|source| source.id == source_id)
        .ok_or("未找到数据源")?;
    test_source_config_inner(&state, source, None, &source_id).await
}

async fn test_source_config_inner(
    state: &AppState,
    source: SourceConfig,
    api_key: Option<String>,
    source_id: &str,
) -> Result<String, String> {
    let configured = if matches!(
        source.kind.as_str(),
        "deepseek" | "openai_compatible" | "newapi"
    ) {
        let key = match api_key.filter(|value| !value.trim().is_empty()) {
            Some(value) => value,
            None => {
                let reference = state
                    .repository
                    .lock()
                    .secret_ref(&source.id)
                    .map_err(|error| error.to_string())?
                    .ok_or("尚未设置 Keychain 密钥")?;
                secrets::load_secret(&reference).map_err(|error| error.to_string())?
            }
        };
        with_api_key(source, key)
    } else {
        source
    };
    let report = validate_source(&configured)
        .await
        .map_err(|error| error.to_string())?;
    record_log(
        state,
        "info",
        "source.test",
        "数据源连通性测试完成",
        Some(source_id),
    );
    Ok(report.message)
}

#[tauri::command]
pub async fn test_source_config(
    state: State<'_, AppState>,
    source: SourceConfig,
    api_key: Option<String>,
) -> Result<String, String> {
    let source_id = source.id.clone();
    test_source_config_inner(&state, source, api_key, &source_id).await
}

#[tauri::command]
pub fn list_source_configs(state: State<'_, AppState>) -> Result<Vec<SourceConfig>, String> {
    state
        .repository
        .lock()
        .list_sources()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn delete_source(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let secret_ref = state
        .repository
        .lock()
        .secret_ref(&id)
        .map_err(|error| error.to_string())?;
    if let Some(secret_ref) = secret_ref {
        secrets::delete_secret(&secret_ref).map_err(|error| error.to_string())?;
    }
    state
        .repository
        .lock()
        .delete_source(&id)
        .map_err(|error| error.to_string())?;
    let repository = state.repository.lock();
    if repository
        .load_setting::<String>(SELECTED_SOURCE_ID_SETTING)
        .map_err(|error| error.to_string())?
        .as_deref()
        == Some(id.as_str())
    {
        let sources = repository
            .list_sources()
            .map_err(|error| error.to_string())?;
        let _ = selected_source_id(&repository, &sources)?;
    }
    drop(repository);
    state.sources.lock().retain(|item| item.id != id);
    record_log(&state, "info", "source.delete", "数据源已删除", Some(&id));
    Ok(())
}

#[tauri::command]
pub fn list_logs(
    state: State<'_, AppState>,
    page: Option<u32>,
    page_size: Option<u32>,
) -> Result<LogPage, String> {
    let page = page.unwrap_or(1).max(1);
    let page_size = page_size.unwrap_or(50).clamp(10, 100);
    let (items, total) = state
        .repository
        .lock()
        .logs_page(page, page_size)
        .map_err(|error| error.to_string())?;
    let total_pages = total.div_ceil(page_size as u64) as u32;
    Ok(LogPage {
        items,
        page,
        page_size,
        total,
        total_pages,
    })
}

#[tauri::command]
pub fn get_overview(state: State<'_, AppState>) -> Result<DashboardViewModel, String> {
    get_overview_from_state(&state)
}

fn get_overview_from_state(state: &AppState) -> Result<DashboardViewModel, String> {
    let repository = state.repository.lock();
    let source = selected_source(&repository).ok();
    let snapshots = repository
        .recent_snapshots(500)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|snapshot| {
            source
                .as_ref()
                .is_some_and(|source| snapshot.source_id == source.id)
        })
        .collect::<Vec<_>>();
    let now = Utc::now();
    let local_now = now.with_timezone(&Local);
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
        .expect("当前本地日期必须有效")
        .with_timezone(&Utc);
    let month_start = Local
        .with_ymd_and_hms(local_now.year(), local_now.month(), 1, 0, 0, 0)
        .single()
        .expect("当前本地月份必须有效")
        .with_timezone(&Utc);
    let today = period_total(
        &snapshots,
        today_start,
        now,
        crate::domain::snapshot::Period::Day,
    );
    let month = period_total(
        &snapshots,
        month_start,
        now,
        crate::domain::snapshot::Period::Month,
    );
    let balance = snapshots.iter().find_map(|s| {
        s.balance_amount
            .as_ref()
            .zip(s.balance_currency.as_ref())
            .map(|(amount, currency)| format!("{} {currency}", amount.round_dp(2)))
    });
    Ok(DashboardViewModel {
        today_tokens: today,
        month_tokens: month,
        updated_at: now,
        balance,
        source_status: match (source, snapshots.is_empty()) {
            (Some(source), true) => format!("{}：尚未同步", source.name),
            (Some(source), false) => format!("{}：最近同步成功", source.name),
            (None, _) => "尚未选择数据源".into(),
        },
    })
}

fn period_total(
    snapshots: &[UsageSnapshot],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    summary_period: crate::domain::snapshot::Period,
) -> u64 {
    if let Some(summary) = snapshots
        .iter()
        .filter(|snapshot| {
            snapshot.period == summary_period
                && snapshot.observed_at >= start
                && snapshot.observed_at <= end
        })
        .max_by_key(|snapshot| snapshot.observed_at)
    {
        return summary.effective_total_tokens().unwrap_or_default();
    }
    snapshots
        .iter()
        .filter(|snapshot| {
            snapshot.period == crate::domain::snapshot::Period::Instant
                && snapshot.observed_at >= start
                && snapshot.observed_at <= end
        })
        .filter_map(UsageSnapshot::effective_total_tokens)
        .sum()
}

#[tauri::command]
pub fn preview_dashboard(state: State<'_, AppState>) -> Result<String, String> {
    Ok(render_svg(&get_overview(state)?, 400, 360))
}

async fn sync_sources_inner(state: &AppState) -> Result<String, String> {
    let sync_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let source = match selected_source(&state.repository.lock()) {
        Ok(source) => source,
        Err(error) => return Err(error),
    };
    if let Err(error) = state.repository.lock().start_sync(&sync_id, now) {
        return Err(error.to_string());
    }
    record_log(
        &state,
        "info",
        "sync.start",
        "同步任务已开始",
        Some(&sync_id),
    );
    let mut errors = Vec::new();
    let configured = if matches!(
        source.kind.as_str(),
        "deepseek" | "openai_compatible" | "newapi"
    ) {
        match state
            .repository
            .lock()
            .secret_ref(&source.id)
            .map_err(|error| error.to_string())?
            .map(|reference| secrets::load_secret(&reference))
        {
            Some(Ok(secret)) => with_api_key(source.clone(), secret),
            Some(Err(error)) => {
                errors.push(format!("{}: {error}", source.name));
                source.clone()
            }
            None => {
                errors.push(format!("{}: 未配置 Keychain 密钥引用", source.name));
                source.clone()
            }
        }
    } else {
        source.clone()
    };
    if errors.is_empty() {
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            fetch_source(
                &configured,
                QueryRange {
                    start: Local
                        .with_ymd_and_hms(
                            now.with_timezone(&Local).year(),
                            now.with_timezone(&Local).month(),
                            1,
                            0,
                            0,
                            0,
                        )
                        .single()
                        .expect("当前本地月份必须有效")
                        .with_timezone(&Utc),
                    end: now,
                },
            ),
        )
        .await
        {
            Ok(Ok(snapshots)) => {
                for snapshot in snapshots {
                    if let Err(error) = state.repository.lock().save_snapshot(&snapshot) {
                        errors.push(format!("{}: {error}", source.name));
                    }
                }
            }
            Ok(Err(error)) => errors.push(format!("{}: {error}", source.name)),
            Err(_) => errors.push(format!("{}: 同步超时（30 秒）", source.name)),
        }
    }
    let status = if errors.is_empty() {
        "success"
    } else {
        "partial"
    };
    let error_summary = (!errors.is_empty()).then(|| errors.join(" | "));
    state
        .repository
        .lock()
        .finish_sync(&sync_id, status, error_summary.as_deref())
        .map_err(|error| error.to_string())?;
    record_log(
        &state,
        if errors.is_empty() { "info" } else { "error" },
        "sync.finish",
        if errors.is_empty() {
            "同步任务已完成"
        } else {
            "同步任务部分失败"
        },
        error_summary.as_deref(),
    );
    if errors.is_empty() {
        Ok(sync_id)
    } else {
        Err(format!("部分同步失败：{}", errors.join("；")))
    }
}

pub(crate) async fn run_sync_workflow(
    state: &AppState,
    app: Option<&AppHandle>,
) -> Result<String, String> {
    {
        let mut running = state.sync_running.lock();
        if *running {
            return Err("同步任务已在运行".into());
        }
        *running = true;
    }

    let sync_result = sync_sources_inner(state).await;
    let result = match sync_result {
        Err(error) => Err(error),
        Ok(sync_id) => match auto_connect_last_device_inner(state).await {
            Err(error) => Err(error),
            Ok(None) => Ok(format!(
                "{sync_id}：同步完成，未发现上次连接的设备，未执行推送"
            )),
            Ok(Some(connection)) if connection.epd_control_characteristic.is_none() => {
                let _ = disconnect_device_inner(state, &connection.id).await;
                Ok(format!(
                    "{sync_id}：同步完成，设备未发现 EPD 控制特征，未执行推送"
                ))
            }
            Ok(Some(connection)) => {
                let transfer = match load_device_config(state) {
                    Ok(config) => push_epd_test_image_inner(state, &connection.id, config).await,
                    Err(error) => Err(error),
                };
                let disconnect_result = disconnect_device_inner(state, &connection.id).await;
                match transfer {
                    Ok(result) if result.connected => {
                        if let Err(error) = disconnect_result {
                            record_log(
                                state,
                                "error",
                                "device.disconnect",
                                "同步后断开设备失败",
                                Some(&error),
                            );
                        }
                        Ok(format!("{sync_id}：同步并推送完成"))
                    }
                    Ok(_) => Err("同步完成，但设备在推送过程中断开".into()),
                    Err(error) => Err(error),
                }
            }
        },
    };
    *state.sync_running.lock() = false;
    if let Some(app) = app {
        let _ = app.emit("sync-completed", serde_json::json!({
            "success": result.is_ok(),
            "message": result.as_ref().map(String::as_str).unwrap_or_else(|error| error.as_str()),
        }));
    }
    result
}

#[tauri::command]
pub async fn sync_all(state: State<'_, AppState>) -> Result<String, String> {
    run_sync_workflow(&state, None).await
}

#[tauri::command]
pub async fn sync_and_push(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    run_sync_workflow(&state, Some(&app)).await
}

async fn shared_ble_adapter(state: &AppState) -> Result<btleplug::platform::Adapter, String> {
    let mut adapter = state.ble_adapter.lock().await;
    if adapter.is_none() {
        *adapter = Some(
            create_adapter()
                .await
                .map_err(|error| format!("蓝牙适配器初始化失败：{error}"))?,
        );
    }
    Ok(adapter.as_ref().expect("BLE adapter initialized").clone())
}

async fn connect_and_cache_device(
    state: &AppState,
    device_id: &str,
    expected_name: Option<&str>,
) -> Result<BleConnectionInfo, String> {
    let config = load_device_config(state)?;
    let adapter = shared_ble_adapter(state).await?;
    let (info, peripheral) = connect_nrf_epd(&adapter, device_id, expected_name, config.driver_id)
        .await
        .map_err(|error| format!("蓝牙连接失败：{error}"))?;
    state
        .ble_devices
        .lock()
        .insert(info.id.clone(), info.name.clone());
    state
        .ble_peripherals
        .lock()
        .await
        .insert(info.id.clone(), peripheral);
    Ok(info)
}

fn remember_last_epd_device(state: &AppState, name: &str) {
    if let Err(error) = state
        .repository
        .lock()
        .save_setting(LAST_EPD_DEVICE_NAME_SETTING, &name)
    {
        record_log(
            state,
            "error",
            "device.remember",
            "无法保存上次连接的电子墨水屏设备",
            Some(&error.to_string()),
        );
    }
}

fn load_device_config(state: &AppState) -> Result<DeviceConfig, String> {
    let repository = state.repository.lock();
    let config = repository
        .load_setting("device_config")
        .map_err(|error| error.to_string())?
        .unwrap_or_else(DeviceConfig::monochrome_400x300);
    let migrated = config.clone().migrate_default_driver();
    if migrated.driver_id != config.driver_id {
        repository
            .save_setting("device_config", &migrated)
            .map_err(|error| error.to_string())?;
    }
    Ok(migrated)
}

#[tauri::command]
pub async fn scan_devices(state: State<'_, AppState>) -> Result<Vec<BleDevice>, String> {
    let adapter = shared_ble_adapter(&state).await?;
    let devices = scan_nrf_epd(&adapter, std::time::Duration::from_secs(4))
        .await
        .map_err(|error| format!("蓝牙扫描失败：{error}"))?;
    *state.ble_devices.lock() = devices
        .iter()
        .map(|device| (device.id.clone(), device.name.clone()))
        .collect();
    Ok(devices)
}

#[tauri::command]
pub async fn connect_device(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<BleConnectionInfo, String> {
    let expected_name = state.ble_devices.lock().get(&device_id).cloned();
    let info = connect_and_cache_device(&state, &device_id, expected_name.as_deref()).await?;
    remember_last_epd_device(&state, &info.name);
    record_log(
        &state,
        "info",
        "device.connect",
        "电子墨水屏设备已连接",
        Some(&info.id),
    );
    Ok(info)
}

async fn auto_connect_last_device_inner(
    state: &AppState,
) -> Result<Option<BleConnectionInfo>, String> {
    let last_name = state
        .repository
        .lock()
        .load_setting::<String>(LAST_EPD_DEVICE_NAME_SETTING)
        .map_err(|error| error.to_string())?;
    let Some(last_name) = last_name.filter(|name| !name.is_empty()) else {
        return Ok(None);
    };

    let adapter = shared_ble_adapter(&state).await?;
    let devices = match scan_nrf_epd(&adapter, std::time::Duration::from_secs(4)).await {
        Ok(devices) => devices,
        Err(error) => {
            record_log(
                &state,
                "error",
                "device.auto_connect",
                "自动连接上次设备时扫描失败",
                Some(&error.to_string()),
            );
            return Err(format!("蓝牙扫描失败：{error}"));
        }
    };
    state.ble_devices.lock().extend(
        devices
            .iter()
            .map(|device| (device.id.clone(), device.name.clone())),
    );
    let Some(device) = devices.into_iter().find(|device| device.name == last_name) else {
        record_log(
            &state,
            "info",
            "device.auto_connect",
            "未发现上次连接的电子墨水屏设备，已跳过自动连接",
            Some(&last_name),
        );
        return Ok(None);
    };

    match connect_and_cache_device(&state, &device.id, Some(&device.name)).await {
        Ok(info) => {
            record_log(
                &state,
                "info",
                "device.auto_connect",
                "已自动连接上次使用的电子墨水屏设备",
                Some(&info.name),
            );
            Ok(Some(info))
        }
        Err(error) => {
            record_log(
                &state,
                "error",
                "device.auto_connect",
                "自动连接上次电子墨水屏设备失败",
                Some(&format!("device={last_name}; error={error}")),
            );
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn auto_connect_last_device(
    state: State<'_, AppState>,
) -> Result<Option<BleConnectionInfo>, String> {
    auto_connect_last_device_inner(&state).await
}

async fn disconnect_device_inner(state: &AppState, device_id: &str) -> Result<bool, String> {
    let peripheral = state.ble_peripherals.lock().await.get(device_id).cloned();
    let Some(peripheral) = peripheral else {
        return Ok(false);
    };
    if !peripheral
        .is_connected()
        .await
        .map_err(|error| format!("读取蓝牙连接状态失败：{error}"))?
    {
        state.ble_peripherals.lock().await.remove(device_id);
        return Ok(false);
    }
    peripheral
        .disconnect()
        .await
        .map_err(|error| format!("蓝牙断开失败：{error}"))?;
    state.ble_peripherals.lock().await.remove(device_id);
    record_log(
        &state,
        "info",
        "device.disconnect",
        "电子墨水屏设备已断开",
        Some(device_id),
    );
    Ok(true)
}

#[tauri::command]
pub async fn disconnect_device(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<bool, String> {
    disconnect_device_inner(&state, &device_id).await
}

#[tauri::command]
pub fn get_device_config(state: State<'_, AppState>) -> Result<DeviceConfig, String> {
    load_device_config(&state)
}

#[tauri::command]
pub fn save_device_config(
    state: State<'_, AppState>,
    config: DeviceConfig,
) -> Result<DeviceConfig, String> {
    if config.width == 0
        || config.height == 0
        || config.color_layers == 0
        || config.color_layers > 4
    {
        return Err("屏幕宽高必须大于零，颜色层数必须为 1–4".into());
    }
    if config.block_size == 0 || config.block_size + 8 > config.mtu {
        return Err("块大小必须小于 MTU（含协议头）".into());
    }
    state
        .repository
        .lock()
        .save_setting("device_config", &config)
        .map_err(|error| error.to_string())?;
    record_log(&state, "info", "device.config", "设备配置已保存", None);
    Ok(config)
}

async fn push_epd_test_image_inner(
    state: &AppState,
    device_id: &str,
    config: DeviceConfig,
) -> Result<TransferResult, String> {
    if !(1..=2).contains(&config.color_layers) {
        return Err("当前统计卡片传输仅支持单层或双层 EPD".into());
    }
    let expected_name = state.ble_devices.lock().get(device_id).cloned();
    let model = get_overview_from_state(state)?;
    let image_layers = if config.color_layers == 2 {
        let (black, red) = render_tricolor_bitmaps(&model, config.width, config.height)?;
        vec![black, red]
    } else {
        vec![render_mono_bitmap(&model, config.width, config.height)?]
    };
    let cached_peripheral = state.ble_peripherals.lock().await.get(device_id).cloned();
    let peripheral = if let Some(peripheral) = cached_peripheral {
        peripheral
    } else {
        let adapter = shared_ble_adapter(&state).await?;
        let (info, peripheral) = connect_nrf_epd(
            &adapter,
            device_id,
            expected_name.as_deref(),
            config.driver_id,
        )
        .await
        .map_err(|error| format!("蓝牙连接失败：{error}"))?;
        state.ble_devices.lock().insert(info.id.clone(), info.name);
        state
            .ble_peripherals
            .lock()
            .await
            .insert(device_id.to_string(), peripheral.clone());
        peripheral
    };
    let result =
        transfer_epd_image_on_peripheral(device_id, &image_layers, &config, &peripheral).await;
    if !peripheral.is_connected().await.unwrap_or(false) {
        state.ble_peripherals.lock().await.remove(device_id);
    }
    match result {
        Ok(result) => {
            record_log(
                &state,
                "info",
                "device.transfer",
                "仪表盘位图传输完成",
                Some(&format!(
                    "device={device_id}; firmware={:?}; driver_id=0x{:02X}; mode={:?}; mtu={}; block_size={}; blocks={}; retries={}",
                    result.firmware_version,
                    result.driver_id,
                    result.transfer_mode,
                    result.mtu,
                    result.block_size,
                    result.blocks_sent,
                    result.retry_rounds,
                )),
            );
            Ok(result)
        }
        Err(error) => {
            record_log(
                &state,
                "error",
                "device.transfer",
                "仪表盘位图传输失败",
                Some(&format!("device={device_id}; error={error}")),
            );
            Err(format!("EPD 传输失败：{error}"))
        }
    }
}

#[tauri::command]
pub async fn push_epd_test_image(
    state: State<'_, AppState>,
    device_id: String,
    config: DeviceConfig,
) -> Result<TransferResult, String> {
    push_epd_test_image_inner(&state, &device_id, config).await
}

#[tauri::command]
pub fn prepare_epd_transfer(config: DeviceConfig) -> Result<usize, String> {
    // A blank packed bitmap is a valid transport smoke test; production transfer owns the BLE characteristic.
    let packets = chunk_image(
        &vec![0xFF; config.expected_layer_bytes()],
        &config,
        BW_LAYER,
    )?;
    Ok(packets.len() * config.color_layers as usize)
}
#[tauri::command]
pub fn get_schedule(state: State<'_, AppState>) -> Result<ScheduleConfig, String> {
    Ok(state
        .repository
        .lock()
        .load_setting("schedule_config")
        .map_err(|error| error.to_string())?
        .unwrap_or_default())
}

#[tauri::command]
pub fn save_schedule(
    state: State<'_, AppState>,
    schedule: ScheduleConfig,
) -> Result<ScheduleConfig, String> {
    if schedule.interval_minutes == 0 || schedule.interval_minutes > 24 * 60 {
        return Err("计划间隔必须在 1 到 1440 分钟之间".into());
    }
    state
        .repository
        .lock()
        .save_setting("schedule_config", &schedule)
        .map_err(|error| error.to_string())?;
    record_log(&state, "info", "schedule.save", "计划任务已保存", None);
    Ok(schedule)
}

#[tauri::command]
pub fn schedule_is_due(state: State<'_, AppState>) -> Result<bool, String> {
    let repository = state.repository.lock();
    let schedule: ScheduleConfig = repository
        .load_setting("schedule_config")
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let last_run = repository
        .load_setting("schedule_last_run")
        .map_err(|error| error.to_string())?;
    Ok(is_due(last_run, Utc::now(), &schedule))
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    Ok(state
        .repository
        .lock()
        .load_setting("app_settings")
        .map_err(|error| error.to_string())?
        .unwrap_or_default())
}
#[tauri::command]
pub fn save_settings(
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    if settings.refresh_minutes == 0 || settings.refresh_minutes > 24 * 60 {
        return Err("刷新间隔必须在 1 到 1440 分钟之间".into());
    }
    state
        .repository
        .lock()
        .save_setting("app_settings", &settings)
        .map_err(|error| error.to_string())?;
    record_log(&state, "info", "settings.save", "应用设置已保存", None);
    Ok(settings)
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|error| error.to_string())?;
    manager.is_enabled().map_err(|error| error.to_string())
}
