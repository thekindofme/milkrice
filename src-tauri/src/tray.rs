//! Menu bar presence: quick start/stop and meeting prompts without opening the window.
use crate::state::AppState;
use crate::{recorder, scheduler};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use chrono::Utc;
use milkrice_core::CalEvent;
use tauri::{AppHandle, Emitter, Manager};

const TRAY_ID: &str = "milkrice-tray";
const ICON: &[u8] = include_bytes!("../icons/tray.png");
const ICON_REC: &[u8] = include_bytes!("../icons/tray-rec.png");

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// The next calendar meeting worth a "Start notes" item: in progress or starting within 12 hours,
/// not already prompted for, and (if the setting is on) one with people or a call link.
fn next_event(state: &AppState) -> Option<CalEvent> {
    let now = Utc::now();
    let s = state.settings();
    let only_real = s.only_real_meetings;
    let prompted = state.prompt.lock().as_ref().and_then(|p| p.event.as_ref().map(|e| e.id.clone()));
    state
        .events
        .lock()
        .iter()
        .filter(|e| !e.all_day && e.end > now && e.start < now + chrono::Duration::hours(12))
        .filter(|e| !only_real || scheduler::is_real_meeting(e))
        .filter(|e| !s.is_hidden(e))
        .filter(|e| prompted.as_deref() != Some(e.id.as_str()))
        .min_by_key(|e| e.start)
        .cloned()
}

fn next_label(ev: &CalEvent) -> String {
    let title = if ev.title.trim().is_empty() { "Untitled meeting" } else { ev.title.trim() };
    if ev.start <= Utc::now() {
        format!("Now: {title} · Start notes")
    } else {
        format!("Next: {title} at {} · Start notes", ev.start.with_timezone(&chrono::Local).format("%H:%M"))
    }
}

/// Switch auto-record off ("notify me" instead) and tell the window.
pub fn turn_off_auto_record(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    let mut s = state.settings();
    if s.calendar_mode == "auto" {
        s.calendar_mode = "notify".into();
        state.save_settings(s)?;
    }
    let _ = app.emit("settings-changed", ());
    refresh(app);
    Ok(())
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let state = app.state::<AppState>();
    let recording = state.recording.lock().is_some();
    let prompt = state.prompt.lock().clone();
    let auto = state.settings().calendar_mode == "auto";
    let menu = Menu::new(app)?;
    if recording {
        menu.append(&MenuItem::with_id(app, "stop", "Stop recording", true, None::<&str>)?)?;
    } else {
        if let Some(p) = &prompt {
            menu.append(&MenuItem::with_id(app, "prompt", format!("Start notes: {}", p.title), true, None::<&str>)?)?;
        }
        if let Some(ev) = next_event(&state) {
            menu.append(&MenuItem::with_id(app, format!("next:{}", ev.id), next_label(&ev), true, None::<&str>)?)?;
        }
        menu.append(&MenuItem::with_id(app, "quick", "Start a quick note", true, None::<&str>)?)?;
    }
    if auto {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        menu.append(&MenuItem::with_id(app, "autorec_off", "Auto-record: on — Turn off", true, None::<&str>)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "open", "Open Milkrice", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "update", "Check for updates…", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit Milkrice", true, None::<&str>)?)?;
    Ok(menu)
}

/// While recording, show a running "REC 12:34" beside the red icon so it's hard to miss.
fn start_clock(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let mut last = String::new();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let state = app.state::<AppState>();
            let now = state
                .recording
                .lock()
                .as_ref()
                .map(|r| (r.offset_ms + r.session.elapsed_ms(), r.session.is_paused()));
            let title = match now {
                Some((ms, paused)) => {
                    let s = ms / 1000;
                    let t = if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) };
                    format!("{} {t}", if paused { "Paused" } else { "REC" })
                }
                None => String::new(),
            };
            if title != last {
                if let Some(tray) = app.tray_by_id(TRAY_ID) {
                    let _ = tray.set_title(if title.is_empty() { None } else { Some(title.as_str()) });
                    let _ = tray.set_tooltip(Some(if title.is_empty() { "Milkrice".to_string() } else { format!("Milkrice is recording · {title}") }));
                }
                last = title;
            }
        }
    });
}

