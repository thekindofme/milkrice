//! Commands invoked from the UI. Long work runs on blocking threads so the UI never stalls.
use crate::state::{AppState, HiddenEvent, Settings};
use crate::{recorder, scheduler};

use milkrice_core::store::{ChatMessage, Folder, Meeting, MeetingSummary};
use milkrice_core::{agents_install, audio, catalog, llm, models, CalEvent, Segment};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> R<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(e)?.map_err(e)
}

// ------------------------------------------------------------------ settings & status

/// Settings as seen by the UI: secret calendar addresses are reduced to their host.
fn redacted(mut s: Settings) -> Settings {
    for f in s.calendar_feeds.iter_mut() {
        f.url = crate::ics::host(&f.url);
    }
    s
}

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Settings {
    redacted(state.settings())
}

#[tauri::command]
pub fn save_settings(app: AppHandle, state: State<AppState>, mut settings: Settings) -> R<Settings> {
    let old = state.settings();
    // Feeds (with their secret URLs) are managed only by add/remove_calendar_feed.
    settings.calendar_feeds = old.calendar_feeds.clone();
    // Hidden meetings are managed only by hide_event / unhide_event.
    settings.hidden_events = old.hidden_events.clone();
    state.save_settings(settings.clone()).map_err(e)?;
    if old.model_id != settings.model_id || old.language != settings.language {
        // Reload lazily on next recording; warm it up now in the background.
        *state.engine.lock() = None;
        let app2 = app.clone();
        std::thread::spawn(move || {
            let st = app2.state::<AppState>();
            let r = recorder::ensure_engine(&st);
            let _ = app2.emit("engine", json!({"ready": r.is_ok(), "error": r.err().map(|x| x.to_string())}));
        });
    }
    Ok(redacted(settings))
}

/// Status of every permission Milkrice uses, for the guided permissions panel.
#[tauri::command]
pub fn permissions(app: AppHandle) -> Value {
    use milkrice_core::permissions::{status, Permission};
    use tauri_plugin_notification::{NotificationExt, PermissionState};
    let notif = match app.notification().permission_state() {
        Ok(PermissionState::Granted) => "granted",
        Ok(PermissionState::Denied) => "denied",
        _ => "not_determined",
    };
    json!({
        "microphone": status(Permission::Microphone),
        "system_audio": status(Permission::SystemAudio),
        "calendar": status(Permission::Calendar),
        "notifications": notif,
    })
}

fn bundle_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().map(|e| e == "app").unwrap_or(false)).map(|p| p.to_path_buf())
}

/// Ask for a permission the easiest way macOS allows: the native prompt if it can still be
/// shown, otherwise open the exact System Settings pane and reveal Milkrice.app in Finder so it
/// can be dragged into the list.
#[tauri::command]
pub async fn grant_permission(app: AppHandle, kind: String) -> R<Value> {
    use milkrice_core::permissions::{open_settings, request, status, PermStatus, Permission};
    if kind == "notifications" {
        use tauri_plugin_notification::NotificationExt;
        let st = app.notification().request_permission().map_err(e)?;
        return Ok(json!({"status": format!("{st:?}").to_lowercase(), "opened_settings": false}));
    }
    let p = match kind.as_str() {
        "microphone" => Permission::Microphone,
        "system_audio" => Permission::SystemAudio,
        "calendar" => Permission::Calendar,
        _ => return Err("unknown permission".into()),
    };
    let before = status(p);
    let after = if before == PermStatus::NotDetermined {
        blocking(move || Ok(request(p))).await?
    } else {
        before
    };
    let mut opened = false;
    if after != PermStatus::Granted {
        // The prompt can't be shown again (or never shows for screen capture): go to Settings.
        let _ = open_settings(p);
        // Only the Screen & System Audio list accepts "+"/drag-in; the others list an app
        // automatically once it has asked.
        if p == Permission::SystemAudio {
            if let Some(b) = bundle_path() {
                let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(b).spawn();
            }
        }
        opened = true;
    }
    if p == Permission::Calendar && after == PermStatus::Granted {
        let a = app.clone();
        std::thread::spawn(move || scheduler::refresh_events(&a));
    }
    Ok(json!({"status": after, "opened_settings": opened}))
}

#[tauri::command]
pub fn open_permission_settings(kind: String) -> R<()> {
    use milkrice_core::permissions::{open_settings, Permission};
    let p = match kind.as_str() {
        "microphone" => Permission::Microphone,
        "system_audio" => Permission::SystemAudio,
        _ => Permission::Calendar,
    };
    open_settings(p).map_err(e)
}

/// Screen & System Audio Recording only takes effect after a relaunch.
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    recorder::stop_and_wait(&app, std::time::Duration::from_secs(20));
    app.restart();
}

