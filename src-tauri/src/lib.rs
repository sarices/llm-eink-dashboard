mod ble;
mod commands;
mod domain;
mod epd;
mod providers;
mod render;
mod scheduler;
mod secrets;
mod state;
mod storage;
use chrono::{DateTime, Utc};
use state::AppState;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, RunEvent,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::new();
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(state)
        .setup(|app| {
            let open = MenuItem::with_id(app, "open", "打开面板", true, None::<&str>)?;
            let sync = MenuItem::with_id(app, "sync", "立即同步", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &sync, &quit])?;
            let tray_icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;
            TrayIconBuilder::with_id("main-tray")
                .icon(tray_icon)
                .icon_as_template(false)
                .menu(&menu)
                .tooltip("LLM E-Ink Dashboard")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "sync" => {
                        let _ = app.emit("tray-action", "sync");
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            let scheduler_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    let state = scheduler_app.state::<AppState>();
                    let schedule = match state
                        .repository
                        .lock()
                        .load_setting::<crate::scheduler::ScheduleConfig>("schedule_config")
                    {
                        Ok(Some(schedule)) => schedule,
                        Ok(None) => crate::scheduler::ScheduleConfig::default(),
                        Err(error) => {
                            let _ = state.repository.lock().log_event(
                                "error",
                                "schedule.load",
                                "读取计划任务失败",
                                Some(&error.to_string()),
                            );
                            continue;
                        }
                    };
                    let last_run = match state
                        .repository
                        .lock()
                        .load_setting::<DateTime<Utc>>("schedule_last_run")
                    {
                        Ok(value) => value,
                        Err(error) => {
                            let _ = state.repository.lock().log_event(
                                "error",
                                "schedule.load",
                                "读取计划任务时间失败",
                                Some(&error.to_string()),
                            );
                            None
                        }
                    };
                    if !crate::scheduler::is_due(last_run, Utc::now(), &schedule) {
                        continue;
                    }
                    let attempts = schedule.retry_count.saturating_add(1);
                    let mut result = Err("计划同步未执行".to_string());
                    for _ in 0..attempts {
                        result =
                            crate::commands::run_sync_workflow(&state, Some(&scheduler_app)).await;
                        if result.is_ok() {
                            break;
                        }
                    }
                    if result.is_ok() {
                        let _ = state
                            .repository
                            .lock()
                            .save_setting("schedule_last_run", &Utc::now());
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_sources,
            commands::save_source,
            commands::save_source_config,
            commands::save_source_secret,
            commands::select_source,
            commands::test_source,
            commands::test_source_config,
            commands::list_source_configs,
            commands::get_overview,
            commands::preview_dashboard,
            commands::sync_all,
            commands::sync_and_push,
            commands::delete_source,
            commands::list_logs,
            commands::scan_devices,
            commands::connect_device,
            commands::auto_connect_last_device,
            commands::disconnect_device,
            commands::push_epd_test_image,
            commands::get_settings,
            commands::save_settings,
            commands::get_device_config,
            commands::save_device_config,
            commands::prepare_epd_transfer,
            commands::get_schedule,
            commands::save_schedule,
            commands::schedule_is_due,
            commands::set_autostart
        ])
        .build(tauri::generate_context!())
        .expect("failed to build dashboard")
        .run(|app, event| match event {
            RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label == "main" => {
                api.prevent_close();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            _ => {}
        })
}

#[cfg(test)]
mod integration_tests {
    use super::{
        epd::{chunk_image, DeviceConfig, BW_LAYER},
        render::{render_svg, DashboardViewModel},
        storage::Repository,
    };
    use crate::domain::snapshot::{DataConfidence, Period, UsageSnapshot};
    use chrono::Utc;
    use tempfile::NamedTempFile;

    #[test]
    fn snapshot_storage_render_and_epd_packets_form_a_valid_pipeline() {
        let database = NamedTempFile::new().unwrap();
        let repository = Repository::open(database.path().to_str().unwrap()).unwrap();
        let observed_at = Utc::now();
        let snapshot = UsageSnapshot {
            source_id: "fixture".into(),
            provider: "fixture-provider".into(),
            account_id: "personal".into(),
            model: "fixture-model".into(),
            observed_at,
            period: Period::Day,
            input_tokens: Some(1200),
            output_tokens: Some(3400),
            cached_tokens: Some(0),
            total_tokens: Some(4600),
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
        let persisted = repository.recent_snapshots(1).unwrap();
        assert_eq!(persisted[0].effective_total_tokens(), Some(4600));
        let preview = render_svg(
            &DashboardViewModel {
                today_tokens: persisted[0].effective_total_tokens().unwrap(),
                month_tokens: persisted[0].effective_total_tokens().unwrap(),
                updated_at: observed_at,
                balance: None,
                source_status: "最近同步成功".into(),
            },
            16,
            8,
        );
        assert!(preview.contains("4.6") || preview.contains("4600"));
        let mut device = DeviceConfig::monochrome_400x300();
        device.width = 16;
        device.height = 8;
        device.mtu = 32;
        device.block_size = 8;
        let packets = chunk_image(
            &vec![0xFF; device.expected_layer_bytes()],
            &device,
            BW_LAYER,
        )
        .unwrap();
        assert_eq!(packets.len(), 2);
        assert!(packets.iter().all(|packet| packet[0] == 0x31));
    }
}
