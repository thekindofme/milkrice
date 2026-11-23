//! Opt-in local recording of a meeting: mic on the left channel, call audio on the right,
//! 48 kHz, saved as AAC (.m4a, ~58 MB/hour) next to the transcript so moments can be replayed.
//! Transcription keeps using its own 16 kHz stream; this one is for listening.
use crate::audio::RECORD_RATE;
use crate::types::{AudioSource, SAMPLE_RATE};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct AudioRecorder {
    base: PathBuf,
    mic: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
    sys: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
    mic_n: u64,
    sys_n: u64,
    last: [f32; 2],
}

fn spec() -> hound::WavSpec {
    hound::WavSpec { channels: 1, sample_rate: RECORD_RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }
}

/// AAC bitrate for the stereo file (64 kbps per side).
const AAC_BITRATE: &str = "128000";

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

impl AudioRecorder {
    /// `base` is the output path without extension, e.g. .../audio/<meeting>/<offset_ms>.
    pub fn create(base: &Path) -> Result<Self> {
        if let Some(dir) = base.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mic = hound::WavWriter::create(base.with_extension("mic.wav"), spec())?;
        Ok(Self { base: base.to_path_buf(), mic: Some(mic), sys: None, mic_n: 0, sys_n: 0, last: [0.0; 2] })
    }

    /// `samples` is the 16 kHz transcription stream, `hires` the 48 kHz copy (preferred).
    pub fn write(&mut self, source: AudioSource, samples: &[f32], hires: &[f32]) {
        let up;
        let samples = if hires.is_empty() && !samples.is_empty() {
            up = upsample(samples, &mut self.last[source as usize]);
            &up[..]
        } else {
            if let Some(v) = hires.last() {
                self.last[source as usize] = *v;
            }
            hires
        };
        match source {
            AudioSource::Mic => {
                if let Some(w) = self.mic.as_mut() {
                    for s in samples {
                        let _ = w.write_sample(to_i16(*s));
                    }
                    self.mic_n += samples.len() as u64;
                }
            }
            AudioSource::System => {
                if self.sys.is_none() {
                    // Call audio usually starts a moment after the mic: pad so both line up.
                    match hound::WavWriter::create(self.base.with_extension("sys.wav"), spec()) {
                        Ok(mut w) => {
                            for _ in 0..self.mic_n {
                                let _ = w.write_sample(0i16);
                            }
                            self.sys_n = self.mic_n;
                            self.sys = Some(w);
                        }
                        Err(e) => {
                            log::warn!("audio recording (call side) disabled: {e}");
                            return;
                        }
                    }
                }
                if let Some(w) = self.sys.as_mut() {
                    for s in samples {
                        let _ = w.write_sample(to_i16(*s));
                    }
                    self.sys_n += samples.len() as u64;
                }
            }
        }
    }

    /// Close, mix to stereo and compress. Returns the .m4a path (or the .wav if AAC encoding
    /// isn't available).
    pub fn finish(mut self) -> Result<PathBuf> {
        let mic_path = self.base.with_extension("mic.wav");
        let sys_path = self.base.with_extension("sys.wav");
        if let Some(w) = self.mic.take() {
            w.finalize()?;
        }
        let has_sys = if let Some(w) = self.sys.take() {
            w.finalize()?;
            true
        } else {
            false
        };
        let stereo = self.base.with_extension("wav");
        {
            let mut mic = hound::WavReader::open(&mic_path)?;
            let mut sys = if has_sys { Some(hound::WavReader::open(&sys_path)?) } else { None };
            let mut out = hound::WavWriter::create(
                &stereo,
                hound::WavSpec { channels: 2, ..spec() },
            )?;
            let mut ms = mic.samples::<i16>();
            let mut ss = sys.as_mut().map(|r| r.samples::<i16>());
            loop {
                let l = ms.next().and_then(|r| r.ok());
                let r = ss.as_mut().and_then(|it| it.next()).and_then(|r| r.ok());
                if l.is_none() && r.is_none() {
                    break;
                }
                out.write_sample(l.unwrap_or(0))?;
                out.write_sample(r.unwrap_or(0))?;
            }
            out.finalize()?;
        }
        let _ = std::fs::remove_file(&mic_path);
        let _ = std::fs::remove_file(&sys_path);
        let m4a = self.base.with_extension("m4a");
        let ok = std::process::Command::new("/usr/bin/afconvert")
            .args(["-f", "m4af", "-d", "aac", "-b", AAC_BITRATE])
            .arg(&stereo)
            .arg(&m4a)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && m4a.is_file() {
            let _ = std::fs::remove_file(&stereo);
            Ok(m4a)
        } else {
            log::warn!("AAC encoding unavailable; keeping WAV recording");
            Ok(stereo)
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
    let dir = crate::paths::audio_dir().join(meeting_id);
    let mut v: Vec<(u64, PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("m4a") | Some("wav")))
        .filter(|p| !p.to_string_lossy().ends_with(".mic.wav") && !p.to_string_lossy().ends_with(".sys.wav"))
        .filter_map(|p| p.file_stem()?.to_str()?.parse::<u64>().ok().map(|o| (o, p.clone())))
        .collect();
    v.sort_by_key(|x| x.0);
    v
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

    #[test]
    fn records_mixes_and_encodes() {
        let dir = std::env::temp_dir().join(format!("milkrice-rec-{}", uuid::Uuid::new_v4()));
        let base = dir.join("0");
        let mut r = AudioRecorder::create(&base).unwrap();
        let tone: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let hi: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.02).sin() * 0.3).collect();
        r.write(AudioSource::Mic, &tone, &hi);
        r.write(AudioSource::System, &tone[..8000], &[]); // starts late -> padded; upsampled
        r.write(AudioSource::Mic, &tone, &hi);
        let out = r.finish().unwrap();
        assert!(out.is_file());
        assert!(!base.with_extension("mic.wav").exists());
        assert!(std::fs::metadata(&out).unwrap().len() > 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
