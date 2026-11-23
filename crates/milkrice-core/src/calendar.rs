//! Local calendar access via EventKit (reads every account synced into macOS Calendar —
//! iCloud, Google, Exchange — with no network or sign-in). OWNER: calendar worker.
use crate::types::CalEvent;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarAuth {
    NotDetermined,
    Denied,
    Restricted,
    Authorized,
    Unsupported,
}

pub fn auth_status() -> CalendarAuth {
    imp::auth_status()
}

/// Blocks until the user answers the macOS prompt (timeout ~120 s). Returns true if granted.
pub fn request_access() -> Result<bool> {
    imp::request_access()
}

/// Events (not all-day ones filtered out — caller decides) overlapping [start, end), sorted by start.
pub fn events_between(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<CalEvent>> {
    imp::events_between(start, end)
}

/// Find the first supported video-meeting link (Zoom, Meet, Teams, Webex, Whereby, Around) in `text`.
pub fn find_meeting_url(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(pos) = rest.find("https://") {
        let tail = &rest[pos..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | ')' | ']' | '\\'))
            .unwrap_or(tail.len());
        let candidate = tail[..end].trim_end_matches(['.', ',', ';', '>']);
        if is_meeting_host(host_of(candidate)) {
            return Some(candidate.to_string());
        }
        rest = &tail["https://".len()..];
    }
    None
}

fn host_of(url: &str) -> &str {
    let s = url.strip_prefix("https://").unwrap_or(url);
    let end = s.find(['/', '?', '#', ':']).unwrap_or(s.len());
    &s[..end]
}

