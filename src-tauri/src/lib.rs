mod commands;
mod export;
mod ics;
mod recorder;
mod scheduler;
mod state;
mod sync;
mod tray;
mod updater;

use state::AppState;
use std::sync::Arc;
use tauri::{Manager, RunEvent, WindowEvent};

pub fn run() {
    init_logging();
    match milkrice_core::paths::migration_outcome() {
        milkrice_core::paths::Migration::Migrated => log::info!("migrated data folder from Margin to Milkrice"),
        milkrice_core::paths::Migration::Failed(e) => log::error!("could not migrate the old Margin data folder (left untouched): {e}"),
        milkrice_core::paths::Migration::NotNeeded => {}
    }
    // Use CoreML/Metal acceleration where the engine supports it.
    transcribe_rs_accel();

    let store = Arc::new(milkrice_core::store::Store::open_default().expect("open database"));
    // Notes opened from an upcoming event but never used don't pile up in the list.
    match store.purge_empty_drafts(None) {
        Ok(n) if n > 0 => eprintln!("removed {n} empty draft notes"),
        _ => {}
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| tray::show_main(app)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::Builder::new().macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent).build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::new(store))
        .setup(|app| {
            let handle = app.handle().clone();
            tray::init(&handle)?;
            migrate_login_item(&handle);
            if let Ok(dir) = std::env::var("MILKRICE_SELFTEST") {
                recorder::selftest(handle.clone(), dir.into());
                return Ok(());
            }
            if let Some(secs) = std::env::var("MILKRICE_TEST_EVENT").ok().and_then(|v| v.parse().ok()) {
                recorder::calendar_selftest(handle.clone(), secs);
            }
            // Opt-in recordings: enforce the retention window.
            let days = handle.state::<AppState>().settings().audio_retention_days;
            std::thread::spawn(move || {
                let n = milkrice_core::audio_rec::enforce_retention(days);
                if n > 0 {
                    log::info!("deleted {n} recordings older than {days} days");
                }
            });
            scheduler::spawn(handle.clone());
            sync::spawn(handle.clone());
            // If the app was moved, ~/.local/bin/milkrice may point at the old location: fix it.
            std::thread::spawn(|| {
                let home = dirs::home_dir().unwrap_or_default();
                if milkrice_core::agents_install::repair_cli_link(&home, commands::cli_path().as_deref()) {
                    log::info!("repointed ~/.local/bin/milkrice at this app");
                }
            });
            // Look for a newer release a few seconds after launch (silent unless one exists).
            {
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(8));
                    updater::check(&h, false);
                });
            }
            // Warm the speech model so the first recording starts instantly.
            std::thread::spawn(move || {
                let st = handle.state::<AppState>();
                if st.settings().model_id.is_some() {
                    if let Err(e) = recorder::ensure_engine(&st) {
                        log::warn!("model warm-up failed: {e}");
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::permissions,
            commands::grant_permission,
            commands::open_permission_settings,
            commands::restart_app,
            commands::input_devices,
            commands::audio_devices,
            commands::set_mic,
            commands::mic_test,
            commands::probe_mics,
            commands::list_models,
            commands::download_model,
            commands::cancel_download,
            commands::delete_model,
            commands::providers,
            commands::upcoming_events,
            commands::hidden_events,
            commands::hide_event,
            commands::unhide_event,
            commands::current_prompt,
            commands::add_calendar_feed,
            commands::remove_calendar_feed,
            commands::calendar_sources,
            commands::dismiss_prompt,
            commands::start_event,
            commands::list_meetings,
            commands::get_meeting,
            commands::list_folders,
            commands::discard_if_empty,
            commands::create_folder,
            commands::rename_folder,
            commands::delete_folder,
            commands::set_folder_color,
            commands::set_meeting_folder,
            commands::set_folder_series,
            commands::dismiss_folder_suggestion,
            commands::create_meeting,
            commands::update_meeting,
            commands::update_segment,
            commands::delete_meeting,
            commands::meeting_audio,
            commands::delete_meeting_audio,
            commands::audio_storage,
            commands::rename_speaker,
            commands::start_recording,
            commands::stop_recording,
            commands::pause_recording,
            commands::turn_off_auto_record,
            commands::autostart_get,
            commands::autostart_set,
            commands::rec_state,
            commands::enhance,
            commands::identify_speakers,
            commands::ask,
            commands::ask_all,
            commands::export_markdown,
            commands::obsidian_vaults,
            commands::obsidian_export_all,
            commands::open_in_obsidian,
            commands::agents_status,
            commands::agent_connect,
            commands::sync_status,
            commands::sync_setup,
            commands::sync_pair_start,
            commands::sync_pair_poll,
            commands::sync_pair_accept,
            commands::sync_pair_cancel,
            commands::sync_now,
            commands::sync_leave,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Milkrice")
        .run(|app, event| match event {
            // Closing the window keeps Milkrice in the menu bar so it can catch meetings.
            RunEvent::WindowEvent { label, event: WindowEvent::CloseRequested { api, .. }, .. } if label == "main" => {
                api.prevent_close();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => tray::show_main(app),
            RunEvent::ExitRequested { .. } => {
                recorder::stop_and_wait(app, std::time::Duration::from_secs(20));
            }
            _ => {}
        });
}

fn transcribe_rs_accel() {
    // Plain CPU int8 is the fast path on Apple Silicon; ONNX engines don't benefit from CoreML
    // (and some, like Canary, fail on it). Whisper still uses Metal via whisper.cpp.
    milkrice_core::transcribe_rs::set_ort_accelerator(milkrice_core::transcribe_rs::OrtAccelerator::CpuOnly);
}

/// Before the rename, "Open at login" wrote ~/Library/LaunchAgents/Margin.plist pointing at
/// Margin.app. If that is present, drop it and register the Milkrice login item instead.
fn migrate_login_item(app: &tauri::AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let Some(dir) = dirs::home_dir().map(|h| h.join("Library/LaunchAgents")) else { return };
    let old = dir.join("Margin.plist");
    let Ok(text) = std::fs::read_to_string(&old) else { return };
    if !text.contains("Margin.app") {
        return; // not ours; leave it alone
    }
    match std::fs::remove_file(&old) {
        Ok(()) => {
            if let Err(e) = app.autolaunch().enable() {
                log::warn!("could not enable Milkrice login item after migration: {e}");
            } else {
                log::info!("migrated login item from Margin to Milkrice");
            }
        }
        Err(e) => log::warn!("could not remove old Margin login item: {e}"),
    }
}

/// Log to ~/Library/Application Support/Milkrice/milkrice.log (rotated at 5 MB) and stderr.
fn init_logging() {
    use std::io::Write;
    let path = milkrice_core::paths::data_dir().join("milkrice.log");
    if std::fs::metadata(&path).map(|m| m.len() > 5 << 20).unwrap_or(false) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
    let file = std::sync::Mutex::new(file);
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,ort=warn,whisper_rs=warn"))
        .format(move |buf, rec| {
            let line = format!("{} {:<5} {}: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), rec.level(), rec.target(), rec.args());
            if let Ok(mut f) = file.lock() {
                if let Some(f) = f.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
            }
            writeln!(buf, "{line}")
        })
        .init();
}
