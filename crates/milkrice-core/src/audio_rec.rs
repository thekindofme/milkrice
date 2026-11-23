//! Opt-in local recording of a meeting: mic on the left channel, call audio on the right,
//! 48 kHz, saved as AAC (.m4a, ~58 MB/hour) next to the transcript so moments can be replayed.
//! Transcription keeps using its own 16 kHz stream; this one is for listening.
//!
//! Both sides are written as they arrive, kept in step (a gap on one side becomes silence), and
//! the mic side goes through WebRTC's AEC3 echo canceller with the call audio as reference, so
//! the caller isn't heard a second time through your speakers.
use crate::audio::RECORD_RATE;
use crate::types::{AudioSource, SAMPLE_RATE};
use aec3::nodes::audio::AudioFormat;
use aec3::pipelines::linear::{self, LinearPipeline};
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct AudioRecorder {
    base: PathBuf,
    out: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    /// Not yet written, per source (index = `AudioSource as usize`), at `RECORD_RATE`.
    queue: [VecDeque<f32>; 2],
    /// Last sample per source, for upsampling streams that come without a 48 kHz copy.
    last: [f32; 2],
    /// When each source last delivered audio.
    seen: [Option<Instant>; 2],
    /// Removes the call audio that leaks into the mic through the speakers.
    aec: Option<LinearPipeline>,
    mic_frame: Vec<f32>,
    sys_frame: Vec<f32>,
    cleaned: Vec<f32>,
}

fn spec() -> hound::WavSpec {
    hound::WavSpec { channels: 2, sample_rate: RECORD_RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }
}

/// AAC bitrate for the stereo file (64 kbps per side).
const AAC_BITRATE: &str = "128000";

/// Echo canceller frame: 10 ms.
const FRAME: usize = (RECORD_RATE / 100) as usize;

/// Captures deliver every ~100 ms. A source silent for longer than this had a gap (a capture
/// restarted, or call audio only just started): on its return it is padded with silence up to
/// the other side so both stay in time.
const GAP: Duration = Duration::from_millis(400);

/// A side that never shows up (no call audio at all) is filled in once the other is this far
/// ahead, so the recording keeps being written.
const MAX_SKEW: usize = RECORD_RATE as usize * 2;

/// 16 kHz -> 48 kHz by linear interpolation, for sources that only have the transcription
/// stream (test playback). Live captures provide real 48 kHz audio.
fn upsample(samples: &[f32], last: &mut f32) -> Vec<f32> {
    let k = (RECORD_RATE / SAMPLE_RATE) as usize;
    let mut out = Vec::with_capacity(samples.len() * k);
    for &s in samples {
        for i in 1..=k {
            out.push(*last + (s - *last) * i as f32 / k as f32);
        }
        *last = s;
    }
    out
}

fn to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0) as i16
}

fn echo_canceller() -> Option<LinearPipeline> {
    let format = AudioFormat::ten_ms(RECORD_RATE, 1);
    // Echo removal only: the mic otherwise stays as it was captured (no gain control or
    // noise suppression).
    match linear::builder(format, format).enable_gain_controller2(false).enable_noise_suppression(false).build() {
        Ok(p) => Some(p),
        Err(e) => {
            log::warn!("echo canceller unavailable, keeping the raw mic: {e}");
            None
        }
    }
}

impl AudioRecorder {
    /// `base` is the output path without extension, e.g. .../audio/<meeting>/<offset_ms>.
    pub fn create(base: &Path) -> Result<Self> {
        if let Some(dir) = base.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // In progress under a name `recordings` skips until it is finished.
        let out = hound::WavWriter::create(base.with_extension("rec.wav"), spec())?;
        Ok(Self {
            base: base.to_path_buf(),
            out,
            queue: Default::default(),
            last: [0.0; 2],
            seen: [None; 2],
            aec: echo_canceller(),
            mic_frame: vec![0.0; FRAME],
            sys_frame: vec![0.0; FRAME],
            cleaned: vec![0.0; FRAME],
        })
    }

    /// `samples` is the 16 kHz transcription stream, `hires` the 48 kHz copy (preferred).
    pub fn write(&mut self, source: AudioSource, samples: &[f32], hires: &[f32]) {
        self.write_at(source, samples, hires, Instant::now())
    }

