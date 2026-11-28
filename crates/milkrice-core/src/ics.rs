//! Read-only calendar subscriptions in iCalendar format (e.g. Google Calendar's
//! "Secret address in iCal format"). No sign-in, no OAuth app, no write access: the user pastes
//! one private, revocable URL for one calendar.
use crate::types::{Attendee, CalEvent};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use std::collections::HashMap;
use std::time::Duration;

/// Fetch an .ics feed. `webcal://` is accepted and treated as https.
pub fn fetch(url: &str) -> Result<String> {
    let url = url.trim();
    let url = if let Some(rest) = url.strip_prefix("webcal://") { format!("https://{rest}") } else { url.to_string() };
    if !url.starts_with("https://") {
        bail!("calendar address must start with https:// or webcal://");
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .https_only(true)
        .user_agent("Milkrice/0.1 (local meeting notes)")
        .build()?;
    // A feed address is a secret (e.g. Google's private iCal link). reqwest errors include the
    // URL, and these errors end up in milkrice.log and the UI, so strip it from every one.
    let resp = client.get(&url).send().map_err(|e| e.without_url()).context("fetching calendar")?;
    if !resp.status().is_success() {
        bail!("calendar fetch failed: HTTP {}", resp.status());
    }
    let body = resp.text().map_err(|e| e.without_url()).context("reading calendar")?;
    if !body.contains("BEGIN:VCALENDAR") {
        bail!("that address didn't return an iCal calendar");
    }
    Ok(body)
}

#[derive(Debug, Clone, Default)]
struct Prop {
    name: String,
    params: HashMap<String, String>,
    value: String,
}

/// Unfold continuation lines and split into properties.
fn props(text: &str) -> Vec<Prop> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(&raw[1..]);
        } else if !raw.is_empty() {
            lines.push(raw.to_string());
        }
    }
    lines
        .into_iter()
        .filter_map(|l| {
            // Split name/params from value at the first ':' that's not inside quotes.
            let mut in_q = false;
            let mut split = None;
            for (i, c) in l.char_indices() {
                match c {
                    '"' => in_q = !in_q,
                    ':' if !in_q => {
                        split = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let i = split?;
            let (head, value) = (&l[..i], &l[i + 1..]);
            let mut parts = head.split(';');
            let name = parts.next()?.to_ascii_uppercase();
            let params = parts
                .filter_map(|p| p.split_once('='))
                .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_string()))
                .collect();
            Some(Prop { name, params, value: value.to_string() })
        })
        .collect()
}

fn unescape(s: &str) -> String {
    s.replace("\\n", "\n").replace("\\N", "\n").replace("\\,", ",").replace("\\;", ";").replace("\\\\", "\\")
}

/// Parse DTSTART/DTEND-style values into UTC. Returns (instant, all_day).
fn parse_dt(p: &Prop) -> Option<(DateTime<Utc>, bool)> {
    let v = p.value.trim();
    if p.params.get("VALUE").map(|v| v == "DATE").unwrap_or(false) || v.len() == 8 {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        return Some((Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?), true));
    }
    if let Some(z) = v.strip_suffix('Z') {
        let n = NaiveDateTime::parse_from_str(z, "%Y%m%dT%H%M%S").ok()?;
        return Some((Utc.from_utc_datetime(&n), false));
    }
    let n = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    let tz: chrono_tz::Tz = p
        .params
        .get("TZID")
        .and_then(|t| t.parse().ok())
        .unwrap_or(chrono_tz::Tz::UTC);
    let local = tz.from_local_datetime(&n).earliest()?;
    Some((local.with_timezone(&Utc), false))
}

#[derive(Debug, Clone, Default)]
struct VEvent {
    uid: String,
    summary: String,
    start: Option<(DateTime<Utc>, bool)>,
    end: Option<(DateTime<Utc>, bool)>,
    start_raw: Option<Prop>,
    rrule: Option<String>,
    exdates: Vec<Prop>,
    recurrence_id: Option<DateTime<Utc>>,
    attendees: Vec<Attendee>,
    organizer: Option<Attendee>,
    location: Option<String>,
    description: String,
    url: Option<String>,
    conference: Option<String>,
    cancelled: bool,
    declined_by_owner: bool,
}

fn person(p: &Prop) -> Attendee {
    let email = p.value.trim().strip_prefix("mailto:").or_else(|| p.value.strip_prefix("MAILTO:")).map(String::from);
    let name = p
        .params
        .get("CN")
        .cloned()
        .filter(|n| !n.is_empty() && !n.contains('@'))
        .or_else(|| email.as_ref().map(|e| e.split('@').next().unwrap_or(e).to_string()))
        .unwrap_or_default();
    Attendee { name, email, is_self: false }
}

