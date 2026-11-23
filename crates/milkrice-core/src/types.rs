use serde::{Deserialize, Serialize};

/// Target sample rate for everything downstream of capture.
pub const SAMPLE_RATE: u32 = 16_000;

/// Where a piece of audio came from. Mic = the user ("Me"); System = everyone else on the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioSource {
    Mic,
    System,
}

/// A block of 16 kHz mono f32 samples from one source.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub source: AudioSource,
    /// 16 kHz mono, for transcription.
    pub samples: Vec<f32>,
    /// 48 kHz mono for a kept recording (`audio::RECORD_RATE`); empty when none is kept.
    pub hires: Vec<f32>,
}

/// One transcribed utterance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: i64,
    pub start_ms: u64,
    pub end_ms: u64,
    pub source: AudioSource,
    /// Stable speaker key within a meeting: "me", "s1", "s2"...
    pub speaker: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Attendee {
    pub name: String,
    pub email: Option<String>,
    pub is_self: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalEvent {
    /// EventKit identifier plus start time, unique per occurrence.
    pub id: String,
    pub title: String,
    pub start: chrono::DateTime<chrono::Utc>,
    pub end: chrono::DateTime<chrono::Utc>,
    pub attendees: Vec<Attendee>,
    pub location: Option<String>,
    /// Zoom/Meet/Teams link found in url, location or notes.
    pub meeting_url: Option<String>,
    pub calendar: String,
    pub calendar_color: Option<String>,
    pub all_day: bool,
}
