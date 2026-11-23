//! Downmix + resample helpers shared by the mic and system capture paths.
//! Everything is turned into 16 kHz mono f32 blocks of ~100 ms for transcription. While a
//! recording is being kept, a second 48 kHz copy rides along for the audio file.
use crate::types::SAMPLE_RATE;
use anyhow::{anyhow, Result};
use rubato::{FftFixedIn, Resampler};

/// Samples per emitted block (100 ms at 16 kHz).
pub const BLOCK: usize = (SAMPLE_RATE as usize) / 10;

/// Average interleaved `channels`-wide frames into mono, appending to `out`.
pub fn downmix_interleaved(data: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(data);
        return;
    }
    let inv = 1.0 / channels as f32;
    out.extend(data.chunks_exact(channels).map(|f| f.iter().sum::<f32>() * inv));
}

/// Average planar channels (equal length) into mono, appending to `out`.
pub fn downmix_planar(planes: &[&[f32]], out: &mut Vec<f32>) {
    let Some(len) = planes.iter().map(|p| p.len()).min() else { return };
    if planes.len() == 1 {
        out.extend_from_slice(&planes[0][..len]);
        return;
    }
    let inv = 1.0 / planes.len() as f32;
    out.extend((0..len).map(|i| planes.iter().map(|p| p[i]).sum::<f32>() * inv));
}

/// Sample rate of kept recordings.
pub const RECORD_RATE: u32 = 48_000;

/// Streaming mono resampler from one fixed rate to another.
struct Stream {
    resampler: Option<FftFixedIn<f32>>,
    in_buf: Vec<f32>,
    out_chunk: Vec<Vec<f32>>,
}

impl Stream {
    fn new(input_rate: u32, output_rate: u32) -> Result<Self> {
        let resampler = if input_rate == output_rate {
            None
        } else {
            // ~20 ms input chunks.
            let chunk = (input_rate as usize / 50).max(64);
            Some(
                FftFixedIn::<f32>::new(input_rate as usize, output_rate as usize, chunk, 2, 1)
                    .map_err(|e| anyhow!("resampler init ({input_rate} -> {output_rate} Hz): {e}"))?,
            )
        };
        let out_chunk = match &resampler {
            Some(r) => r.output_buffer_allocate(true),
            None => Vec::new(),
        };
        Ok(Self { resampler, in_buf: Vec::new(), out_chunk })
    }

    fn push(&mut self, mono: &[f32], out: &mut Vec<f32>) {
        let Some(r) = self.resampler.as_mut() else {
            out.extend_from_slice(mono);
            return;
        };
        self.in_buf.extend_from_slice(mono);
        loop {
            let need = r.input_frames_next();
            if self.in_buf.len() < need {
                break;
            }
            let input = [&self.in_buf[..need]];
            match r.process_into_buffer(&input, &mut self.out_chunk, None) {
                Ok((_, produced)) => out.extend_from_slice(&self.out_chunk[0][..produced]),
                Err(e) => log::error!("resample error: {e}"),
            }
            self.in_buf.drain(..need);
        }
    }
}

/// Streaming mono resampler to 16 kHz that emits fixed-size blocks, plus (while a recording
/// is kept, see `super::set_keep_recording_audio`) a 48 kHz copy for the audio file.
pub struct Mono16k {
    rate: u32,
    asr: Stream,
    pending: Vec<f32>,
    hires: Option<(Stream, Vec<f32>)>,
}

impl Mono16k {
    pub fn new(input_rate: u32) -> Result<Self> {
        Self::with_hires(input_rate, super::keep_recording_audio())
    }

    pub fn with_hires(input_rate: u32, hires: bool) -> Result<Self> {
        if input_rate == 0 {
            return Err(anyhow!("invalid input sample rate 0"));
        }
        let hires = if hires { Some((Stream::new(input_rate, RECORD_RATE)?, Vec::new())) } else { None };
        Ok(Self {
            rate: input_rate,
            asr: Stream::new(input_rate, SAMPLE_RATE)?,
            pending: Vec::with_capacity(BLOCK * 2),
            hires,
        })
    }

    pub fn input_rate(&self) -> u32 {
        self.rate
    }

    /// Feed mono samples at the input rate; `emit(block, hires)` is called for each full 100 ms
    /// 16 kHz block, with the 48 kHz samples produced since the previous call (empty when no
    /// recording is kept).
    pub fn push(&mut self, mono: &[f32], mut emit: impl FnMut(Vec<f32>, Vec<f32>)) {
        self.asr.push(mono, &mut self.pending);
        if let Some((r, buf)) = self.hires.as_mut() {
            r.push(mono, buf);
        }
        let mut start = 0;
        while self.pending.len() - start >= BLOCK {
            let hires = self.hires.as_mut().map(|(_, b)| std::mem::take(b)).unwrap_or_default();
            emit(self.pending[start..start + BLOCK].to_vec(), hires);
            start += BLOCK;
        }
        if start > 0 {
            self.pending.drain(..start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn stereo_48k_sine_to_16k_mono() {
        let secs = 3usize;
        let n = 48_000 * secs;
        let mut inter = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin() * 0.5;
            inter.push(s);
            inter.push(s);
        }
        let mut mono = Vec::new();
        downmix_interleaved(&inter, 2, &mut mono);
        assert_eq!(mono.len(), n);

        let mut rs = Mono16k::new(48_000).unwrap();
        let mut out: Vec<f32> = Vec::new();
        // feed in odd-sized pieces to exercise buffering
        for piece in mono.chunks(777) {
            rs.push(piece, |b, _| {
                assert_eq!(b.len(), BLOCK);
                out.extend(b)
            });
        }
        let expected = 16_000 * secs;
        // within the resampler delay + one partial block
        assert!(out.len() <= expected && out.len() + 2 * BLOCK >= expected, "len {}", out.len());
        let tail = &out[out.len() / 2..];
        let r = rms(tail);
        let want = 0.5 / 2f32.sqrt();
        assert!((r - want).abs() < 0.02, "rms {r} vs {want}");
    }

    #[test]
    fn hires_copy_tracks_asr_stream() {
        let n = 24_000 * 3;
        let mono: Vec<f32> = (0..n).map(|i| (i as f32 * 0.03).sin() * 0.4).collect();
        let mut rs = Mono16k::with_hires(24_000, true).unwrap();
        let (mut asr, mut hi) = (0usize, 0usize);
        for piece in mono.chunks(500) {
            rs.push(piece, |b, h| {
                asr += b.len();
                hi += h.len();
            });
        }
        // 3x as many 48 kHz samples as 16 kHz ones, give or take resampler delay.
        let diff = (hi as i64 - 3 * asr as i64).abs();
        assert!(asr > 0 && diff <= 3 * BLOCK as i64, "asr {asr} hires {hi}");
        let mut plain = Mono16k::with_hires(24_000, false).unwrap();
        plain.push(&mono, |_, h| assert!(h.is_empty()));
    }

    #[test]
    fn passthrough_16k_and_planar() {
        let a = vec![1.0f32; 3200];
        let b = vec![0.0f32; 3200];
        let mut mono = Vec::new();
        downmix_planar(&[&a, &b], &mut mono);
        assert!(mono.iter().all(|v| (*v - 0.5).abs() < 1e-6));
        let mut rs = Mono16k::new(16_000).unwrap();
        let mut blocks = 0;
        rs.push(&mono, |_, _| blocks += 1);
        assert_eq!(blocks, 2);
    }
}
