//! Microphone capture via cpal. The callback only downmixes to mono and hands the
//! samples to a worker thread, which resamples to 16 kHz and emits 100 ms blocks.
use super::resample::Mono16k;
use super::CaptureHandle;
use crate::types::{AudioFrame, AudioSource};
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use crossbeam_channel::{bounded, unbounded, RecvTimeoutError, Sender, TrySendError};
use std::time::{Duration, Instant};
use std::thread::JoinHandle;

/// How long device enumeration / stream startup may block. CoreAudio can stall
/// indefinitely while a microphone permission prompt is pending.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
const ENUMERATE_TIMEOUT: Duration = Duration::from_secs(3);

pub fn list_input_devices() -> Vec<String> {
    // Enumeration probes each device through CoreAudio, which can block; run it on a
    // helper thread and return whatever was found when the timeout expires.
    let (name_tx, name_rx) = unbounded::<String>();
    let spawned = std::thread::Builder::new().name("milkrice-mic-enum".into()).spawn(move || {
        let host = cpal::default_host();
        if let Some(n) = host.default_input_device().and_then(|d| d.name().ok()) {
            let _ = name_tx.send(n);
        }
        match host.input_devices() {
            Ok(devs) => {
                for n in devs.filter_map(|d| d.name().ok()) {
                    let _ = name_tx.send(n);
                }
            }
            Err(e) => log::warn!("cannot enumerate input devices: {e}"),
        }
    });
    let mut names: Vec<String> = Vec::new();
    if spawned.is_err() {
        return names;
    }
    let deadline = Instant::now() + ENUMERATE_TIMEOUT;
    loop {
        match name_rx.recv_deadline(deadline) {
            Ok(n) => {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                log::warn!("input device enumeration timed out; returning partial list");
                break;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    names
}

fn find_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        None => host.default_input_device().ok_or_else(|| anyhow!("no default input device")),
        Some(wanted) => {
            let mut devs = host.input_devices().context("enumerating input devices")?;
            match devs.find(|d| d.name().map(|n| n == wanted).unwrap_or(false)) {
                Some(d) => Ok(d),
                // Unplugged headset etc.: fall back to the system default rather than failing.
                None => {
                    log::warn!("input device '{wanted}' not found; using the system default");
                    host.default_input_device().ok_or_else(|| anyhow!("no input device available"))
                }
            }
        }
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    cfg: &StreamConfig,
    raw_tx: Sender<Vec<f32>>,
    errors: Option<super::CaptureErrors>,
) -> Result<Stream>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = cfg.channels as usize;
    let mut dropped = 0u64;
    let stream = device.build_input_stream(
        cfg,
        move |data: &[T], _| {
            let mut mono = Vec::with_capacity(data.len() / channels.max(1) + 1);
            if channels <= 1 {
                mono.extend(data.iter().map(|s| s.to_sample::<f32>()));
            } else {
                // Convert + average per frame without an intermediate buffer.
                let inv = 1.0 / channels as f32;
                mono.extend(
                    data.chunks_exact(channels)
                        .map(|f| f.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() * inv),
                );
            }
            match raw_tx.try_send(mono) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    dropped += 1;
                    if dropped.is_power_of_two() {
                        log::warn!("mic worker behind; dropped {dropped} callback buffers");
                    }
                }
                Err(TrySendError::Disconnected(_)) => {}
            }
        },
        move |e| {
            log::error!("mic stream error: {e}");
            if let Some(errs) = errors.as_ref() {
                let _ = errs.try_send((AudioSource::Mic, e.to_string()));
            }
        },
        None,
    )?;
    Ok(stream)
}