    fn write_at(&mut self, source: AudioSource, samples: &[f32], hires: &[f32], now: Instant) {
        let i = source as usize;
        let other = 1 - i;
        let returning = self.seen[i].is_none_or(|t| now.duration_since(t) > GAP);
        self.seen[i] = Some(now);
        if returning && self.queue[other].len() > self.queue[i].len() {
            let n = self.queue[other].len() - self.queue[i].len();
            self.queue[i].extend(std::iter::repeat_n(0.0, n));
        }
        if hires.is_empty() {
            let up = upsample(samples, &mut self.last[i]);
            self.queue[i].extend(up);
        } else {
            self.last[i] = *hires.last().unwrap_or(&0.0);
            self.queue[i].extend(hires);
        }
        let (a, b) = (self.queue[0].len(), self.queue[1].len());
        if a > b + MAX_SKEW {
            self.queue[1].extend(std::iter::repeat_n(0.0, a - b));
        } else if b > a + MAX_SKEW {
            self.queue[0].extend(std::iter::repeat_n(0.0, b - a));
        }
        self.drain();
    }

    /// Write every complete 10 ms frame both sides have.
    fn drain(&mut self) {
        let (mic, sys) = (AudioSource::Mic as usize, AudioSource::System as usize);
        while self.queue[mic].len() >= FRAME && self.queue[sys].len() >= FRAME {
            for (d, s) in self.mic_frame.iter_mut().zip(self.queue[mic].drain(..FRAME)) {
                *d = s;
            }
            for (d, s) in self.sys_frame.iter_mut().zip(self.queue[sys].drain(..FRAME)) {
                *d = s;
            }
            let mut left = &self.mic_frame;
            if let Some(aec) = self.aec.as_mut() {
                let done = aec.handle_render_frame(&self.sys_frame).is_ok()
                    && aec.process_capture_frame(&self.mic_frame, &mut self.cleaned).unwrap_or(false);
                if done {
                    left = &self.cleaned;
                }
            }
            for (l, r) in left.iter().zip(&self.sys_frame) {
                let _ = self.out.write_sample(to_i16(*l));
                let _ = self.out.write_sample(to_i16(*r));
            }
        }
    }

    /// Close and compress. Returns the .m4a path (or the .wav if AAC encoding isn't available).
    pub fn finish(mut self) -> Result<PathBuf> {
        // Pad the shorter side (to a whole frame) so the tail is written too.
        let longest = self.queue.iter().map(|q| q.len()).max().unwrap_or(0).div_ceil(FRAME) * FRAME;
        for q in self.queue.iter_mut() {
            let n = longest - q.len();
            q.extend(std::iter::repeat_n(0.0, n));
        }
        self.drain();
        self.out.finalize()?;
        let raw = self.base.with_extension("rec.wav");
        let m4a = self.base.with_extension("m4a");
        let ok = std::process::Command::new("/usr/bin/afconvert")
            .args(["-f", "m4af", "-d", "aac", "-b", AAC_BITRATE])
            .arg(&raw)
            .arg(&m4a)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && m4a.is_file() {
            let _ = std::fs::remove_file(&raw);
            Ok(m4a)
        } else {
            log::warn!("AAC encoding unavailable; keeping WAV recording");
            let wav = self.base.with_extension("wav");
            std::fs::rename(&raw, &wav)?;
            Ok(wav)
        }
    }
}

/// Meeting ids are UUIDs; anything else must never be joined onto a path.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Kept recordings for a meeting, sorted by the transcript offset they start at.
pub fn recordings(meeting_id: &str) -> Vec<(u64, PathBuf)> {
    if !valid_id(meeting_id) {
        return vec![];
    }
    recordings_in(&crate::paths::audio_dir().join(meeting_id))
}

fn recordings_in(dir: &Path) -> Vec<(u64, PathBuf)> {
    let mut v: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("m4a") | Some("wav")))
        .filter(|p| ![".rec.wav", ".mic.wav", ".sys.wav"].iter().any(|x| p.to_string_lossy().ends_with(x)))
        .filter_map(|p| p.file_stem()?.to_str()?.parse::<u64>().ok().map(|o| (o, p.clone())))
        .collect();
    v.sort_by_key(|x| x.0);
    v
}

/// Loudness outline of a kept recording for the playback timeline: one level (0-255, a
/// -60..0 dB scale) per `PEAK_STEP_MS` for each side (mic, call).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Peaks {
    pub step_ms: u32,
    pub mic: Vec<u8>,
    pub call: Vec<u8>,
}

