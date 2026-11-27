# Review: PR #1 — Capture iPhone/FaceTime call audio via Core Audio process tap; 48 kHz kept recordings

Branch `claude/project-thread-vho18q` (5 commits, base `5291d7d`). Reviewed the full diff against `main`.

**What ran on Linux:** `milkrice-core` unit tests on this branch (with the local-only build tweaks described in the cloud/reviews-and-tests PR): `audio::resample::*` 3/3 pass, `audio_rec::records_mixes_and_encodes` and `removes_call_audio_leaking_into_the_mic` pass (AEC3 does cancel the synthetic echo), and `gap_on_one_side_keeps_sides_in_step` fails only because `/usr/bin/afconvert` does not exist on Linux. `tap.rs` is macOS-only and was **not compiled**; everything below about it is from reading the code against Apple's Core Audio headers.

**Merge:** merges cleanly with #2. Against #3 the only conflict is `CHANGELOG.md` (both add an Unreleased bullet at the top). No semantic conflicts found: #1 adds `AudioFrame::hires` and #3 adds `CalEvent::recurring`, and neither PR builds the other's struct.

## Findings (most severe first)

### 1. High: existing users lose call audio on their first recording after updating (silent tap, no fallback)
`crates/milkrice-core/src/audio/tap.rs:322` and `crates/milkrice-core/src/audio/mod.rs:146`

`tap::start` refuses to run only when the permission is `Denied`. For everyone upgrading, the System Audio Recording permission was never requested, so it is `Unknown`, and the tap path is taken. When the tap is created before the user has answered, macOS shows the prompt and **delivers silence, not an error** (the module doc comment says this too). Buffers keep arriving, so neither `IO_STALL` nor the session watchdog fires, and there is no fallback to ScreenCaptureKit, which these users already granted. Result: the other participants are missing from the transcript (and the kept recording) for that whole meeting. If the user clicks "Don't allow", it stays silent for the rest of the session.

**Fix:** use the tap only when `permission() == Granted`. Otherwise use ScreenCaptureKit, and request the tap permission from onboarding/Settings (or in the background when no recording is running). `system_audio_permitted()` (mod.rs:165) is fine as it is.

### 2. High: the aggregate's input buffer list can include the output device's own input streams, which get mixed into the "call" side
`tap.rs:223-231` (`io_proc`) and `tap.rs:282` (sub-device list)

The aggregate is built with the default **output** device as a sub-device. When that device also has input streams (USB/Bluetooth headsets, AirPods in HFP mode, many docks and audio interfaces), the aggregate's input `AudioBufferList` holds the sub-device's input buffers **as well as** the tap's buffer. `io_proc` treats "more than one buffer" as the planes of one de-interleaved signal (`downmix_planar`). That:
- mixes the user's own headset mic into the System side, so the user is transcribed and diarized as "the other people";
- averages an interleaved 2-channel tap buffer as if it were a mono plane (`downmix_planar` only reads `byte_size/4` samples per buffer and ignores `mNumberChannels`), which garbles the audio;
- truncates to the shortest buffer (`downmix_planar` uses the min length), so data is lost when buffers differ in size.

**Fix:** after creating the aggregate, read `kAudioDevicePropertyStreamConfiguration` (input scope) and work out which buffer indices belong to the tap (tap streams come after the sub-devices' streams), then downmix only those, honouring each buffer's `mNumberChannels`. Or check `buffers.len()` against the tap's `channels_per_frame`/non-interleaved flag and bail to ScreenCaptureKit when the layout is unexpected.

### 3. High: restarting after "output device changed" runs two taps at once, so call audio is duplicated
`tap.rs:369-372` together with `session.rs:147` (`Restarter::restart`, unchanged code)

When the default output changes (connecting AirPods, plugging in a display with speakers), the worker reports a capture error. The watchdog then starts a **new** system capture, swaps it in, and stops the old one on a background thread. With ScreenCaptureKit the old stream was already dead. Here the old tap is a *global* tap that is still running and still delivering, so for a while (or until teardown finishes) both taps send `AudioSource::System` frames into the same channel. That causes:
- every utterance transcribed twice or garbled;
- in `AudioRecorder`, the System queue growing twice as fast, which trips `MAX_SKEW` padding on the mic side and shifts the recording.

The user also sees "Audio from the other people on the call stopped … Reconnecting…" every time they put AirPods on, even though nothing was lost.

**Fix:** once the tap has decided it must be rebuilt, stop forwarding frames from it (set a flag the worker checks before `tx.send`). Better: rebuild the aggregate inside `tap.rs` without going through the error/warning path. A planned device switch shouldn't show `CAPTURE_LOST`.

### 4. Medium: the real-time IO callback allocates, and can wake a thread
`tap.rs:223` (`Vec::new()` and the growth inside `downmix_*`), `tap.rs:228` (`Vec<&[f32]>` of planes), `tap.rs:234` (`try_send`)

`io_proc` runs on Core Audio's real-time IO thread. Each callback does one or two heap allocations, and `crossbeam_channel::try_send` can futex-wake the receiver. Allocation on the IO thread can take a lock held by another thread and cause overloads (dropouts in the tap and the aggregate's other sub-device). This is the classic Core Audio rule violation.