/// Build the cpal stream; returns it with the sample rate.
fn open(device: Option<&str>, raw_tx: Sender<Vec<f32>>, errors: Option<super::CaptureErrors>) -> Result<(Stream, u32)> {
    let dev = find_device(device)?;
    let name = dev.name().unwrap_or_else(|_| "?".into());
    let supported = dev.default_input_config().context("querying default input config")?;
    let rate = supported.sample_rate().0;
    let fmt = supported.sample_format();
    let cfg: StreamConfig = supported.into();
    log::info!("mic '{name}': {} ch @ {rate} Hz, {fmt:?}", cfg.channels);
    let stream = match fmt {
        SampleFormat::F32 => build_stream::<f32>(&dev, &cfg, raw_tx, errors),
        SampleFormat::I16 => build_stream::<i16>(&dev, &cfg, raw_tx, errors),
        SampleFormat::I32 => build_stream::<i32>(&dev, &cfg, raw_tx, errors),
        SampleFormat::I8 => build_stream::<i8>(&dev, &cfg, raw_tx, errors),
        SampleFormat::U8 => build_stream::<u8>(&dev, &cfg, raw_tx, errors),
        SampleFormat::U16 => build_stream::<u16>(&dev, &cfg, raw_tx, errors),
        SampleFormat::U32 => build_stream::<u32>(&dev, &cfg, raw_tx, errors),
        SampleFormat::F64 => build_stream::<f64>(&dev, &cfg, raw_tx, errors),
        other => Err(anyhow!("unsupported mic sample format {other:?}")),
    }
    .context("building input stream")?;
    stream.play().context("starting input stream")?;
    Ok((stream, rate))
}

pub fn start_mic(
    device: Option<String>,
    tx: Sender<AudioFrame>,
    errors: Option<super::CaptureErrors>,
) -> Result<CaptureHandle> {
    let (raw_tx, raw_rx) = bounded::<Vec<f32>>(256);
    let (stop_tx, stop_rx) = bounded::<()>(1);
    let (ready_tx, ready_rx) = bounded::<Result<u32, String>>(1);

    // Stream thread: owns the !Send cpal stream until told to stop.
    let stream_thread: JoinHandle<()> = std::thread::Builder::new()
        .name("milkrice-mic-stream".into())
        .spawn(move || match open(device.as_deref(), raw_tx, errors) {
            Ok((stream, rate)) => {
                let _ = ready_tx.send(Ok(rate));
                // Blocks until stop (or the handle is dropped without sending).
                let _ = stop_rx.recv();
                drop(stream); // drops the callback -> closes raw channel -> worker exits
            }
            Err(e) => {
                let _ = ready_tx.send(Err(format!("{e:#}")));
            }
        })
        .context("spawning mic stream thread")?;

    let rate = match ready_rx.recv_timeout(STARTUP_TIMEOUT) {
        Ok(Ok(r)) => r,
        Err(RecvTimeoutError::Timeout) => {
            // The stream thread is stuck inside CoreAudio; dropping stop_tx lets it
            // exit on its own if it ever gets unstuck. Do not join it.
            return Err(anyhow!(
                "timed out opening the microphone (is a microphone permission prompt pending?)"
            ));
        }
        Ok(Err(e)) => {
            let _ = stream_thread.join();
            return Err(anyhow!("{e}"));
        }
        Err(_) => {
            let _ = stream_thread.join();
            return Err(anyhow!("mic stream thread died during startup"));
        }
    };

    let mut resampler = match Mono16k::new(rate) {
        Ok(r) => r,
        Err(e) => {
            let _ = stop_tx.send(());
            let _ = stream_thread.join();
            return Err(e);
        }
    };

    let worker = std::thread::Builder::new()
        .name("milkrice-mic-resample".into())
        .spawn(move || {
            'outer: for chunk in raw_rx.iter() {
                let mut alive = true;
                resampler.push(&chunk, |samples, hires| {
                    if alive && tx.send(AudioFrame { source: AudioSource::Mic, samples, hires }).is_err() {
                        alive = false;
                    }
                });
                if !alive {
                    log::info!("mic frame receiver closed; stopping resampler");
                    break 'outer;
                }
            }
        })
        .context("spawning mic resample thread")?;

    Ok(CaptureHandle::new(move || {
        let _ = stop_tx.send(());
        let _ = stream_thread.join();
        let _ = worker.join();
    }))
}