fn parse_events(text: &str, owner_email: Option<&str>) -> Vec<VEvent> {
    let mut out = Vec::new();
    let mut cur: Option<VEvent> = None;
    let mut depth = 0; // skip nested components like VALARM
    for p in props(text) {
        match (p.name.as_str(), p.value.as_str()) {
            ("BEGIN", "VEVENT") => {
                cur = Some(VEvent::default());
                depth = 0;
            }
            ("BEGIN", _) if cur.is_some() => depth += 1,
            ("END", "VEVENT") => {
                if let Some(e) = cur.take() {
                    out.push(e);
                }
            }
            ("END", _) if cur.is_some() && depth > 0 => depth -= 1,
            _ => {
                let Some(e) = cur.as_mut() else { continue };
                if depth > 0 {
                    continue;
                }
                match p.name.as_str() {
                    "UID" => e.uid = p.value.clone(),
                    "SUMMARY" => e.summary = unescape(&p.value),
                    "DTSTART" => {
                        e.start = parse_dt(&p);
                        e.start_raw = Some(p.clone());
                    }
                    "DTEND" => e.end = parse_dt(&p),
                    "RRULE" => e.rrule = Some(p.value.clone()),
                    "EXDATE" => e.exdates.push(p.clone()),
                    "RECURRENCE-ID" => e.recurrence_id = parse_dt(&p).map(|x| x.0),
                    "STATUS" => e.cancelled = p.value.eq_ignore_ascii_case("CANCELLED"),
                    "LOCATION" => e.location = Some(unescape(&p.value)).filter(|s| !s.is_empty()),
                    "DESCRIPTION" => e.description = unescape(&p.value),
                    "URL" => e.url = Some(p.value.clone()),
                    "X-GOOGLE-CONFERENCE" => e.conference = Some(p.value.clone()),
                    "ORGANIZER" => e.organizer = Some(person(&p)),
                    "ATTENDEE" => {
                        let mut a = person(&p);
                        let me = owner_email
                            .zip(a.email.as_deref())
                            .map(|(o, e)| o.eq_ignore_ascii_case(e))
                            .unwrap_or(false);
                        if me {
                            a.is_self = true;
                            if p.params.get("PARTSTAT").map(|s| s == "DECLINED").unwrap_or(false) {
                                e.declined_by_owner = true;
                            }
                        }
                        e.attendees.push(a);
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// Line with params for the rrule crate, e.g. "DTSTART;TZID=Europe/Paris:20261101T094500".
fn prop_line(p: &Prop) -> String {
    let mut s = p.name.clone();
    for (k, v) in &p.params {
        if k == "TZID" || k == "VALUE" {
            s.push_str(&format!(";{k}={v}"));
        }
    }
    format!("{s}:{}", p.value)
}

fn occurrences(e: &VEvent, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    let Some((start, _)) = e.start else { return vec![] };
    let (Some(rule), Some(sp)) = (&e.rrule, &e.start_raw) else {
        return vec![start];
    };
    let mut spec = format!("{}\nRRULE:{}", prop_line(sp), rule);
    for ex in &e.exdates {
        spec.push('\n');
        spec.push_str(&prop_line(ex));
    }
    let set: rrule::RRuleSet = match spec.parse() {
        Ok(s) => s,
        Err(err) => {
            log::debug!("rrule '{spec}': {err}");
            return vec![start];
        }
    };
    let tz = rrule::Tz::UTC;
    set.after(from.with_timezone(&tz) - chrono::Duration::days(1))
        .before(to.with_timezone(&tz))
        .all(400)
        .dates
        .into_iter()
        .map(|d| d.with_timezone(&Utc))
        .collect()
}

/// Expand an iCal feed into concrete events overlapping [from, to).
pub fn parse(text: &str, calendar_name: &str, owner_email: Option<&str>, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<CalEvent> {
    let events = parse_events(text, owner_email);
    // One-off edits of recurring events replace the generated instance.
    let overrides: HashMap<(String, i64), &VEvent> = events
        .iter()
        .filter_map(|e| e.recurrence_id.map(|r| ((e.uid.clone(), r.timestamp()), e)))
        .collect();
    let mut out = Vec::new();
    for e in &events {
        if e.recurrence_id.is_some() {
            continue; // handled when expanding its parent
        }
        let Some((start, all_day)) = e.start else { continue };
        let dur = e.end.map(|(end, _)| end - start).unwrap_or_else(|| chrono::Duration::minutes(if all_day { 1440 } else { 30 }));
        for occ in occurrences(e, from, to) {
            let inst = overrides.get(&(e.uid.clone(), occ.timestamp())).copied().unwrap_or(e);
            let (s, ad) = inst.start.unwrap_or((occ, all_day));
            let s = if std::ptr::eq(inst, e) { occ } else { s };
            let en = inst.end.map(|x| x.0).filter(|_| !std::ptr::eq(inst, e)).unwrap_or(s + dur);
            if inst.cancelled || inst.declined_by_owner || en <= from || s >= to {
                continue;
            }
            let mut attendees = inst.attendees.clone();
            if let Some(o) = &inst.organizer {
                if !attendees.iter().any(|a| a.email.is_some() && a.email == o.email) {
                    attendees.insert(0, o.clone());
                }
            }
            let meeting_url = inst
                .conference
                .clone()
                .or_else(|| inst.url.as_deref().and_then(crate::calendar::find_meeting_url))
                .or_else(|| inst.location.as_deref().and_then(crate::calendar::find_meeting_url))
                .or_else(|| crate::calendar::find_meeting_url(&inst.description));
            out.push(CalEvent {
                id: format!("ics:{}@{}", inst.uid, s.timestamp()),
                title: inst.summary.clone(),
                start: s,
                end: en,
                attendees,
                location: inst.location.clone(),
                meeting_url,
                calendar: calendar_name.to_string(),
                calendar_color: None,
                all_day: ad,
            });
        }
    }
    out.sort_by_key(|e| e.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:pilates1\r\nSUMMARY:Pilates (online)\r\nDTSTART;TZID=Australia/Melbourne:20261101T094500\r\nDTEND;TZID=Australia/Melbourne:20261101T104500\r\nRRULE:FREQ=WEEKLY;BYDAY=SU\r\nEXDATE;TZID=Australia/Melbourne:20261108T094500\r\nORGANIZER;CN=Ana Lima:mailto:ana@example.com\r\nATTENDEE;CN=Alex;PARTSTAT=ACCEPTED:mailto:me@example.com\r\nX-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:pilates1\r\nRECURRENCE-ID;TZID=Australia/Melbourne:20261115T094500\r\nSUMMARY:Pilates (moved)\r\nDTSTART;TZID=Australia/Melbourne:20261115T110000\r\nDTEND;TZID=Australia/Melbourne:20261115T120000\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:sync2\r\nSUMMARY:Design sync\\, weekly\r\nDTSTART:20261103T230000Z\r\nDTEND:20261103T233000Z\r\nDESCRIPTION:Join: https://zoom.us/j/123?pwd=abc\\nThanks\r\nATTENDEE;CN=Priya;PARTSTAT=ACCEPTED:mailto:priya@example.com\r\nATTENDEE;PARTSTAT=DECLINED:mailto:me@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn expands_recurrence_exdate_and_override() {
        let from = Utc.with_ymd_and_hms(2026, 10, 31, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap();
        let evs = parse(FEED, "Google", Some("me@example.com"), from, to);
        let pil: Vec<_> = evs.iter().filter(|e| e.title.starts_with("Pilates")).collect();
        // Nov 1, (Nov 8 excluded), Nov 15 moved to 11:00
        assert_eq!(pil.len(), 2, "{pil:#?}");
        assert_eq!(pil[0].start, Utc.with_ymd_and_hms(2026, 10, 31, 22, 45, 0).unwrap());
        assert_eq!(pil[1].title, "Pilates (moved)");
        assert_eq!(pil[1].start, Utc.with_ymd_and_hms(2026, 11, 15, 0, 0, 0).unwrap());
        assert_eq!(pil[0].meeting_url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert!(pil[0].attendees.iter().any(|a| a.name == "Ana Lima"));
        assert!(pil[0].attendees.iter().any(|a| a.is_self));
        // Declined by the calendar owner -> skipped.
        assert!(!evs.iter().any(|e| e.title.contains("Design sync")));
        // Without an owner email the declined event is kept and its Zoom link found.
        let all = parse(FEED, "Google", None, from, to);
        let d = all.iter().find(|e| e.title == "Design sync, weekly").unwrap();
        assert_eq!(d.meeting_url.as_deref(), Some("https://zoom.us/j/123?pwd=abc"));
    }
}