#[tauri::command]
pub fn input_devices() -> Vec<String> {
    audio::list_input_devices()
}

/// Inputs, the system defaults, and what's selected — for the recorder's device chip.
#[tauri::command]
pub async fn audio_devices(app: AppHandle) -> R<Value> {
    let selected = app.state::<AppState>().settings().mic_device;
    let capture_system = app.state::<AppState>().settings().capture_system;
    blocking(move || {
        let (default_input, default_output) = audio::default_devices();
        let inputs = audio::list_input_devices();
        let active = selected.clone().filter(|s| inputs.contains(s)).or(default_input.clone());
        Ok(json!({
            "inputs": inputs, "default_input": default_input, "default_output": default_output,
            "selected": selected, "active": active, "capture_system": capture_system,
        }))
    })
    .await
}

/// Listen briefly on every mic and report what each hears (finds the one that hears you).
#[tauri::command]
pub async fn probe_mics(app: AppHandle) -> R<Value> {
    // The picker's preview stream would compete for the device; pause it.
    if let Some(h) = app.state::<AppState>().mic_test.lock().take() {
        h.stop();
    }
    let probes = blocking(|| Ok(audio::probe_inputs(1500))).await?;
    Ok(json!(probes))
}

/// Choose a microphone (None = follow the system default). Hot-swaps if recording.
#[tauri::command]
pub async fn set_mic(app: AppHandle, device: Option<String>) -> R<()> {
    let state = app.state::<AppState>();
    let mut s = state.settings();
    s.mic_device = device.filter(|d| !d.is_empty());
    state.save_settings(s).map_err(e)?;
    let recording = state.recording.lock().is_some();
    if recording {
        let a = app.clone();
        blocking(move || recorder::swap_input(&a)).await?;
    }
    Ok(())
}

/// Live input level for the picker, emitted as "mic-test" events (~12/s) until stopped.
#[tauri::command]
pub fn mic_test(app: AppHandle, device: Option<String>, on: bool) -> R<()> {
    let state = app.state::<AppState>();
    if let Some(h) = state.mic_test.lock().take() {
        h.stop();
    }
    if !on {
        return Ok(());
    }
    let (tx, rx) = milkrice_core::bounded::<milkrice_core::AudioFrame>(32);
    let handle = audio::start_mic(device.filter(|d| !d.is_empty()), tx).map_err(e)?;
    *state.mic_test.lock() = Some(handle);
    std::thread::spawn(move || {
        let mut peak = 0f32;
        let mut last = std::time::Instant::now();
        let mut silent = true;
        while let Ok(f) = rx.recv() {
            for v in &f.samples {
                peak = peak.max(v.abs());
                if *v != 0.0 {
                    silent = false;
                }
            }
            if last.elapsed() >= std::time::Duration::from_millis(80) {
                let _ = app.emit("mic-test", json!({"level": peak, "digital_silence": silent}));
                peak = 0.0;
                last = std::time::Instant::now();
            }
        }
    });
    Ok(())
}

// ------------------------------------------------------------------ models

#[tauri::command]
pub fn list_models(state: State<AppState>) -> Value {
    let s = state.settings();
    let downloading: Vec<String> = state.downloads.lock().keys().cloned().collect();
    json!({
        "models": models::statuses(),
        "active": s.model_id,
        "downloading": downloading,
        "speaker_model": models::speaker_model_present(),
    })
}