pub const PEAK_STEP_MS: u32 = 100;
/// Decode rate for the outline: plenty for loudness, and fast to decode.
const PEAK_RATE: u32 = 8_000;

fn level(sum_sq: f64, n: usize) -> u8 {
    if n == 0 {
        return 0;
    }
    let db = 10.0 * (sum_sq / n as f64 + 1e-12).log10();
    (((db + 60.0) / 60.0).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Outline of the recording at `path`, cached next to it (`<offset>.peaks.json`).
pub fn peaks(path: &Path) -> Result<Peaks> {
    let cache = path.with_extension("peaks.json");
    if let Ok(p) = std::fs::read(&cache).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice::<Peaks>(&b)?)) {
        if p.step_ms == PEAK_STEP_MS {
            return Ok(p);
        }
    }
    let tmp = path.with_extension("peaks.tmp.wav");
    let ok = std::process::Command::new("/usr/bin/afconvert")
        .args(["-f", "WAVE", "-d", &format!("LEI16@{PEAK_RATE}")])
        .arg(path)
        .arg(&tmp)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    anyhow::ensure!(ok, "could not decode {}", path.display());
    let result = (|| {
        let mut r = hound::WavReader::open(&tmp)?;
        let ch = r.spec().channels.max(1) as usize;
        let step = (r.spec().sample_rate as u64 * PEAK_STEP_MS as u64 / 1000) as usize * ch;
        let mut p = Peaks { step_ms: PEAK_STEP_MS, mic: Vec::new(), call: Vec::new() };
        let (mut a, mut b, mut n) = (0f64, 0f64, 0usize);
        for (i, s) in r.samples::<i16>().enumerate() {
            let v = s? as f64 / 32768.0;
            if i % ch == 0 {
                a += v * v;
                n += 1;
            } else if i % ch == 1 {
                b += v * v;
            }
            if (i + 1) % step == 0 {
                p.mic.push(level(a, n));
                p.call.push(if ch > 1 { level(b, n) } else { 0 });
                (a, b, n) = (0.0, 0.0, 0);
            }
        }
        if n > 0 {
            p.mic.push(level(a, n));
            p.call.push(if ch > 1 { level(b, n) } else { 0 });
        }
        Ok::<_, anyhow::Error>(p)
    })();
    let _ = std::fs::remove_file(&tmp);
    let p = result?;
    let _ = std::fs::write(&cache, serde_json::to_vec(&p)?);
    Ok(p)
}

pub fn delete_recordings(meeting_id: &str) -> Result<()> {
    anyhow::ensure!(valid_id(meeting_id), "invalid meeting id");
    let dir = crate::paths::audio_dir().join(meeting_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).context("deleting recordings")?;
    }
    Ok(())
}

/// Delete recordings older than `days` (by file modification time). Returns files removed.
pub fn enforce_retention(days: u32) -> usize {
    if days == 0 {
        return 0;
    }
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(days as u64 * 86_400);
    let mut removed = 0;
    for meeting in std::fs::read_dir(crate::paths::audio_dir()).into_iter().flatten().filter_map(|e| e.ok()) {
        for f in std::fs::read_dir(meeting.path()).into_iter().flatten().filter_map(|e| e.ok()) {
            let old = f.metadata().and_then(|m| m.modified()).map(|t| t < cutoff).unwrap_or(false);
            if old && std::fs::remove_file(f.path()).is_ok() {
                removed += 1;
            }
        }
        let _ = std::fs::remove_dir(meeting.path()); // only succeeds when empty
    }
    removed
}

