# Review: PR #3 — Coming up: hide a meeting or a whole recurring series, with undo

Branch `claude/project-thread-c72tzo` (1 commit, base `5291d7d`). 13 files, +250/−8.

**What ran:** the `milkrice-core` part (`CalEvent::recurring`, `series_id`) compiles on Linux. The new unit test `state::tests::hides_a_series_or_one_occurrence` lives in `src-tauri`, which needs the macOS/Tauri toolchain, and was **not run** here. EventKit behaviour (`calendar.rs`) was reviewed by reading only. **Merge:** conflicts with #1 only in `CHANGELOG.md`. Merges cleanly with #2.

## Findings (most severe first)

### 1. Medium: the invisible Undo toast keeps catching clicks after it fades
`ui/styles.css:526`: `.toast.has-action { … pointer-events: auto; }`

`toast()` removes `.show` after 6 s (or when Undo is clicked), but `.has-action` stays until the next toast. The faded toast (`opacity:0`, centred at the top of the window) still takes pointer events, so for the rest of the session there is an invisible ~300×40 px dead zone at top-centre. That area overlaps the note title and the window drag region, and an invisible "Undo" button sits in it. Clicking there re-shows nothing, but it **does run `unhideEvent` again**.
**Fix:** `.toast.show.has-action { pointer-events: auto; }` (only while shown), and/or remove the button in the hide timer.

### 2. Medium: "Hide just this one" stops working when the occurrence is moved
`crates/milkrice-core/src/ics.rs:271` (`id: format!("ics:{}@{}", inst.uid, s.timestamp())`), `calendar.rs` (`"{eventIdentifier}@{start}"`), `src-tauri/src/commands.rs:351-357`

A one-off hide is keyed by `CalEvent::id`, which includes the **current start time**. When the organiser moves that single occurrence (an ICS `RECURRENCE-ID` override with a new `DTSTART`, or an EventKit detached occurrence), the id changes and the meeting comes back. The nudge and auto-record come back too, which is what the user explicitly hid.
**Fix:** for ICS, key one-off hides by `uid@<RECURRENCE-ID or original start>`. The original occurrence time is available in `parse()` as `occ`. For EventKit, use `occurrenceDate` (the original date of a detached occurrence) instead of `startDate`. A stable "occurrence key" field on `CalEvent` (separate from the display id) would cover both.

### 3. Medium: EventKit detached occurrences may not offer "Hide every …"
`crates/milkrice-core/src/calendar.rs:290`: `recurring: ev.hasRecurrenceRules()`

Please verify on macOS: for a detached (individually edited) occurrence, `hasRecurrenceRules` can be `false`, so the menu only offers "Hide this meeting" for that one, even though `series_id()` would match the series. A user hiding "Yoga" from its moved occurrence then sees all the others. Use `hasRecurrenceRules() || isDetached()`, or check the series in `hide_event` (more than one event in `state.events` sharing `series_id`).

### 4. Low: an empty `eventIdentifier` turns into a series key that hides every unidentified event
`calendar.rs:276` (`unwrap_or_default()`) → id `"@<ts>"` → `series_id() == ""`. Hiding the series of such an event hides **every** event with no identifier. **Fix:** refuse `series` (or fall back to the per-occurrence key) when `series_id()` is empty.

### 5. Low: lost-update race on settings
`commands.rs:358-366` (`hide_event`) and `:374-379` (`unhide_event`) do `let mut s = state.settings(); …; state.save_settings(s)`, which is a read-modify-write outside the lock. `add_calendar_feed` (async, on a Tokio worker) and `save_settings` do the same, so a hide and a feed add that interleave can drop one of the changes. This follows an existing pattern, but the PR adds two more writers.
**Fix:** add `AppState::update_settings(|s| …)` that changes and persists while holding `settings.lock()`.

### 6. Low: Undo does not restore a "meeting is starting" nudge that the hide dismissed
`commands.rs:363-368`: hiding clears `state.prompt`, but `unhide_event` doesn't bring it back. If a user hides the meeting that is starting right now by mistake and hits Undo within the 6 s, the banner is gone and (in `auto` mode) the recording doesn't start until the next scheduler tick re-evaluates it. Check whether the scheduler re-prompts. `event_flag(&e.id)` was never set for it, so it probably does, but the user loses up to one tick. Consider calling the scheduler tick (or re-emitting the prompt) from `unhide_event`.

### 7. Low: Settings › Calendar list can be stale
`ui/app.js` `bindSettings` calls `renderHiddenSettings()` with whatever `S.hidden` holds. That is filled only by `loadEvents()`, so opening Settings before the first event refresh (or right after hiding from the tray) shows an empty list. Call `invoke("hidden_events")` in `renderHiddenSettings` instead.

### 8. Nit: `series_id` doc comment
The doc comment says every occurrence shares `series_id`. That holds for ICS (`ics:<UID>`), but on EventKit the identifier can change when the event's calendar changes (Apple docs), which also un-hides a series. Worth a line in the doc comment.

### Recurring-series edge cases checked and OK
- Google UIDs containing `@` are handled by `rsplit_once` (covered by the new test).
- EXDATE'd occurrences never appear, so there's nothing to hide.
- Cancelled or declined occurrences are filtered before hiding.
- Series hides never expire. One-off hides are pruned a day after their start, both in `hidden_events` and on the next `hide_event`.
- `Settings` is `#[serde(default)]`, so an old `settings.json` without `hidden_events` still loads.
- `save_settings` from the UI keeps `hidden_events` from the server copy, so a stale UI can't un-hide.
- Scheduler (auto-record and nudge) and tray `next_event` both honour `is_hidden`.
- Same UID in two subscribed feeds: hiding the series hides it in both, which is what users would expect.

---
_Generated by [Claude Code](https://claude.ai/code)_