#[tauri::command]
pub fn download_model(app: AppHandle, state: State<AppState>, id: String) -> R<()> {
    let spec = catalog::find(&id).ok_or("unknown model")?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut d = state.downloads.lock();
        if d.contains_key(&id) {
            return Ok(());
        }
        d.insert(id.clone(), cancel.clone());
    }
    std::thread::spawn(move || {
        let app2 = app.clone();
        let id2 = id.clone();
        let res = models::download(spec, &cancel, move |done, total| {
            let _ = app2.emit("download-progress", json!({"id": id2, "done": done, "total": total}));
        });
        let state = app.state::<AppState>();
        state.downloads.lock().remove(&id);
        let err = res.as_ref().err().map(|x| x.to_string());
        if res.is_ok() {
            // First model becomes the active one.
            let mut s = state.settings();
            if s.model_id.is_none() {
                s.model_id = Some(id.clone());
                let _ = state.save_settings(s);
            }
            // Speaker model piggybacks on the first download.
            if !models::speaker_model_present() {
                let _ = recorder::ensure_embedder(&app);
            }
        }
        let _ = app.emit("download-progress", json!({"id": id, "finished": true, "error": err}));
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_download(state: State<AppState>, id: String) {
    if let Some(c) = state.downloads.lock().get(&id) {
        c.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub fn delete_model(state: State<AppState>, id: String) -> R<()> {
    let spec = catalog::find(&id).ok_or("unknown model")?;
    let mut s = state.settings();
    if s.model_id.as_deref() == Some(id.as_str()) {
        if state.recording.lock().is_some() {
            return Err("Stop recording before deleting the active model.".into());
        }
        *state.engine.lock() = None;
        s.model_id = None;
        state.save_settings(s).map_err(e)?;
    }
    models::delete(spec).map_err(e)
}

// ------------------------------------------------------------------ AI providers

#[tauri::command]
pub async fn providers(app: AppHandle, refresh: bool) -> R<Value> {
    let state = app.state::<AppState>();
    let cached = state.providers.lock().clone();
    let list = match (cached, refresh) {
        (Some(l), false) => l,
        _ => {
            let l = blocking(|| Ok(llm::detect())).await?;
            *state.providers.lock() = Some(l.clone());
            l
        }
    };
    let templates: Vec<Value> =
        llm::TEMPLATES.iter().map(|(id, name, desc)| json!({"id": id, "name": name, "desc": desc})).collect();
    Ok(json!({"providers": list, "templates": templates}))
}

// ------------------------------------------------------------------ calendar

#[tauri::command]
pub fn upcoming_events(state: State<AppState>) -> Vec<Value> {
    let now = chrono::Utc::now();
    let s = state.settings();
    state
        .events
        .lock()
        .iter()
        .filter(|ev| ev.end > now - chrono::Duration::hours(2) && !ev.all_day && !s.is_hidden(ev))
        .take(40)
        .map(|ev| {
            json!({
                "event": ev,
                "real": scheduler::is_real_meeting(ev),
                "meeting_id": state.store.find_by_event(&ev.id).ok().flatten(),
            })
        })
        .collect()
}

/// Meetings hidden from Coming up, newest first. One-offs drop off a day after they've passed.
#[tauri::command]
pub fn hidden_events(state: State<AppState>) -> Vec<HiddenEvent> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(1);
    state.settings().hidden_events.into_iter().filter(|h| h.start.is_none_or(|t| t > cutoff)).rev().collect()
}

/// Hide a calendar event from Coming up, the menu bar, nudges and auto-record: the whole
/// recurring series, or just this occurrence.
#[tauri::command]
pub fn hide_event(app: AppHandle, state: State<AppState>, event_id: String, series: bool) -> R<HiddenEvent> {
    let ev = state.events.lock().iter().find(|e| e.id == event_id).cloned().ok_or("That event is no longer on your calendar")?;
    let series = series && ev.recurring;
    let h = HiddenEvent {
        key: if series { ev.series_id().to_string() } else { ev.id.clone() },
        title: ev.title.trim().to_string(),
        series,
        start: if series { None } else { Some(ev.start) },
    };
    let mut s = state.settings();
    let cutoff = chrono::Utc::now() - chrono::Duration::days(1);
    s.hidden_events.retain(|x| x.key != h.key && x.start.is_none_or(|t| t > cutoff));
    s.hidden_events.push(h.clone());
    // A "meeting is starting" nudge for it goes too.
    let nudged = state.prompt.lock().as_ref().and_then(|p| p.event.as_ref()).is_some_and(|pe| s.is_hidden(pe));
    state.save_settings(s).map_err(e)?;
    if nudged {
        *state.prompt.lock() = None;
        let _ = app.emit("prompt", Value::Null);
    }
    events_changed(&app);
    Ok(h)
}

#[tauri::command]
pub fn unhide_event(app: AppHandle, state: State<AppState>, key: String) -> R<()> {
    let mut s = state.settings();
    s.hidden_events.retain(|x| x.key != key);
    state.save_settings(s).map_err(e)?;
    events_changed(&app);
    Ok(())
}

fn events_changed(app: &AppHandle) {
    let _ = app.emit("events-updated", ());
    crate::tray::refresh(app);
}

/// Subscribe to a read-only iCal feed (e.g. Google Calendar's secret address). Validated first.
#[tauri::command]
pub async fn add_calendar_feed(app: AppHandle, name: String, url: String, owner_email: Option<String>) -> R<Value> {
    let feed = crate::ics::Feed {
        id: uuid::Uuid::new_v4().to_string(),
        name: if name.trim().is_empty() { "Google Calendar".into() } else { name.trim().to_string() },
        url: url.trim().to_string(),
        owner_email: owner_email.map(|e| e.trim().to_string()).filter(|e| e.contains('@')),
    };
    let f2 = feed.clone();
    let upcoming = blocking(move || crate::ics::test_feed(&f2)).await?;
    let state = app.state::<AppState>();
    let mut s = state.settings();
    s.calendar_feeds.retain(|f| f.url != feed.url);
    s.calendar_feeds.push(feed.clone());
    state.save_settings(s).map_err(e)?;
    log::info!("calendar feed added: '{}' ({}), {upcoming} events in the next 30 days", feed.name, crate::ics::host(&feed.url));
    let a = app.clone();
    std::thread::spawn(move || scheduler::refresh_events(&a));
    Ok(json!({"id": feed.id, "upcoming": upcoming}))
}

#[tauri::command]
pub fn remove_calendar_feed(app: AppHandle, state: State<AppState>, id: String) -> R<()> {
    let mut s = state.settings();
    s.calendar_feeds.retain(|f| f.id != id);
    state.save_settings(s).map_err(e)?;
    crate::ics::forget(&id);
    let a = app.clone();
    std::thread::spawn(move || scheduler::refresh_events(&a));
    Ok(())
}

/// Calendar sources for Settings: the macOS calendars (via permission) and subscribed feeds
/// (URLs never sent to the UI in full).
#[tauri::command]
pub fn calendar_sources(state: State<AppState>) -> Value {
    let feeds: Vec<Value> = state
        .settings()
        .calendar_feeds
        .iter()
        .map(|f| json!({"id": f.id, "name": f.name, "host": crate::ics::host(&f.url), "owner_email": f.owner_email}))
        .collect();
    let mut cals: Vec<String> = state.events.lock().iter().map(|e| e.calendar.clone()).collect();
    cals.sort();
    cals.dedup();
    json!({"feeds": feeds, "calendars_with_events": cals, "event_count": state.events.lock().len()})
}

#[tauri::command]
pub fn current_prompt(state: State<AppState>) -> Value {
    serde_json::to_value(state.prompt.lock().clone()).unwrap_or(Value::Null)
}

#[tauri::command]
pub fn dismiss_prompt(app: AppHandle, state: State<AppState>) {
    *state.prompt.lock() = None;
    let _ = app.emit("prompt", Value::Null);
    crate::tray::refresh(&app);
}

/// Start (or open) notes for a calendar event.
#[tauri::command]
pub async fn start_event(app: AppHandle, event: CalEvent, record: bool) -> R<String> {
    blocking(move || {
        let state = app.state::<AppState>();
        if record {
            scheduler::start_for_event(&app, &event)?;
            Ok(state.store.find_by_event(&event.id)?.unwrap_or_default())
        } else {
            Ok(match state.store.find_by_event(&event.id)? {
                Some(id) => id,
                None => recorder::new_meeting(&state, &event.title, Some(&event))?.id,
            })
        }
    })
    .await
}

// ------------------------------------------------------------------ meetings

#[tauri::command]
pub fn list_meetings(state: State<AppState>, query: Option<String>) -> R<Vec<MeetingRow>> {
    let phone = milkrice_sync::desktop::iphone_meetings();
    let rows = state.store.list_meetings(query.as_deref(), 500).map_err(e)?;
    Ok(rows.into_iter().map(|m| MeetingRow { from_iphone: phone.contains(&m.id), m }).collect())
}

#[derive(Serialize)]
pub struct MeetingRow {
    #[serde(flatten)]
    m: MeetingSummary,
    /// Recorded on a paired iPhone.
    from_iphone: bool,
}

#[derive(Serialize)]
pub struct MeetingDetail {
    meeting: Meeting,
    segments: Vec<Segment>,
    chats: Vec<ChatMessage>,
    busy: Option<String>,
    /// Folders the note is in.
    folders: Vec<Folder>,
    /// A folder to offer when the note is in none (and the suggestion wasn't dismissed).
    folder_suggestion: Option<Folder>,
    /// Folders that auto-collect future meetings of this note's calendar series.
    series_folders: Vec<String>,
    /// True when the note came from a calendar event (so "auto-add future meetings" applies).
    has_series: bool,
}

#[tauri::command]
pub fn get_meeting(state: State<AppState>, id: String) -> R<MeetingDetail> {
    let meeting = state.store.get_meeting(&id).map_err(e)?.ok_or("not found")?;
    let folders = state.store.folders_of(&id).map_err(e)?;
    let folder_suggestion = if folders.is_empty() { recorder::folder_suggestion(&state, &meeting) } else { None };
    let series = recorder::series_of(&meeting);
    Ok(MeetingDetail {
        segments: state.store.segments(&id).map_err(e)?,
        chats: state.store.chats(&id).map_err(e)?,
        busy: state.busy.lock().get(&id).cloned(),
        folders,
        folder_suggestion,
        series_folders: state.store.series_folders(&series).map_err(e)?,
        has_series: !series.is_empty(),
        meeting,
    })
}

/// Leaving a note: if nothing was ever put in it, don't keep it.
#[tauri::command]
pub fn discard_if_empty(state: State<AppState>, keep: Option<String>) -> R<usize> {
    state.store.purge_empty_drafts(keep.as_deref()).map_err(e)
}

// ------------------------------------------------------------------ folders

#[tauri::command]
pub fn list_folders(state: State<AppState>) -> R<Vec<Folder>> {
    state.store.list_folders().map_err(e)
}

/// Create a folder (or reuse one with the same name); optionally put a note in it.
#[tauri::command]
pub fn create_folder(app: AppHandle, state: State<AppState>, name: String, meeting_id: Option<String>) -> R<Folder> {
    let f = state.store.create_folder(&name).map_err(e)?;
    recorder::folders_changed();
    if let Some(id) = meeting_id {
        state.store.set_meeting_folder(&id, &f.id, true).map_err(e)?;
        crate::export::schedule(&app, &id);
    }
    Ok(f)
}

#[tauri::command]
pub fn rename_folder(state: State<AppState>, id: String, name: String) -> R<()> {
    state.store.rename_folder(&id, &name).map_err(e)?;
    recorder::folders_changed();
    Ok(())
}

#[tauri::command]
pub fn set_folder_color(state: State<AppState>, id: String, color: String) -> R<()> {
    state.store.set_folder_color(&id, &color).map_err(e)
}

#[tauri::command]
pub fn delete_folder(state: State<AppState>, id: String) -> R<()> {
    state.store.delete_folder(&id).map_err(e)?;
    recorder::folders_changed();
    Ok(())
}

#[tauri::command]
pub fn set_meeting_folder(app: AppHandle, state: State<AppState>, meeting_id: String, folder_id: String, on: bool) -> R<()> {
    state.store.set_meeting_folder(&meeting_id, &folder_id, on).map_err(e)?;
    recorder::folders_changed();
    if !on {
        // Taking a note out of a folder also stops auto-adding its series there (as in Granola).
        if let Some(m) = state.store.get_meeting(&meeting_id).map_err(e)? {
            let series = recorder::series_of(&m);
            if !series.is_empty() {
                state.store.set_series_folder(&series, &folder_id, false).map_err(e)?;
            }
        }
    }
    crate::export::schedule(&app, &meeting_id);
    Ok(())
}

/// Turn "auto-add future meetings of this series" on or off for a folder.
#[tauri::command]
pub fn set_folder_series(state: State<AppState>, meeting_id: String, folder_id: String, on: bool) -> R<()> {
    let m = state.store.get_meeting(&meeting_id).map_err(e)?.ok_or("not found")?;
    state.store.set_series_folder(&recorder::series_of(&m), &folder_id, on).map_err(e)
}

/// "Not this one": stop suggesting a folder for this note.
#[tauri::command]
pub fn dismiss_folder_suggestion(state: State<AppState>, meeting_id: String) -> R<()> {
    state.store.set_folder_hint(&meeting_id, "dismissed").map_err(e)
}

#[tauri::command]
pub fn create_meeting(state: State<AppState>, title: Option<String>) -> R<Meeting> {
    recorder::new_meeting(&state, title.as_deref().unwrap_or(""), None).map_err(e)
}

#[tauri::command]
pub fn update_meeting(app: AppHandle, state: State<AppState>, id: String, field: String, value: String) -> R<()> {
    state.store.set_field(&id, &field, &value).map_err(e)?;
    if field == "enhanced" {
        // Mark the AI notes as edited by the user (export/agents label them; Regenerate asks first).
        state.store.set_field(&id, "enhanced_edited", "1").map_err(e)?;
    }
    crate::export::schedule(&app, &id);
    Ok(())
}

#[tauri::command]
pub fn update_segment(state: State<AppState>, id: i64, text: String) -> R<()> {
    state.store.update_segment_text(id, &text).map_err(e)
}

#[tauri::command]
pub fn delete_meeting(app: AppHandle, state: State<AppState>, id: String) -> R<()> {
    if state.recording.lock().as_ref().map(|r| r.meeting_id == id).unwrap_or(false) {
        recorder::stop_and_wait(&app, std::time::Duration::from_secs(20));
    }
    let _ = milkrice_core::audio_rec::delete_recordings(&id);
    state.store.delete_meeting(&id).map_err(e)
}

/// Kept recordings for a meeting (opt-in setting), each with the transcript offset it starts at.
#[tauri::command]
pub fn meeting_audio(id: String) -> Value {
    json!(milkrice_core::audio_rec::recordings(&id)
        .into_iter()
        .map(|(offset_ms, p)| json!({"offset_ms": offset_ms, "path": p.to_string_lossy()}))
        .collect::<Vec<_>>())
}

#[tauri::command]
pub fn delete_meeting_audio(id: String) -> R<()> {
    milkrice_core::audio_rec::delete_recordings(&id).map_err(e)
}

#[tauri::command]
pub fn audio_storage() -> Value {
    json!({"bytes": milkrice_core::audio_rec::usage_bytes()})
}

/// Rename a speaker; naming two speakers the same merges them.
#[tauri::command]
pub fn rename_speaker(app: AppHandle, state: State<AppState>, id: String, key: String, name: String) -> R<()> {
    crate::export::schedule(&app, &id);
    let mut m = state.store.get_meeting(&id).map_err(e)?.ok_or("not found")?;
    let name = name.trim().to_string();
    // Naming (or confirming) a speaker by hand clears the AI's guess.
    m.speakers_guessed.retain(|k| *k != key);
    if let Some((other, _)) = m.speakers.iter().find(|(k, v)| **k != key && v.eq_ignore_ascii_case(&name) && !name.is_empty()) {
        let other = other.clone();
        state.store.reassign_speaker(&id, &key, &other).map_err(e)?;
        m.speakers.remove(&key);
    } else if name.is_empty() {
        m.speakers.remove(&key);
    } else {
        m.speakers.insert(key, name);
    }
    state.store.set_field(&id, "speakers_guessed", &serde_json::to_string(&m.speakers_guessed).map_err(e)?).map_err(e)?;
    state.store.set_field(&id, "speakers", &serde_json::to_string(&m.speakers).map_err(e)?).map_err(e)
}

// ------------------------------------------------------------------ recording

#[tauri::command]
pub async fn start_recording(app: AppHandle, id: String) -> R<()> {
    blocking(move || recorder::start(&app, &id, None)).await
}

#[tauri::command]
pub fn stop_recording(app: AppHandle, ai: Option<bool>) -> R<()> {
    // `ai: false` skips the after-meeting AI pass (the onboarding mic test).
    recorder::stop(&app, ai.unwrap_or(true)).map_err(e)
}

/// Turn auto-record off (switch to "notify me"), from the sidebar marker, banner or tray.
#[tauri::command]
pub fn turn_off_auto_record(app: AppHandle) -> R<Settings> {
    crate::tray::turn_off_auto_record(&app).map_err(e)?;
    Ok(redacted(app.state::<AppState>().settings()))
}

#[tauri::command]
pub fn autostart_get(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
pub fn autostart_set(app: AppHandle, on: bool) -> R<bool> {
    use tauri_plugin_autostart::ManagerExt;
    let l = app.autolaunch();
    if on { l.enable() } else { l.disable() }.map_err(e)?;
    Ok(l.is_enabled().unwrap_or(on))
}

#[tauri::command]
pub fn pause_recording(app: AppHandle, state: State<AppState>, paused: bool) {
    if let Some(r) = state.recording.lock().as_ref() {
        if paused {
            r.session.pause()
        } else {
            r.session.resume()
        }
    }
    recorder::emit_rec_state(&app);
}

#[tauri::command]
pub fn rec_state(app: AppHandle) {
    recorder::emit_rec_state(&app);
}

// ------------------------------------------------------------------ AI

#[tauri::command]
pub async fn enhance(app: AppHandle, id: String) -> R<String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let m = state.store.get_meeting(&id)?.ok_or_else(|| anyhow::anyhow!("not found"))?;
        let segs = state.store.segments(&id)?;
        anyhow::ensure!(!segs.is_empty() || !m.notes.trim().is_empty(), "Nothing to work with yet — record or write something first.");
        let (p, model) = recorder::provider(&state)?;
        state.busy.lock().insert(id.clone(), "Writing notes".into());
        let _ = app.emit("busy", json!({"id": id, "what": "Writing notes"}));
        let r = recorder::enhance(&app, &m, &segs, p, &model);
        state.busy.lock().remove(&id);
        let _ = app.emit("busy", json!({"id": id, "what": null}));
        r
    })
    .await
}

#[tauri::command]
pub async fn identify_speakers(app: AppHandle, id: String) -> R<()> {
    blocking(move || {
        let state = app.state::<AppState>();
        let mut m = state.store.get_meeting(&id)?.ok_or_else(|| anyhow::anyhow!("not found"))?;
        let segs = state.store.segments(&id)?;
        // The calendar can settle a 1:1 without any AI.
        if recorder::name_by_structure(&state, &id) {
            m = state.store.get_meeting(&id)?.ok_or_else(|| anyhow::anyhow!("not found"))?;
            let unnamed = segs.iter().any(|s| s.source == milkrice_core::AudioSource::System && !m.speakers.contains_key(&s.speaker));
            if !unnamed {
                let _ = app.emit("meeting-updated", json!({"id": id}));
                crate::export::schedule(&app, &id);
                return Ok(());
            }
        }
        let (p, model) = recorder::provider(&state)?;
        let (sys, prompt) = llm::speaker_names_prompt(&recorder::with_user_name(&state, m.clone()), &segs)
            .ok_or_else(|| anyhow::anyhow!("No other speakers in this meeting."))?;
        let map = llm::parse_speaker_map(&llm::complete(p, &model, &sys, &prompt)?);
        // Only fill in names the user hasn't set: a confirmed name always wins.
        let mut added = 0;
        for (k, v) in map {
            if m.speakers.contains_key(&k) && !m.speakers_guessed.contains(&k) {
                continue;
            }
            if !m.speakers_guessed.contains(&k) {
                m.speakers_guessed.push(k.clone());
            }
            m.speakers.insert(k, v);
            added += 1;
        }
        anyhow::ensure!(added > 0, "Couldn't tell who's who from the conversation — name speakers by clicking them.");
        state.store.set_field(&id, "speakers", &serde_json::to_string(&m.speakers)?)?;
        state.store.set_field(&id, "speakers_guessed", &serde_json::to_string(&m.speakers_guessed)?)?;
        let _ = app.emit("meeting-updated", json!({"id": id}));
        crate::export::schedule(&app, &id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn ask(app: AppHandle, id: String, question: String) -> R<String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let m = recorder::with_user_name(&state, state.store.get_meeting(&id)?.ok_or_else(|| anyhow::anyhow!("not found"))?);
        let segs = state.store.segments(&id)?;
        let chats = state.store.chats(&id)?;
        let mut history = Vec::new();
        let mut q: Option<String> = None;
        for c in chats {
            if c.role == "user" {
                q = Some(c.content);
            } else if let Some(qq) = q.take() {
                history.push((qq, c.content));
            }
        }
        let (p, model) = recorder::provider(&state)?;
        let (sys, prompt) = llm::ask_prompt(&m, &segs, &history, &question);
        state.store.add_chat(&id, "user", &question)?;
        let answer = llm::complete(p, &model, &sys, &prompt)?;
        state.store.add_chat(&id, "assistant", &answer)?;
        Ok(answer)
    })
    .await
}

#[tauri::command]
pub async fn ask_all(app: AppHandle, question: String) -> R<String> {
    blocking(move || {
        let state = app.state::<AppState>();
        // Most relevant first: search hits, then recent meetings.
        let words: Vec<&str> = question.split_whitespace().filter(|w| w.len() > 4).collect();
        let mut ids: Vec<String> = Vec::new();
        for w in &words {
            for m in state.store.list_meetings(Some(w.trim_matches(|c: char| !c.is_alphanumeric())), 6)? {
                if !ids.contains(&m.id) {
                    ids.push(m.id);
                }
            }
        }
        for m in state.store.list_meetings(None, 12)? {
            if !ids.contains(&m.id) {
                ids.push(m.id);
            }
        }
        ids.truncate(12);
        let mut data = Vec::new();
        for id in ids {
            if let Some(m) = state.store.get_meeting(&id)? {
                let segs = state.store.segments(&id)?;
                data.push((m, segs));
            }
        }
        anyhow::ensure!(!data.is_empty(), "No meetings yet.");
        let (p, model) = recorder::provider(&state)?;
        let (sys, prompt) = llm::ask_all_prompt(&data, &question);
        llm::complete(p, &model, &sys, &prompt)
    })
    .await
}

// ------------------------------------------------------------------ agents (CLI, MCP, skill)
// The logic lives in milkrice_core::agents_install, shared with `milkrice agents …`.

/// The milkrice-cli shipped next to this app's binary.
pub fn cli_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join("milkrice-cli");
    p.is_file().then_some(p)
}

fn agents_env() -> agents_install::Env {
    agents_install::Env::from_system(cli_path())
}

#[tauri::command]
pub async fn agents_status() -> R<Value> {
    blocking(|| Ok(serde_json::to_value(agents_install::status(&agents_env()))?)).await
}

/// One-click agent setup. target: cli | claude | codex | cursor | opencode | vscode.
/// Agent targets also install the `milkrice` command, which the skill relies on.
#[tauri::command]
pub async fn agent_connect(target: String) -> R<String> {
    blocking(move || {
        let t = agents_install::Target::parse(&target)?;
        let log = agents_install::install(&agents_env(), t)?;
        Ok(match t {
            agents_install::Target::Cli => match agents_install::resolve_command(&agents_env()) {
                Some((p, _)) => format!("`milkrice` is installed at {}", p.display()),
                None => "Installed the milkrice command".into(),
            },
            _ => {
                let what = if log.changed.is_empty() { "was already connected" } else { "can now read your meetings (MCP + skill)" };
                format!("{} {what}. Restart it to pick up the change.", t.label())
            }
        })
    })
    .await
}

// ------------------------------------------------------------------ Obsidian / second brain

#[tauri::command]
pub fn obsidian_vaults() -> Value {
    json!(milkrice_core::obsidian::detect_vaults())
}

/// Write every past meeting into the chosen vault/folder. Returns how many notes were written.
#[tauri::command]
pub async fn obsidian_export_all(app: AppHandle) -> R<usize> {
    blocking(move || {
        let state = app.state::<AppState>();
        anyhow::ensure!(state.settings().obsidian.is_some(), "Choose an Obsidian vault or folder first.");
        let mut n = 0;
        for m in state.store.list_meetings(None, 100_000)? {
            if crate::export::export_now(&app, &m.id)?.is_some() {
                n += 1;
            }
        }
        log::info!("exported {n} meetings to Obsidian");
        Ok(n)
    })
    .await
}

/// Open a meeting's note in Obsidian (falls back to the default Markdown app).
#[tauri::command]
pub async fn open_in_obsidian(app: AppHandle, id: String) -> R<()> {
    let path = {
        let a = app.clone();
        let id2 = id.clone();
        blocking(move || crate::export::export_now(&a, &id2)).await?
    }
    .ok_or("Turn on Obsidian export in Settings first.")?;
    let url = format!(
        "obsidian://open?path={}",
        path.to_string_lossy().bytes().map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        }).collect::<String>()
    );
    let ok = std::process::Command::new("/usr/bin/open").arg(&url).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        let _ = std::process::Command::new("/usr/bin/open").arg(&path).status();
    }
    Ok(())
}