**Fix:** use a preallocated lock-free single-producer/single-consumer ring buffer (`rtrb` or `ringbuf`) of f32 samples. Downmix straight into it with no temporary `Vec`, and let the worker poll it, as it already does with `recv_timeout(250ms)`.

### 5. Medium: dropped tap buffers shift the call side for the rest of the recording
`tap.rs:234` (`let _ = tx.try_send(mono)`) together with `audio_rec.rs:59,127` (`GAP = 400 ms`)

When the 256-slot channel is full, a buffer is silently dropped. The recorder pads only gaps longer than 400 ms, so a shorter run of drops makes all later call audio play early relative to the mic. I checked this on Linux with a throwaway test against this branch: dropping three 100 ms System writes at t = 1.0 s moved a call burst sent at t = 2.0 s to t ≈ 1.8 s in the output. The transcript timestamps for the System side drift the same way.

**Fix:** count dropped frames in the callback (an atomic counter) and have the worker insert the same number of zero samples, so the System stream keeps its sample count. Fixing #4 with a ring buffer fixes this too.

### 6. Medium: in-app playback is captured during a live recording
`tap.rs:148` (`own_process_object`)

Only Milkrice's own PID is excluded. Audio from `<audio>`/`new Audio()` in the WKWebView plays in the separate `com.apple.WebKit.WebContent` process, and this PR adds a playback timeline that makes replaying likely. If the user replays an old meeting while a recording is running, it lands in the "others" transcript. Also, the own-process object only exists once the process has used audio, so the exclusion is often empty on the first recording.

**Fix:** also exclude WebKit helper processes whose responsible PID is ours (look up `kAudioHardwarePropertyProcessObjectList`, then `kAudioProcessPropertyPID` and the bundle ID), or pause the shared `player` when a recording starts.

### 7. Medium: the fallback `peaks()` decoder gives wrong per-side outlines for new recordings
`audio_rec.rs:295` (`if i % ch == 0 { a += … }`)

New files mix both voices into both channels (`MIC_PAN`/`CALL_PAN`), but when `<offset>.peaks.json` is missing (deleted, or files copied or synced without it) the fallback still reads left as "mic" and right as "call". The timeline then shows both people talking all the time. It is only correct for recordings made before this PR.

**Fix:** treat the outline file as required for new-format recordings (for example, write a format/version marker into peaks.json and don't fall back for new files), or note in the code that the fallback is legacy-only and show a single combined lane when it is used.

### 8. Low: `KEEP_RECORDING_AUDIO` is process-global and never reset
`audio/mod.rs:16`, `session.rs:205`

The flag is set when a session starts and never cleared. Later captures that aren't part of a session (the Settings mic-level preview `mic_test`, `probe_mics`) keep running the 48 kHz resampler for nothing. A future second concurrent capture would also race on it. **Fix:** pass `hires: bool` explicitly through `start_mic_watched` and `start_system_watched` (the `Restarter` already has the session config), or reset the flag in `Session::stop`.

### 9. Low: the `report` latch hides later, different failures
`tap.rs:331`. After the first report (for example a 2 s stall that later recovers), a real "output device changed" is never reported. **Fix:** latch per reason, or reset `reported` once buffers flow again.

### 10. Low: `write_sample` errors are ignored
`audio_rec.rs:169-170`. On a full disk the recording silently stops growing and `finish` still reports success. **Fix:** remember the first error, stop writing, and surface it from `finish`.

### 11. Low (UI performance): `drawTimeline` forces style recalculation on every `timeupdate` and during drags
`ui/app.js:944` (`resolveColor` inserts an element and calls `getComputedStyle`), `:967`, `:1000`, `:1057`. The colour cache is rebuilt on every draw, and `speakerAt` scans every segment for every 4 px bar (O(width/4 × segments) per frame; 2-hour meetings have thousands of segments). **Fix:** cache resolved colours across draws (invalidate on theme change), and walk the segments with a moving index because both lists are sorted.

### Notes / verified OK
- `Tap::drop` order (stop, destroy IOProc, destroy aggregate, destroy tap, free the boxed `Sender`) is correct. `AudioDeviceStop` called off the IO thread is synchronous, so the callback can't use the freed sender.
- Error paths in `create()` clean up through `Drop`.
- The `device_uid` +1 CFString is adopted correctly with `Retained::from_raw`.
- `TCCAccessPreflight` return codes (0 = granted, 1 = denied, otherwise unknown) match Apple's AudioCap sample. This is private SPI, so record that in SECURITY/notes in case App Store distribution is ever considered.
- Using the aggregate's nominal rate rather than the tap's rate is right when drift compensation is on.
- The 48 kHz `hires` stream and the 16 kHz stream come from separate resamplers with different delays (a few ms). That is fine for listening, but don't use `hires` for anything that must line up exactly with transcript timestamps.

---
_Generated by [Claude Code](https://claude.ai/code)_