fn is_meeting_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    let is = |d: &str| h == d || h.ends_with(&format!(".{d}"));
    is("zoom.us")
        || is("zoomgov.com")
        || h == "meet.google.com"
        || h == "teams.microsoft.com"
        || h == "teams.live.com"
        || is("webex.com")
        || is("whereby.com")
        || is("around.co")
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{find_meeting_url, CalendarAuth};
    use crate::types::{Attendee, CalEvent};
    use anyhow::{anyhow, Result};
    use block2::RcBlock;
    use chrono::{DateTime, TimeZone, Utc};
    use crossbeam_channel::{bounded, unbounded, Sender};
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2_app_kit::{NSColor, NSColorSpace};
    use objc2_event_kit::{
        EKAuthorizationStatus, EKCalendar, EKEntityType, EKEvent, EKEventStore, EKParticipant,
        EKParticipantStatus,
    };
    use objc2_foundation::{NSDate, NSError};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    pub fn auth_status() -> CalendarAuth {
        // SAFETY: class method with a plain enum argument; no preconditions.
        let s = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
        if s == EKAuthorizationStatus::NotDetermined {
            CalendarAuth::NotDetermined
        } else if s == EKAuthorizationStatus::Restricted {
            CalendarAuth::Restricted
        } else if s == EKAuthorizationStatus::FullAccess {
            CalendarAuth::Authorized
        } else {
            // Denied, WriteOnly (cannot read events) and unknown future values.
            CalendarAuth::Denied
        }
    }

    /// Owns the (non-Send) EKEventStore on a dedicated thread.
    struct Worker {
        store: Option<Retained<EKEventStore>>,
        store_authorized: bool,
        last_refresh: Instant,
    }

    impl Worker {
        /// Returns a store, recreating it if access was granted after it was created.
        fn store(&mut self) -> &EKEventStore {
            let authorized = auth_status() == CalendarAuth::Authorized;
            if self.store.is_none() || (authorized && !self.store_authorized) {
                // SAFETY: plain +alloc/-init of EKEventStore.
                self.store = Some(unsafe { EKEventStore::new() });
                self.store_authorized = authorized;
                self.last_refresh = Instant::now();
            } else if authorized && self.last_refresh.elapsed() > Duration::from_secs(300) {
                if let Some(s) = &self.store {
                    // SAFETY: no arguments; store is alive.
                    unsafe { s.refreshSourcesIfNecessary() };
                }
                self.last_refresh = Instant::now();
            }
            self.store.as_ref().expect("store set above")
        }

        fn reset(&mut self) {
            self.store = None;
        }
    }

    type Job = Box<dyn FnOnce(&mut Worker) + Send>;

    fn sender() -> &'static Sender<Job> {
        static TX: OnceLock<Sender<Job>> = OnceLock::new();
        TX.get_or_init(|| {
            let (tx, rx) = unbounded::<Job>();
            std::thread::Builder::new()
                .name("milkrice-eventkit".into())
                .spawn(move || {
                    let mut w = Worker { store: None, store_authorized: false, last_refresh: Instant::now() };
                    while let Ok(job) = rx.recv() {
                        // Never let a panic kill the worker thread.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut w)));
                    }
                })
                .expect("spawn eventkit thread");
            tx
        })
    }

    fn run<R: Send + 'static>(
        timeout: Duration,
        f: impl FnOnce(&mut Worker) -> R + Send + 'static,
    ) -> Result<R> {
        let (rtx, rrx) = bounded(1);
        sender()
            .send(Box::new(move |w| {
                let _ = rtx.send(f(w));
            }))
            .map_err(|_| anyhow!("eventkit thread gone"))?;
        rrx.recv_timeout(timeout).map_err(|_| anyhow!("timed out waiting for EventKit"))
    }

    pub fn request_access() -> Result<bool> {
        if auth_status() == CalendarAuth::Authorized {
            return Ok(true);
        }
        run(Duration::from_secs(125), |w| {
            // Use a fresh store for the request; the old one may predate the grant.
            w.reset();
            let (tx, rx) = bounded::<bool>(1);
            let block = RcBlock::new(move |granted: Bool, err: *mut NSError| {
                if !err.is_null() {
                    // SAFETY: EventKit passes a valid NSError (or null) for the duration of the call.
                    log::warn!("calendar access error: {}", unsafe { &*err }.localizedDescription());
                }
                let _ = tx.try_send(granted.as_bool());
            });
            let store = w.store();
            // SAFETY: the block is a valid, retained block that EventKit copies if it
            // keeps it; its signature matches (BOOL, NSError*).
            unsafe {
                store.requestFullAccessToEventsWithCompletion(&*block as *const _ as *mut _);
            }
            let r = rx.recv_timeout(Duration::from_secs(120));
            log::info!("calendar access result: {r:?}");
            let granted = r.unwrap_or(false);
            // Force a fresh store on next use so it sees the newly granted calendars.
            w.reset();
            granted
        })
    }

    fn to_utc(d: &NSDate) -> DateTime<Utc> {
        let secs = d.timeIntervalSince1970();
        Utc.timestamp_opt(secs.floor() as i64, ((secs - secs.floor()) * 1e9) as u32)
            .single()
            .unwrap_or_default()
    }

    fn to_nsdate(d: DateTime<Utc>) -> Retained<NSDate> {
        NSDate::dateWithTimeIntervalSince1970(d.timestamp() as f64)
    }

    fn calendar_hex(cal: &EKCalendar) -> Option<String> {
        // SAFETY: simple property getters; the NSColor conversion yields an sRGB color
        // on which getRed:green:blue:alpha: is valid, with valid out-pointers.
        unsafe {
            let color: Retained<NSColor> = cal.color();
            let rgb = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
            let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
            rgb.getRed_green_blue_alpha(&mut r, &mut g, &mut b, &mut a);
            let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            Some(format!("#{:02x}{:02x}{:02x}", c(r), c(g), c(b)))
        }
    }

    fn participant(p: &EKParticipant) -> Attendee {
        // SAFETY: simple property getters on a live participant.
        let (url, name, is_self) = unsafe {
            (
                p.URL().absoluteString().map(|s| s.to_string()),
                p.name().map(|s| s.to_string()),
                p.isCurrentUser(),
            )
        };
        let email = url.and_then(|u| {
            let lower = u.to_ascii_lowercase();
            lower.starts_with("mailto:").then(|| u["mailto:".len()..].trim().to_string())
        });
        let email = email.filter(|e| e.contains('@'));
        let name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .or_else(|| email.as_ref().map(|e| e.split('@').next().unwrap_or(e).to_string()))
            .unwrap_or_else(|| "Unknown".into());
        Attendee { name, email, is_self }
    }

    fn convert(ev: &EKEvent) -> Option<CalEvent> {
        // SAFETY: simple property getters on a live EKEvent.
        unsafe {
            let start = to_utc(&ev.startDate());
            let end = to_utc(&ev.endDate());
            let mut attendees: Vec<Attendee> = Vec::new();
            if let Some(list) = ev.attendees() {
                for p in list.iter() {
                    if p.isCurrentUser() && p.participantStatus() == EKParticipantStatus::Declined {
                        return None;
                    }
                    attendees.push(participant(&p));
                }
            }
            if let Some(org) = ev.organizer() {
                let o = participant(&org);
                let dup = attendees.iter().any(|a| {
                    (a.email.is_some() && a.email.as_deref().map(str::to_lowercase) == o.email.as_deref().map(str::to_lowercase))
                        || (a.email.is_none() && a.name == o.name)
                });
                if !dup {
                    attendees.insert(0, o);
                }
            }
            let location = ev.location().map(|s| s.to_string()).filter(|s| !s.trim().is_empty());
            let url = ev.URL().and_then(|u| u.absoluteString()).map(|s| s.to_string());
            let notes = ev.notes().map(|s| s.to_string());
            let meeting_url = [url.as_deref(), location.as_deref(), notes.as_deref()]
                .into_iter()
                .flatten()
                .find_map(find_meeting_url);
            let cal = ev.calendar();
            let id = format!(
                "{}@{}",
                ev.eventIdentifier().map(|s| s.to_string()).unwrap_or_default(),
                start.timestamp()
            );
            Some(CalEvent {
                id,
                title: ev.title().to_string(),
                start,
                end,
                attendees,
                location,
                meeting_url,
                calendar: cal.as_ref().map(|c| c.title().to_string()).unwrap_or_default(),
                calendar_color: cal.as_ref().and_then(|c| calendar_hex(c)),
                all_day: ev.isAllDay(),
                recurring: ev.hasRecurrenceRules(),
            })
        }
    }

    pub fn events_between(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<CalEvent>> {
        if auth_status() != CalendarAuth::Authorized {
            return Err(anyhow!("calendar access not authorized"));
        }
        run(Duration::from_secs(30), move |w| {
            let store = w.store();
            let (s, e) = (to_nsdate(start), to_nsdate(end));
            // SAFETY: valid NSDate arguments; nil calendars means all calendars.
            let events = unsafe {
                let pred = store.predicateForEventsWithStartDate_endDate_calendars(&s, &e, None);
                store.eventsMatchingPredicate(&pred)
            };
            let mut out: Vec<CalEvent> = events.iter().filter_map(|ev| convert(&ev)).collect();
            out.sort_by_key(|e| (e.start, e.end));
            out
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::CalendarAuth;
    use crate::types::CalEvent;
    use anyhow::Result;
    use chrono::{DateTime, Utc};

    pub fn auth_status() -> CalendarAuth {
        CalendarAuth::Unsupported
    }
    pub fn request_access() -> Result<bool> {
        Ok(false)
    }
    pub fn events_between(_s: DateTime<Utc>, _e: DateTime<Utc>) -> Result<Vec<CalEvent>> {
        Ok(Vec::new())
    }
}
