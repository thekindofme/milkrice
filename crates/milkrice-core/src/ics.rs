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
        .user_agent("Milkrice/0.1 (local meeting notes)")
        .build()?;
    let resp = client.get(&url).send().context("fetching calendar")?;
    if !resp.status().is_success() {
        bail!("calendar fetch failed: HTTP {}", resp.status());
    }
    let body = resp.text()?;
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

/// Edge cases that bite real calendars: DST, all-day and multi-day events, RRULE variants,
/// exceptions, unicode.
#[cfg(test)]
mod edge_tests {
    use super::*;

    fn feed(events: &[&str]) -> String {
        let mut s = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
        for e in events {
            s.push_str("BEGIN:VEVENT\r\n");
            for line in e.lines() {
                s.push_str(line.trim_start());
                s.push_str("\r\n");
            }
            s.push_str("END:VEVENT\r\n");
        }
        s.push_str("END:VCALENDAR\r\n");
        s
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn weekly_series_keeps_local_time_across_dst_end() {
        // 09:00 New York every Friday; DST ends Sun 1 Nov 2026 (EDT -4 -> EST -5).
        let f = feed(&["UID:standup
            SUMMARY:Standup
            DTSTART;TZID=America/New_York:20261023T090000
            DTEND;TZID=America/New_York:20261023T091500
            RRULE:FREQ=WEEKLY;BYDAY=FR"]);
        let evs = parse(&f, "Work", None, utc(2026, 10, 22, 0, 0), utc(2026, 11, 14, 0, 0));
        let starts: Vec<_> = evs.iter().map(|e| e.start).collect();
        assert_eq!(starts, vec![utc(2026, 10, 23, 13, 0), utc(2026, 10, 30, 13, 0), utc(2026, 11, 6, 14, 0), utc(2026, 11, 13, 14, 0)]);
        assert!(evs.iter().all(|e| e.end - e.start == chrono::Duration::minutes(15)));
        // Occurrence ids are distinct and share the series prefix.
        assert!(evs.iter().all(|e| e.id.starts_with("ics:standup@")));
        let mut ids: Vec<_> = evs.iter().map(|e| e.id.clone()).collect();
        ids.dedup();
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn weekly_series_keeps_local_time_across_dst_start_southern_hemisphere() {
        // Sydney DST starts Sun 4 Oct 2026 (AEST +10 -> AEDT +11).
        let f = feed(&["UID:syd
            SUMMARY:Planning
            DTSTART;TZID=Australia/Sydney:20260928T100000
            DTEND;TZID=Australia/Sydney:20260928T110000
            RRULE:FREQ=WEEKLY"]);
        let evs = parse(&f, "Work", None, utc(2026, 9, 27, 0, 0), utc(2026, 10, 7, 0, 0));
        let starts: Vec<_> = evs.iter().map(|e| e.start).collect();
        assert_eq!(starts, vec![utc(2026, 9, 28, 0, 0), utc(2026, 10, 4, 23, 0)]);
    }

    #[test]
    fn event_in_the_spring_forward_gap_is_not_dropped() {
        // 02:30 on 8 Mar 2026 does not exist in New York (clocks jump 02:00 -> 03:00).
        // Calendar apps show it at 03:30 EDT; it must not vanish from the feed.
        let f = feed(&["UID:gap
            SUMMARY:Night shift handover
            DTSTART;TZID=America/New_York:20260308T023000
            DTEND;TZID=America/New_York:20260308T040000"]);
        let evs = parse(&f, "Work", None, utc(2026, 3, 7, 0, 0), utc(2026, 3, 9, 0, 0));
        assert_eq!(evs.len(), 1, "event at a non-existent local time was dropped");
        assert_eq!(evs[0].start, utc(2026, 3, 8, 7, 30));
        assert_eq!(evs[0].end, utc(2026, 3, 8, 8, 0));
    }

    #[test]
    fn ambiguous_fall_back_time_uses_the_first_occurrence() {
        // 01:30 on 1 Nov 2026 happens twice in New York; RFC 5545 says use the first (EDT).
        let f = feed(&["UID:amb
            SUMMARY:Early call
            DTSTART;TZID=America/New_York:20261101T013000
            DTEND;TZID=America/New_York:20261101T014500"]);
        let evs = parse(&f, "Work", None, utc(2026, 10, 31, 0, 0), utc(2026, 11, 2, 0, 0));
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].start, utc(2026, 11, 1, 5, 30));
    }

    #[test]
    fn all_day_and_multi_day_events() {
        let f = feed(&[
            "UID:holiday
            SUMMARY:Public holiday
            DTSTART;VALUE=DATE:20261126
            DTEND;VALUE=DATE:20261127",
            "UID:offsite
            SUMMARY:Team offsite
            DTSTART;VALUE=DATE:20261123
            DTEND;VALUE=DATE:20261126",
            "UID:noend
            SUMMARY:Reminder day
            DTSTART;VALUE=DATE:20261125",
        ]);
        let evs = parse(&f, "Work", None, utc(2026, 11, 24, 0, 0), utc(2026, 11, 30, 0, 0));
        let get = |t: &str| evs.iter().find(|e| e.title == t).unwrap_or_else(|| panic!("{t} missing: {evs:#?}"));
        let h = get("Public holiday");
        assert!(h.all_day);
        assert_eq!(h.end - h.start, chrono::Duration::days(1));
        // Started before the window but still running: kept.
        let o = get("Team offsite");
        assert!(o.all_day);
        assert_eq!(o.end - o.start, chrono::Duration::days(3));
        // No DTEND on a date event: lasts the day (RFC 5545 3.6.1).
        let r = get("Reminder day");
        assert!(r.all_day);
        assert_eq!(r.end - r.start, chrono::Duration::days(1));
    }

    #[test]
    fn yearly_all_day_series() {
        let f = feed(&["UID:bday
            SUMMARY:Ana's birthday
            DTSTART;VALUE=DATE:20200214
            DTEND;VALUE=DATE:20200215
            RRULE:FREQ=YEARLY"]);
        let evs = parse(&f, "Personal", None, utc(2027, 2, 1, 0, 0), utc(2027, 3, 1, 0, 0));
        assert_eq!(evs.len(), 1, "{evs:#?}");
        assert!(evs[0].all_day);
        assert_eq!(evs[0].start, utc(2027, 2, 14, 0, 0));
    }

    #[test]
    fn until_count_and_exdate_in_utc() {
        let f = feed(&[
            "UID:count
            SUMMARY:Onboarding
            DTSTART:20261102T150000Z
            DTEND:20261102T160000Z
            RRULE:FREQ=DAILY;COUNT=3",
            "UID:until
            SUMMARY:Sprint review
            DTSTART;TZID=Europe/Berlin:20261102T140000
            DTEND;TZID=Europe/Berlin:20261102T150000
            RRULE:FREQ=WEEKLY;UNTIL=20261116T130000Z
            EXDATE;TZID=Europe/Berlin:20261109T140000",
        ]);
        let evs = parse(&f, "Work", None, utc(2026, 11, 1, 0, 0), utc(2026, 12, 31, 0, 0));
        assert_eq!(evs.iter().filter(|e| e.title == "Onboarding").count(), 3);
        let sr: Vec<_> = evs.iter().filter(|e| e.title == "Sprint review").map(|e| e.start).collect();
        assert_eq!(sr, vec![utc(2026, 11, 2, 13, 0), utc(2026, 11, 16, 13, 0)]);
    }

    #[test]
    fn until_as_a_date_with_a_timed_start_still_expands() {
        // Seen in exported feeds: UNTIL given as a DATE while DTSTART is a DATE-TIME. Strictly
        // invalid, but every calendar app treats it as "through that day" rather than
        // showing only the first meeting.
        let f = feed(&["UID:loose
            SUMMARY:Office hours
            DTSTART;TZID=Europe/London:20261103T100000
            DTEND;TZID=Europe/London:20261103T110000
            RRULE:FREQ=WEEKLY;UNTIL=20261117"]);
        let evs = parse(&f, "Work", None, utc(2026, 11, 1, 0, 0), utc(2026, 12, 31, 0, 0));
        let starts: Vec<_> = evs.iter().map(|e| e.start).collect();
        assert_eq!(starts, vec![utc(2026, 11, 3, 10, 0), utc(2026, 11, 10, 10, 0), utc(2026, 11, 17, 10, 0)]);
    }

    #[test]
    fn until_as_a_date_time_with_an_all_day_start_still_expands() {
        let f = feed(&["UID:allday-until
            SUMMARY:Focus day
            DTSTART;VALUE=DATE:20261103
            DTEND;VALUE=DATE:20261104
            RRULE:FREQ=WEEKLY;UNTIL=20261117T000000Z"]);
        let evs = parse(&f, "Work", None, utc(2026, 11, 1, 0, 0), utc(2026, 12, 31, 0, 0));
        assert_eq!(evs.len(), 3, "{evs:#?}");
        assert!(evs.iter().all(|e| e.all_day));
    }

    #[test]
    fn cancelled_occurrence_override_is_skipped() {
        let f = feed(&[
            "UID:wk
            SUMMARY:1:1
            DTSTART:20261102T090000Z
            DTEND:20261102T093000Z
            RRULE:FREQ=WEEKLY;COUNT=3",
            "UID:wk
            RECURRENCE-ID:20261109T090000Z
            SUMMARY:1:1
            STATUS:CANCELLED
            DTSTART:20261109T090000Z
            DTEND:20261109T093000Z",
        ]);
        let evs = parse(&f, "Work", None, utc(2026, 11, 1, 0, 0), utc(2026, 12, 1, 0, 0));
        let starts: Vec<_> = evs.iter().map(|e| e.start).collect();
        assert_eq!(starts, vec![utc(2026, 11, 2, 9, 0), utc(2026, 11, 16, 9, 0)]);
    }

    #[test]
    fn single_invited_occurrence_without_its_series_is_shown() {
        // Invited to one occurrence of someone else's series: Google's feed has only the
        // RECURRENCE-ID instance, not the parent with the RRULE.
        let f = feed(&["UID:foreign-series
            RECURRENCE-ID;TZID=Europe/Paris:20261110T150000
            SUMMARY:Guest talk
            DTSTART;TZID=Europe/Paris:20261110T160000
            DTEND;TZID=Europe/Paris:20261110T170000"]);
        let evs = parse(&f, "Work", None, utc(2026, 11, 1, 0, 0), utc(2026, 12, 1, 0, 0));
        assert_eq!(evs.len(), 1, "orphan occurrence dropped");
        assert_eq!(evs[0].title, "Guest talk");
        assert_eq!(evs[0].start, utc(2026, 11, 10, 15, 0));
    }

    #[test]
    fn unicode_titles_folding_and_escapes() {
        // A folded SUMMARY (continuation line starts with a space) with emoji and CJK.
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u1\r\nSUMMARY:週次定例 🚀 Café\\, Zürich \r\n — Q4 planning\r\nLOCATION:Room \\\\new\r\nDTSTART:20261102T090000Z\r\nDTEND:20261102T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let evs = parse(text, "Work", None, utc(2026, 11, 1, 0, 0), utc(2026, 11, 3, 0, 0));
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].title, "週次定例 🚀 Café, Zürich — Q4 planning");
        // "\\" is an escaped backslash, so "\\new" is a backslash followed by "new", not a newline.
        assert_eq!(evs[0].location.as_deref(), Some("Room \\new"));
    }

    #[test]
    fn lf_only_feed_and_lowercase_names_parse() {
        let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nuid:lf\nsummary:Plain LF\ndtstart:20261102T090000Z\ndtend:20261102T093000Z\nEND:VEVENT\nEND:VCALENDAR\n";
        let evs = parse(text, "Work", None, utc(2026, 11, 1, 0, 0), utc(2026, 11, 3, 0, 0));
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].title, "Plain LF");
    }

    #[test]
    fn events_outside_the_window_and_missing_start_are_ignored() {
        let f = feed(&[
            "UID:past
            SUMMARY:Past
            DTSTART:20261001T090000Z
            DTEND:20261001T100000Z",
            "UID:nostart
            SUMMARY:Broken",
        ]);
        assert!(parse(&f, "Work", None, utc(2026, 11, 1, 0, 0), utc(2026, 11, 3, 0, 0)).is_empty());
        assert!(parse("", "Work", None, utc(2026, 11, 1, 0, 0), utc(2026, 11, 3, 0, 0)).is_empty());
    }
}