pub fn init(app: &AppHandle) -> tauri::Result<TrayIcon> {
    start_clock(app);
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(ICON)?)
        .icon_as_template(true)
        .tooltip("Milkrice")
        .menu(&build_menu(app)?)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, ev| {
            let app = app.clone();
            match ev.id.as_ref() {
                "open" => show_main(&app),
                "update" => crate::updater::check(&app, true),
                "quit" => {
                    recorder::stop_and_wait(&app, std::time::Duration::from_secs(20));
                    app.exit(0)
                }
                "stop" => {
                    let _ = recorder::stop(&app, true);
                }
                "autorec_off" => {
                    if let Err(e) = turn_off_auto_record(&app) {
                        recorder::notify(&app, "Couldn't turn off auto-record", &e.to_string());
                    } else {
                        recorder::notify(&app, "Auto-record is off", "Milkrice will notify you when meetings start instead.");
                    }
                }
                id if id.starts_with("next:") => {
                    let ev_id = id["next:".len()..].to_string();
                    std::thread::spawn(move || {
                        let ev = app.state::<AppState>().events.lock().iter().find(|e| e.id == ev_id).cloned();
                        if let Some(ev) = ev {
                            if let Err(e) = scheduler::start_for_event(&app, &ev) {
                                recorder::notify(&app, "Couldn't start", &e.to_string());
                            }
                        }
                        show_main(&app);
                    });
                }
                "quick" => {
                    std::thread::spawn(move || {
                        let state = app.state::<AppState>();
                        let r = recorder::new_meeting(&state, "", None).and_then(|m| {
                            recorder::start(&app, &m.id, None)?;
                            Ok(m.id)
                        });
                        match r {
                            Ok(id) => {
                                let _ = app.emit("open-meeting", serde_json::json!({"id": id}));
                            }
                            Err(e) => recorder::notify(&app, "Couldn't start", &e.to_string()),
                        }
                        show_main(&app);
                    });
                }
                "prompt" => {
                    std::thread::spawn(move || {
                        let p = app.state::<AppState>().prompt.lock().clone();
                        if let Some(p) = p {
                            let r = match &p.event {
                                Some(ev) => scheduler::start_for_event(&app, ev),
                                None => {
                                    let state = app.state::<AppState>();
                                    recorder::new_meeting(&state, &p.title, None).and_then(|m| {
                                        recorder::start(&app, &m.id, None)?;
                                        let _ = app.emit("open-meeting", serde_json::json!({"id": m.id}));
                                        Ok(())
                                    })
                                }
                            };
                            if let Err(e) = r {
                                recorder::notify(&app, "Couldn't start", &e.to_string());
                            }
                        }
                        show_main(&app);
                    });
                }
                _ => {}
            }
        })
        .build(app)
}

/// Rebuild the menu and swap the icon to reflect recording state.
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let state = app.state::<AppState>();
    let recording = state.recording.lock().is_some();
    // Only rebuild when something visible changed, so an open menu isn't yanked away.
    static LAST: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);
    let next = next_event(&state).map(|e| format!("{}|{}", e.id, next_label(&e)));
    let key = format!(
        "{recording}|{:?}|{}|{next:?}",
        state.prompt.lock().as_ref().map(|p| p.title.clone()),
        state.settings().calendar_mode
    );
    {
        let mut last = LAST.lock();
        if last.as_deref() == Some(key.as_str()) {
            return;
        }
        *last = Some(key);
    }
    if let Ok(menu) = build_menu(app) {
        let _ = tray.set_menu(Some(menu));
    }
    if let Ok(img) = Image::from_bytes(if recording { ICON_REC } else { ICON }) {
        let _ = tray.set_icon(Some(img));
        let _ = tray.set_icon_as_template(!recording);
    }
}
