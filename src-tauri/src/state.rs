use chrono::{DateTime, Utc};
use milkrice_core::diarize::SpeakerEmbedder;
use milkrice_core::llm::{Provider, ProviderInfo};
use milkrice_core::session::Session;
use milkrice_core::store::Store;
use milkrice_core::stt::Transcriber;
use milkrice_core::CalEvent;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub model_id: Option<String>,
    /// "auto" or a language code.
    pub language: String,
    pub provider: Option<Provider>,
    pub provider_model: String,
    pub capture_system: bool,
    pub mic_device: Option<String>,
    /// What to do when a calendar meeting starts: "notify" | "auto" | "off".
    pub calendar_mode: String,
    /// Stop recording automatically when the calendar event is over and the room goes quiet.
    pub auto_stop: bool,
    /// Notice calls started outside the calendar (Zoom/Teams/... using the mic).
    pub detect_calls: bool,
    /// Only meetings with other people or a call link count (skip focus blocks etc.).
    pub only_real_meetings: bool,
    pub auto_enhance: bool,
    pub diarize: bool,
    pub default_template: String,
    pub user_name: String,
    pub onboarded: bool,
    /// Read-only iCal subscriptions (e.g. Google Calendar's secret address).
    pub calendar_feeds: Vec<crate::ics::Feed>,
    /// Opt-in: keep a compressed local recording of each meeting.
    pub keep_audio: bool,
    /// Delete kept recordings after this many days (0 = keep until deleted).
    pub audio_retention_days: u32,
    /// Mirror every meeting as Markdown into an Obsidian vault / folder (None = off).
    pub obsidian: Option<milkrice_core::obsidian::ExportOptions>,
    /// Calendar events the user hid from Coming up (no nudges, no auto-record either).
    /// Managed only by hide_event / unhide_event.
    pub hidden_events: Vec<HiddenEvent>,
}

/// A hidden calendar event: a whole recurring series, or one occurrence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HiddenEvent {
    /// `CalEvent::series_id` for a series, `CalEvent::id` for one occurrence.
    pub key: String,
    pub title: String,
    pub series: bool,
    /// Start of the hidden occurrence (one-offs only), to show it and forget it once it's past.
    pub start: Option<DateTime<Utc>>,
}

impl Settings {
    pub fn is_hidden(&self, ev: &CalEvent) -> bool {
        self.hidden_events.iter().any(|h| if h.series { h.key == ev.series_id() } else { h.key == ev.id })
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model_id: None,
            language: "auto".into(),
            provider: None,
            provider_model: String::new(),
            capture_system: true,
            mic_device: None,
            calendar_mode: "notify".into(),
            auto_stop: true,
            detect_calls: true,
            only_real_meetings: true,
            auto_enhance: true,
            diarize: true,
            default_template: "auto".into(),
            user_name: String::new(),
            onboarded: false,
            calendar_feeds: Vec::new(),
            keep_audio: false,
            audio_retention_days: 30,
            obsidian: None,
            hidden_events: Vec::new(),
        }
    }
}

pub struct ActiveRecording {
    pub meeting_id: String,
    pub session: Session,
    pub event_end: Option<DateTime<Utc>>,
    /// Last time anyone said something (for auto-stop).
    pub last_speech: Arc<Mutex<Instant>>,
    pub offset_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Prompt {
    /// "calendar" | "call"
    pub kind: String,
    pub title: String,
    pub event: Option<CalEvent>,
    pub app: Option<String>,
    pub at: DateTime<Utc>,
}

pub struct AppState {
    pub store: Arc<Store>,
    pub settings: Mutex<Settings>,
    pub engine: Mutex<Option<Arc<Mutex<Transcriber>>>>,
    pub engine_loading: AtomicBool,
    pub embedder: Mutex<Option<Arc<Mutex<SpeakerEmbedder>>>>,
    pub recording: Mutex<Option<ActiveRecording>>,
    pub downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub providers: Mutex<Option<Vec<ProviderInfo>>>,
    pub events: Mutex<Vec<CalEvent>>,
    pub prompt: Mutex<Option<Prompt>>,
    /// Meetings currently being enhanced, so the UI can show progress.
    pub busy: Mutex<HashMap<String, String>>,
    /// Live level preview for the mic picker.
    pub mic_test: Mutex<Option<milkrice_core::audio::CaptureHandle>>,
    /// Speaker voice prints per meeting, in memory only so resuming keeps labels stable.
    pub voiceprints: Mutex<HashMap<String, milkrice_core::diarize::SpeakerClusterer>>,
    /// Meeting id of a recording that auto-record started (shown loudly in the UI and tray).
    pub auto_recording: Mutex<Option<String>>,
}

impl AppState {
    pub fn new(store: Arc<Store>) -> Self {
        let settings = store
            .get_setting("settings")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            store,
            settings: Mutex::new(settings),
            engine: Mutex::new(None),
            engine_loading: AtomicBool::new(false),
            embedder: Mutex::new(None),
            recording: Mutex::new(None),
            downloads: Mutex::new(HashMap::new()),
            providers: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            prompt: Mutex::new(None),
            busy: Mutex::new(HashMap::new()),
            mic_test: Mutex::new(None),
            voiceprints: Mutex::new(HashMap::new()),
            auto_recording: Mutex::new(None),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().clone()
    }

    pub fn save_settings(&self, s: Settings) -> anyhow::Result<()> {
        self.store.set_setting("settings", &serde_json::to_string(&s)?)?;
        *self.settings.lock() = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str) -> CalEvent {
        CalEvent {
            id: id.into(),
            title: "Yoga".into(),
            start: Utc::now(),
            end: Utc::now(),
            attendees: vec![],
            location: None,
            meeting_url: None,
            calendar: "Personal".into(),
            calendar_color: None,
            all_day: false,
            recurring: true,
        }
    }

    #[test]
    fn hides_a_series_or_one_occurrence() {
        // Google iCal UIDs contain '@' themselves; only the last one is the start time.
        let mon = ev("ics:abc123@google.com@1700000000");
        let tue = ev("ics:abc123@google.com@1700086400");
        assert_eq!(mon.series_id(), "ics:abc123@google.com");
        let mut s = Settings::default();
        s.hidden_events.push(HiddenEvent { key: tue.id.clone(), title: "Yoga".into(), series: false, start: Some(tue.start) });
        assert!(!s.is_hidden(&mon) && s.is_hidden(&tue));
        s.hidden_events = vec![HiddenEvent { key: mon.series_id().into(), title: "Yoga".into(), series: true, start: None }];
        assert!(s.is_hidden(&mon) && s.is_hidden(&tue));
        assert!(!s.is_hidden(&ev("ics:other@google.com@1700000000")));
    }
}