/// Bytes used by kept recordings.
pub fn usage_bytes() -> u64 {
    std::fs::read_dir(crate::paths::audio_dir())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .flat_map(|m| std::fs::read_dir(m.path()).into_iter().flatten().filter_map(|e| e.ok()))
        .filter_map(|f| f.metadata().ok())
        .map(|m| m.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn read_stereo(p: &Path) -> (Vec<f32>, Vec<f32>) {
        let raw = p.with_extension("check.wav");
        assert!(std::process::Command::new("/usr/bin/afconvert")
            .args(["-f", "WAVE", "-d", "LEI16"])
            .arg(p)
            .arg(&raw)
            .status()
            .unwrap()
            .success());
        let mut r = hound::WavReader::open(&raw).unwrap();
        let v: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
        (v.iter().step_by(2).copied().collect(), v.iter().skip(1).step_by(2).copied().collect())
    }

    fn noise(n: usize, seed: &mut u32) -> Vec<f32> {
        (0..n)
            .map(|_| {
                *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect()
    }

    #[test]
    fn records_mixes_and_encodes() {
        let dir = std::env::temp_dir().join(format!("milkrice-rec-{}", uuid::Uuid::new_v4()));
        let base = dir.join("0");
        let mut r = AudioRecorder::create(&base).unwrap();
        assert!(recordings_in(&dir).is_empty(), "an unfinished recording must not be listed");
        let tone: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let hi: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.02).sin() * 0.3).collect();
        r.write(AudioSource::Mic, &tone, &hi);
        r.write(AudioSource::System, &tone[..8000], &[]); // starts late -> padded; upsampled
        r.write(AudioSource::Mic, &tone, &hi);
        let out = r.finish().unwrap();
        assert!(out.is_file());
        assert!(!base.with_extension("rec.wav").exists());
        let p = peaks(&out).unwrap();
        // ~2 s of audio at 100 ms steps; the call side only joins after the first second.
        assert!((19..=22).contains(&p.mic.len()), "{} steps", p.mic.len());
        assert!(p.mic[5] > 150 && p.call[5] < 50 && p.call[12] > 150, "{p:?}");
        assert_eq!(peaks(&out).unwrap(), p, "cached outline differs");
        assert!(recordings_in(&dir).len() == 1, "the cache must not be listed as a recording");
        assert!(std::fs::metadata(&out).unwrap().len() > 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A gap on one side (a capture that restarted) must not shift the rest of the recording.
    #[test]
    fn gap_on_one_side_keeps_sides_in_step() {
        let dir = std::env::temp_dir().join(format!("milkrice-rec-{}", uuid::Uuid::new_v4()));
        let base = dir.join("0");
        let mut r = AudioRecorder::create(&base).unwrap();
        r.aec = None;
        let sec = RECORD_RATE as usize;
        let block = sec / 10;
        let quiet = vec![0.0f32; block];
        let loud = vec![0.5f32; block];
        // 3 s of call audio; the mic delivers its first second, then nothing for 1 s, then a
        // loud final second.
        let t0 = Instant::now();
        for t in 0..30 {
            let now = t0 + Duration::from_millis(100 * t);
            r.write_at(AudioSource::System, &[], &quiet, now);
            if t < 10 {
                r.write_at(AudioSource::Mic, &[], &quiet, now);
            } else if t >= 20 {
                r.write_at(AudioSource::Mic, &[], &loud, now);
            }
        }
        let out = r.finish().unwrap();
        let (mic, _) = read_stereo(&out);
        // The loud part lands in the last second, not right after the first.
        assert!(rms(&mic[sec + sec / 4..2 * sec - sec / 4]) < 0.05, "gap was not kept");
        assert!(rms(&mic[2 * sec + sec / 4..3 * sec - sec / 4]) > 0.3, "loud part misplaced");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Call audio leaking into the mic (delayed and quieter, as through speakers) is removed.
    #[test]
    fn removes_call_audio_leaking_into_the_mic() {
        let dir = std::env::temp_dir().join(format!("milkrice-rec-{}", uuid::Uuid::new_v4()));
        let base = dir.join("0");
        let mut r = AudioRecorder::create(&base).unwrap();
        let sec = RECORD_RATE as usize;
        let mut seed = 7;
        let far = noise(6 * sec, &mut seed);
        let delay = sec / 10; // 100 ms speaker-to-mic
        let mic: Vec<f32> = (0..far.len()).map(|i| if i >= delay { far[i - delay] * 0.3 } else { 0.0 }).collect();
        for (f, m) in far.chunks(sec / 10).zip(mic.chunks(sec / 10)) {
            r.write(AudioSource::System, &[], f);
            r.write(AudioSource::Mic, &[], m);
        }
        let out = r.finish().unwrap();
        let (left, _) = read_stereo(&out);
        let before = rms(&mic[4 * sec..6 * sec]);
        let after = rms(&left[4 * sec..6 * sec]);
        assert!(after < before * 0.25, "echo only went from {before} to {after}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
