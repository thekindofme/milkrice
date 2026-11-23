//! System-output audio capture via ScreenCaptureKit (macOS). Requires Screen Recording permission.
use super::resample::{downmix_interleaved, downmix_planar, Mono16k};
use super::CaptureHandle;
use crate::types::{AudioFrame, AudioSource};
use anyhow::{anyhow, Context, Result};
use crossbeam_channel::Sender;
use parking_lot::Mutex;
use screencapturekit::prelude::*;

extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {}

pub fn permitted() -> bool {
    // SAFETY: plain CoreGraphics query with no arguments.
    unsafe { CGPreflightScreenCaptureAccess() }
}

pub fn request_permission() -> bool {
    // SAFETY: plain CoreGraphics call with no arguments.
    unsafe { CGRequestScreenCaptureAccess() }
}

const REQUESTED_RATE: u32 = 48_000;
const REQUESTED_CHANNELS: u32 = 2;

struct Handler {
    tx: Sender<AudioFrame>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    resampler: Option<Mono16k>,
    mono: Vec<f32>,
    closed: bool,
}

impl Handler {
    fn process(&self, sample: &CMSampleBuffer) {
        let rate = sample
            .format_description()
            .and_then(|f| f.audio_sample_rate())
            .map(|r| r.round() as u32)
            .filter(|r| *r > 0)
            .unwrap_or(REQUESTED_RATE);
        let list = match sample.audio_buffer_list() {
            Ok(l) => l,
            Err(e) => {
                log::debug!("audio_buffer_list failed: {e}");
                return;
            }
        };
        let mut st = self.state.lock();
        if st.closed {
            return;
        }
        let State { resampler, mono, closed } = &mut *st;
        mono.clear();

        // Bytes -> f32 (ScreenCaptureKit delivers 32-bit float PCM).
        let to_f32 = |b: &[u8]| -> Vec<f32> {
            b.chunks_exact(4).map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]])).collect()
        };
        let n = list.num_buffers();
        if n == 0 {
            return;
        }
        if n == 1 {
            let Some(buf) = list.get(0) else { return };
            let data = to_f32(buf.data());
            downmix_interleaved(&data, buf.number_channels.max(1) as usize, mono);
        } else {
            let planes: Vec<Vec<f32>> =
                list.iter().map(|b| to_f32(b.data())).collect();
            let refs: Vec<&[f32]> = planes.iter().map(|p| p.as_slice()).collect();
            downmix_planar(&refs, mono);
        }
        if mono.is_empty() {
            return;
        }

        if resampler.as_ref().map(|r| r.input_rate()) != Some(rate) {
            match Mono16k::new(rate) {
                Ok(r) => *resampler = Some(r),
                Err(e) => {
                    log::error!("system audio resampler: {e}");
                    return;
                }
            }
        }
        if let Some(r) = resampler.as_mut() {
            r.push(mono, |samples, hires| {
                if !*closed
                    && self.tx.send(AudioFrame { source: AudioSource::System, samples, hires }).is_err()
                {
                    *closed = true;
                }
            });
        }
    }
}

impl SCStreamOutputTrait for Handler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if of_type == SCStreamOutputType::Audio {
            self.process(&sample);
        }
    }
}

pub fn start(tx: Sender<AudioFrame>, errors: Option<super::CaptureErrors>) -> Result<CaptureHandle> {
    if !permitted() {
        return Err(anyhow!(
            "Screen Recording permission not granted (needed to capture system audio)"
        ));
    }
    let content = SCShareableContent::get()
        .map_err(|e| anyhow!("SCShareableContent: {e}"))?;
    let display = content
        .displays()
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no displays available for system audio capture"))?;
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[])
        .build()
        .map_err(|e| anyhow!("content filter: {e}"))?;
    // Video cannot be disabled outright: shrink it to a 2x2 surface at 1 fps.
    let config = SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_minimum_frame_interval(&CMTime::new(1, 1))
        .with_captures_audio(true)
        .with_sample_rate(REQUESTED_RATE as i32)
        .with_channel_count(REQUESTED_CHANNELS as i32)
        .with_excludes_current_process_audio(true);

    // The delegate is the only way ScreenCaptureKit reports a stream it stopped on its own.
    let mut stream = match errors {
        Some(errs) => SCStream::new_with_delegate(
            &filter,
            &config,
            ErrorHandler::new(move |e| {
                log::error!("system audio stream stopped: {e}");
                let _ = errs.try_send((AudioSource::System, e.to_string()));
            }),
        ),
        None => SCStream::new(&filter, &config),
    }
    .map_err(|e| anyhow!("SCStream: {e}"))?;
    stream
        .add_output_handler(
            Handler { tx, state: Mutex::new(State::default()) },
            SCStreamOutputType::Audio,
        )
        .map_err(|e| anyhow!("add audio handler: {e}"))?;
    stream.start_capture().map_err(|e| anyhow!("start capture: {e}")).context("system audio")?;
    log::info!("system audio capture started");

    Ok(CaptureHandle::new(move || {
        if let Err(e) = stream.stop_capture() {
            log::warn!("stop system capture: {e}");
        }
        drop(stream);
    }))
}