// ------------------------------------------------------------------ export

pub fn to_markdown(m: &Meeting, segs: &[Segment]) -> String {
    let mut out = format!("# {}\n\n", if m.title.is_empty() { "Untitled meeting" } else { &m.title });
    out.push_str(&format!("*{}*", m.created_at.with_timezone(&chrono::Local).format("%A %e %B %Y, %H:%M")));
    if !m.attendees.is_empty() {
        out.push_str(&format!(" · {}", m.attendees.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    out.push_str("\n\n");
    if !m.enhanced.trim().is_empty() {
        out.push_str(m.enhanced.trim());
        out.push_str("\n\n");
    }
    if !m.notes.trim().is_empty() {
        out.push_str("## My notes\n\n");
        out.push_str(m.notes.trim());
        out.push_str("\n\n");
    }
    if !segs.is_empty() {
        out.push_str("## Transcript\n\n");
        out.push_str(&llm::transcript_text(segs, &m.speakers, usize::MAX));
        out.push('\n');
    }
    out
}

#[tauri::command]
pub fn export_markdown(state: State<AppState>, id: String, save: bool) -> R<String> {
    let m = state.store.get_meeting(&id).map_err(e)?.ok_or("not found")?;
    let segs = state.store.segments(&id).map_err(e)?;
    let md = to_markdown(&m, &segs);
    if !save {
        return Ok(md);
    }
    let dir = dirs::document_dir().unwrap_or_else(milkrice_core::paths::data_dir).join("Milkrice");
    std::fs::create_dir_all(&dir).map_err(e)?;
    let safe: String = m
        .title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' })
        .collect();
    let name = format!("{} {}.md", m.created_at.with_timezone(&chrono::Local).format("%Y-%m-%d"), safe.trim());
    let path = dir.join(name);
    std::fs::write(&path, md).map_err(e)?;
    Ok(path.to_string_lossy().to_string())
}

// ---------- iPhone sync

#[tauri::command]
pub async fn sync_status() -> R<Value> {
    tauri::async_runtime::spawn_blocking(crate::sync::status).await.map_err(e)
}

#[tauri::command]
pub async fn sync_setup(folder: Option<String>) -> R<Value> {
    tauri::async_runtime::spawn_blocking(move || crate::sync::setup(folder)).await.map_err(e)?.map_err(e)
}

#[tauri::command]
pub async fn sync_pair_start() -> R<Value> {
    tauri::async_runtime::spawn_blocking(crate::sync::pair_start).await.map_err(e)?.map_err(e)
}

#[tauri::command]
pub fn sync_pair_poll() -> R<Option<milkrice_sync::desktop::Joining>> {
    crate::sync::pair_poll().map_err(e)
}

#[tauri::command]
pub async fn sync_pair_accept() -> R<String> {
    tauri::async_runtime::spawn_blocking(crate::sync::pair_accept).await.map_err(e)?.map_err(e)
}

#[tauri::command]
pub fn sync_pair_cancel() {
    crate::sync::pair_cancel()
}

#[tauri::command]
pub fn sync_now() {
    crate::sync::wake()
}

#[tauri::command]
pub fn sync_leave() -> R<()> {
    crate::sync::leave().map_err(e)
}
