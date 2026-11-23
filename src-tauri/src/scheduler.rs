//! Background loop: watches the calendar and running calls, prompts or auto-starts recording
//! when a meeting begins, and stops it when the meeting is over.
use crate::recorder;
use crate::state::{AppState, Prompt};
use chrono::{Duration as CDur, Utc};
use milkrice_core::calendar::{self, CalendarAuth};
use milkrice_core::{detect, CalEvent};
use serde_json::json;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const TICK: Duration = Duration::from_secs(5);
/// Notify this long before the event's start.
const LEAD: i64 = 60;
/// After an event ends, stop once nobody has spoken for this long.
fn quiet_after_end() -> Duration {
    // Overridable for automated tests.
    let secs = std::env::var("MILKRICE_QUIET_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(90);
    Duration::from_secs(secs)
}
/// Stop regardless after this much silence (forgotten recordings).
const MAX_SILENCE: Duration = Duration::from_secs(15 * 60);
/// Before any conversation has happened (you're early, waiting for people to join), only a
/// much longer silence counts as a forgotten recording.
const MAX_SILENCE_WAITING: Duration = Duration::from_secs(45 * 60);
/// This much transcribed speech means the meeting has actually happened.
const CONVERSATION_MS: i64 = 60_000;

/// `"<calendar id>@<start unix>"` -> `"<calendar id>"`: the same event across reschedules.
fn event_base(id: &str) -> &str {
    id.rsplit_once('@').filter(|(_, ts)| ts.parse::<i64>().is_ok()).map(|(b, _)| b).unwrap_or(id)
}

/// The meeting being recorded was moved in the calendar: follow its new end time. Calendar
/// feeds can lag, so a recording started from stale times must not stop at the old end.
fn follow_moved_event(state: &AppState) {
    let (mid, end) = {
        let rec = state.recording.lock();
        match rec.as_ref() {
            Some(r) => (r.meeting_id.clone(), r.event_end),
            None => return,
        }
    };
    let Some(end) = end else { return };
    let Ok(Some(m)) = state.store.get_meeting(&mid) else { return };
    let Some(eid) = m.event_id else { return };
    let base = event_base(&eid).to_string();
    let moved = state
        .events
        .lock()
        .iter()
        .filter(|e| event_base(&e.id) == base && (e.end - end).num_hours().abs() < 12)
        .min_by_key(|e| (e.end - end).num_seconds().abs())
        .map(|e| e.end);
    if let Some(new_end) = moved.filter(|n| *n != end) {
        log::info!("recording's calendar event moved: now ends {new_end}");
        if let Some(r) = state.recording.lock().as_mut() {
            r.event_end = Some(new_end);
        }
    }
}
/// After the event ends, when we can't tell whether the call app still holds the mic but a
/// meeting app is running, stop after this much silence instead.
const QUIET_FALLBACK: Duration = Duration::from_secs(5 * 60);

/// Whether the call looks over from the call app's side: no known meeting app is capturing
/// the mic (Milkrice's own capture doesn't count).
fn call_app_released(quiet: Duration) -> bool {
    match detect::meeting_app_capturing_input() {
        Some(None) => true,
        Some(Some(app)) => {
            log::debug!("auto-stop: {app} still uses the mic; keep recording");
            false
        }
        // No per-process attribution (macOS < 14.2): any running meeting app means wait longer.
        None => detect::running_meeting_apps().is_empty() || quiet > QUIET_FALLBACK,
    }
}

pub fn is_real_meeting(e: &CalEvent) -> bool {
    !e.all_day && (e.meeting_url.is_some() || e.attendees.iter().filter(|a| !a.is_self).count() >= 1)
}

pub fn refresh_events(app: &AppHandle) {
    let state = app.state::<AppState>();
    if std::env::var("MILKRICE_TEST_EVENT").is_ok() {
        return; // test mode: events are injected
    }
    // Log permission changes once, so calendar problems are diagnosable from milkrice.log.
    static LAST_AUTH: parking_lot::Mutex<Option<CalendarAuth>> = parking_lot::Mutex::new(None);
    let auth = calendar::auth_status();
    {
        let mut last = LAST_AUTH.lock();
        if *last != Some(auth) {
            log::info!("calendar permission: {auth:?}");
            *last = Some(auth);
        }
    }
    let now = Utc::now();
    let mut evs: Vec<CalEvent> = Vec::new();
    if auth == CalendarAuth::Authorized {
        match calendar::events_between(now - CDur::hours(12), now + CDur::days(7)) {
            Ok(v) => evs = v,
            Err(e) => log::warn!("calendar: reading macOS calendars failed: {e}"),
        }
    }
    // Subscribed calendars (e.g. Google's secret iCal address) — read-only, no sign-in.
    evs.extend(crate::ics::events_between(app, now - CDur::hours(12), now + CDur::days(7)));
    evs.sort_by_key(|e| e.start);
    static LAST_COUNT: parking_lot::Mutex<Option<usize>> = parking_lot::Mutex::new(None);
    {
        let mut last = LAST_COUNT.lock();
        if *last != Some(evs.len()) {
            log::info!("calendar: {} events in the next 7 days ({} count as meetings)", evs.len(), evs.iter().filter(|e| is_real_meeting(e)).count());
            *last = Some(evs.len());
        }
    }
    {
        {
            // Diagnostics: MILKRICE_DUMP_EVENTS=<file> writes what the scheduler sees.
            if let Ok(path) = std::env::var("MILKRICE_DUMP_EVENTS") {
                let rows: Vec<serde_json::Value> = evs
                    .iter()
                    .map(|e| json!({"title": e.title, "start": e.start, "end": e.end, "attendees": e.attendees.len(),
                        "meeting_url": e.meeting_url, "calendar": e.calendar, "all_day": e.all_day, "counts_as_meeting": is_real_meeting(e)}))
                    .collect();
                let _ = std::fs::write(path, serde_json::to_string_pretty(&rows).unwrap_or_default());
            }
            *state.events.lock() = evs;
            let _ = app.emit("events-updated", ());
        }
    }
}

pub fn spawn(app: AppHandle) {
    std::thread::Builder::new()
        .name("milkrice-scheduler".into())
        .spawn(move || {
            let mut last_refresh = Instant::now() - Duration::from_secs(3600);
            let mut call_seen_at: Option<Instant> = None;
            let mut last_retention = Instant::now();
            loop {
                if last_refresh.elapsed() > Duration::from_secs(60) {
                    refresh_events(&app);
                    last_refresh = Instant::now();
                }
                if last_retention.elapsed() > Duration::from_secs(3600) {
                    let days = app.state::<AppState>().settings().audio_retention_days;
                    milkrice_core::audio_rec::enforce_retention(days);
                    last_retention = Instant::now();
                }
                tick(&app, &mut call_seen_at);
                recorder::emit_rec_state(&app);
                std::thread::sleep(TICK);
            }
        })
        .expect("scheduler thread");
}

fn tick(app: &AppHandle, call_seen_at: &mut Option<Instant>) {
    let state = app.state::<AppState>();
    let s = state.settings();
    let now = Utc::now();

    // ---- auto stop (never when the user turned it off)
    follow_moved_event(&state);
    let quiet_state = {
        let rec = state.recording.lock();
        rec.as_ref().map(|r| (r.meeting_id.clone(), r.last_speech.lock().elapsed(), r.event_end.map(|e| now > e).unwrap_or(false)))
    };
    let mut why = "";
    let should_stop = match quiet_state {
        Some((mid, quiet, event_over)) if s.auto_stop => {
            // A meeting can only be "over" once it has happened: a recording started early
            // (or from a stale calendar time) waits for the conversation instead of stopping
            // at the event's end.
            let talked = state.store.speech_ms(&mid).unwrap_or(0) >= CONVERSATION_MS;
            // Overrunning meetings with a reading pause keep recording while the call app
            // still has the mic.
            if talked && event_over && quiet > quiet_after_end() && call_app_released(quiet) {
                why = "event over and quiet";
                true
            } else if quiet > if talked { MAX_SILENCE } else { MAX_SILENCE_WAITING } {
                why = if talked { "15 min of silence" } else { "45 min with no conversation" };
                true
            } else {
                false
            }
        }
        _ => false,
    };
    if should_stop {
        log::info!("auto-stopping recording: {why}");
        let _ = recorder::stop(app, true);
        recorder::notify(app, "Meeting ended", "Recording stopped. Writing your notes…");
        return;
    }
    // While recording, only look for the *next* calendar meeting once this one's event is over
    // (back-to-back meetings get their own note instead of merging into the previous one).
    let (recording, current_over) = {
        let rec = state.recording.lock();
        (rec.is_some(), rec.as_ref().and_then(|r| r.event_end).map(|e| now > e).unwrap_or(false))
    };
    if recording && !current_over {
        return;
    }

    // ---- calendar meetings
    if s.calendar_mode != "off" {
        let events = state.events.lock().clone();
        let due = events.into_iter().find(|e| {
            let starts_soon = e.start - CDur::seconds(LEAD) <= now && now < e.end;
            starts_soon
                && (!s.only_real_meetings || is_real_meeting(e))
                && !e.all_day
                && !s.is_hidden(e)
                && state.store.event_flag(&e.id).is_none()
        });
        if let Some(ev) = due {
            let _ = state.store.set_event_flag(&ev.id, "prompted");
            if s.calendar_mode == "auto" {
                match start_for_event(app, &ev) {
                    Ok(()) => recorder::auto_started(app, &ev),
                    Err(e) => recorder::notify(app, &ev.title, &format!("Couldn't start recording: {e}")),
                }
            } else {
                let p = Prompt { kind: "calendar".into(), title: ev.title.clone(), event: Some(ev.clone()), app: None, at: now };
                *state.prompt.lock() = Some(p.clone());
                let _ = app.emit("prompt", &p);
                let who = ev.attendees.iter().filter(|a| !a.is_self).take(3).map(|a| a.name.clone()).collect::<Vec<_>>().join(", ");
                recorder::notify(
                    app,
                    &format!("{} is starting", ev.title),
                    &if who.is_empty() { "Open Milkrice to take notes".to_string() } else { format!("With {who}. Open Milkrice to take notes.") },
                );
                crate::tray::refresh(app);
            }
            return;
        }
    }

    // ---- ad-hoc calls (Zoom/Teams/... using the mic, no calendar event)
    if s.detect_calls && !recording {
        if let Some(name) = detect::meeting_app_using_mic() {
            let fresh = call_seen_at.map(|t| t.elapsed() > Duration::from_secs(30 * 60)).unwrap_or(true);
            let has_prompt = state.prompt.lock().is_some();
            if fresh && !has_prompt {
                *call_seen_at = Some(Instant::now());
                let p = Prompt { kind: "call".into(), title: format!("{name} call"), event: None, app: Some(name.clone()), at: now };
                *state.prompt.lock() = Some(p.clone());
                let _ = app.emit("prompt", &p);
                recorder::notify(app, &format!("{name} call detected"), "Open Milkrice to take notes on this call.");
                crate::tray::refresh(app);
            }
        }
    }

    // Expire stale prompts.
    let expired = state
        .prompt
        .lock()
        .as_ref()
        .map(|p| match &p.event {
            Some(e) => now > e.end,
            None => now - p.at > CDur::minutes(20),
        })
        .unwrap_or(false);
    if expired {
        *state.prompt.lock() = None;
        let _ = app.emit("prompt", serde_json::Value::Null);
        crate::tray::refresh(app);
    }
}

/// Create (or reopen) the note for an event and start recording.
pub fn start_for_event(app: &AppHandle, ev: &CalEvent) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    let id = match state.store.find_by_event(&ev.id)? {
        Some(id) => id,
        None => recorder::new_meeting(&state, &ev.title, Some(ev))?.id,
    };
    let _ = state.store.set_event_flag(&ev.id, "recorded");
    recorder::start(app, &id, Some(ev.end))?;
    let _ = app.emit("open-meeting", json!({"id": id}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::event_base;

    #[test]
    fn event_base_ignores_start_time() {
        assert_eq!(event_base("ics:abc@google.com@1795064400"), "ics:abc@google.com");
        assert_eq!(event_base("ics:abc@google.com@1795066200"), event_base("ics:abc@google.com@1795064400"));
        assert_eq!(event_base("ABC-123:XYZ@1795064400"), "ABC-123:XYZ");
        assert_eq!(event_base("no-time"), "no-time");
    }
}
