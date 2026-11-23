//! A live recording session: capture → VAD → transcription → speaker labels → sink.
//!
//! Threads: capture callbacks push 16 kHz frames; a segmenter thread runs one VAD per source;
//! a worker thread transcribes utterances in order, labels speakers and removes echo
//! (your speakers leaking into your mic when you're not on headphones).
//!
//! The segmenter also watches the live captures: a source that stops delivering frames (mic
//! unplugged, AirPods gone, ScreenCaptureKit stream stopped) is reported to the sink as
//! `CAPTURE_LOST:<mic|system>: …` and restarted; `CAPTURE_RESTORED:<mic|system>` follows
//! once audio flows again.
use crate::audio::{self, CaptureErrors, CaptureHandle};
use crate::diarize::{SpeakerClusterer, SpeakerEmbedder};
use crate::stt::Transcriber;
use crate::types::{AudioFrame, AudioSource, Segment, SAMPLE_RATE};
use crate::vad::{Utterance, Vad, VadConfig};
use anyhow::Result;
use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub trait SessionSink: Send + Sync {
    fn segment(&self, seg: Segment);
    /// RMS levels (0..1) roughly every 100 ms.
    fn levels(&self, mic: f32, system: f32);
    fn warning(&self, msg: &str);
}

#[derive(Clone)]
pub struct SessionConfig {
    pub capture_mic: bool,
    pub capture_system: bool,
    pub mic_device: Option<String>,
    /// Added to every timestamp (for resuming a meeting).
    pub offset_ms: u64,
    pub clusterer: Option<SpeakerClusterer>,
    pub vad: VadConfig,
    /// Opt-in: keep a local recording at this path (without extension).
    pub record_to: Option<std::path::PathBuf>,
    /// New remote speakers are numbered after this (s{base+1}, …) when resuming a meeting.
    pub speaker_base: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            capture_mic: true,
            capture_system: true,
            mic_device: None,
            offset_ms: 0,
            clusterer: None,
            vad: VadConfig::default(),
            record_to: None,
            speaker_base: 0,
        }
    }
}

pub struct SessionResult {
    pub duration_ms: u64,
    pub clusterer: SpeakerClusterer,
    /// Speaker renames from the end-of-meeting tidy-up (`(from, to)`), to apply to the
    /// stored segments.
    pub relabel: Vec<(String, String)>,
}

pub struct Session {
    captures: Arc<Mutex<Captures>>,
    capture_count: usize,
    /// Set by `stop()`: the watchdog must not restart anything any more.
    stopping: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    elapsed: Arc<AtomicU64>,
    segmenter: Option<JoinHandle<()>>,
    #[allow(clippy::type_complexity)]
    worker: Option<JoinHandle<(SpeakerClusterer, Vec<(String, String)>)>>,
    started: Instant,
    pub system_active: bool,
}

struct SourcedUtterance {
    source: AudioSource,
    utt: Utterance,
}

/// Running capture handles. Shared with the segmenter so it can swap in a restarted source.
#[derive(Default)]
struct Captures {
    mic: Option<CaptureHandle>,
    system: Option<CaptureHandle>,
    other: Vec<CaptureHandle>,
}

impl Captures {
    fn count(&self) -> usize {
        self.mic.is_some() as usize + self.system.is_some() as usize + self.other.len()
    }
    fn take_all(&mut self) -> Vec<CaptureHandle> {
        let mut v: Vec<CaptureHandle> = self.other.drain(..).collect();
        v.extend(self.mic.take());
        v.extend(self.system.take());
        v
    }
}

/// A source with no frames for this long (while not paused) counts as lost.
const CAPTURE_STALL: Duration = Duration::from_secs(3);
/// While a source stays lost, try restarting it this often.
const CAPTURE_RETRY: Duration = Duration::from_secs(10);

/// Lets the segmenter restart a live capture that stopped delivering audio.
struct Restarter {
    frame_tx: Sender<AudioFrame>,
    err_tx: CaptureErrors,
    err_rx: Receiver<(AudioSource, String)>,
    mic_device: Option<String>,
    captures: Arc<Mutex<Captures>>,
    stopping: Arc<AtomicBool>,
}

impl Restarter {
    fn restart(&self, source: AudioSource) -> Result<()> {
        let new = match source {
            AudioSource::Mic => {
                match audio::start_mic_watched(self.mic_device.clone(), self.frame_tx.clone(), self.err_tx.clone()) {
                    Ok(h) => h,
                    Err(e) if self.mic_device.is_some() => {
                        log::warn!("restarting the chosen mic failed ({e:#}); trying the system default");
                        audio::start_mic_watched(None, self.frame_tx.clone(), self.err_tx.clone())?
                    }
                    Err(e) => return Err(e),
                }
            }
            AudioSource::System => audio::start_system_watched(self.frame_tx.clone(), self.err_tx.clone())?,
        };
        let old = {
            let mut c = self.captures.lock();
            if self.stopping.load(Ordering::SeqCst) {
                drop(c);
                new.stop();
                return Ok(());
            }
            match source {
                AudioSource::Mic => c.mic.replace(new),
                AudioSource::System => c.system.replace(new),
            }
        };
        // Tearing down a dead stream can block inside CoreAudio / ScreenCaptureKit: keep it
        // off the audio path.
        if let Some(old) = old {
            let _ = std::thread::Builder::new().name("milkrice-capture-stop".into()).spawn(move || old.stop());
        }
        Ok(())
    }
}

#[derive(Default)]
struct SourceWatch {
    last_frame: Option<Instant>,
    lost: bool,
    last_try: Option<Instant>,
}

fn source_name(s: AudioSource) -> &'static str {
    match s {
        AudioSource::Mic => "mic",
        AudioSource::System => "system",
    }
}

/// System-audio progress, shared with the worker so held mic lines wait for the system side.
#[derive(Default)]
struct SysProgress {
    /// Any system audio arrived.
    seen: AtomicBool,
    /// The system VAD is inside an utterance.
    active: AtomicBool,
    /// Meeting time (ms, offset included) up to which system audio went through the VAD.
    pos_ms: AtomicU64,
    /// System utterances sent to the worker and not yet transcribed.
    inflight: AtomicU64,
}

impl SysProgress {
    /// True once no system speech that could echo a mic line ending at `mic_end_ms` is still
    /// on its way.
    fn caught_up(&self, mic_end_ms: u64) -> bool {
        !self.seen.load(Ordering::SeqCst)
            || (self.inflight.load(Ordering::SeqCst) == 0
                && !self.active.load(Ordering::SeqCst)
                && self.pos_ms.load(Ordering::SeqCst) >= mic_end_ms + ECHO_WINDOW_MS)
    }
}

impl Session {
    pub fn start(
        cfg: SessionConfig,
        engine: Arc<Mutex<Transcriber>>,
        embedder: Option<Arc<Mutex<SpeakerEmbedder>>>,
        sink: Arc<dyn SessionSink>,
    ) -> Result<Self> {
        // Kept recordings get their own 48 kHz stream; transcription-only sessions skip it.
        audio::set_keep_recording_audio(cfg.record_to.is_some());
        let (frame_tx, frame_rx) = bounded::<AudioFrame>(512);
        let (err_tx, err_rx) = bounded::<(AudioSource, String)>(16);
        let mut captures = Captures::default();
        if cfg.capture_mic {
            captures.mic = Some(audio::start_mic_watched(cfg.mic_device.clone(), frame_tx.clone(), err_tx.clone())?);
        }
        let mut system_active = false;
        if cfg.capture_system {
            match audio::start_system_watched(frame_tx.clone(), err_tx.clone()) {
                Ok(h) => {
                    captures.system = Some(h);
                    system_active = true;
                }
                Err(e) => sink.warning(&format!(
                    "Couldn't capture other participants' audio ({e}). Only your microphone is being transcribed."
                )),
            }
        }
        anyhow::ensure!(captures.count() > 0, "no audio source could be started");
        let captures = Arc::new(Mutex::new(captures));
        let stopping = Arc::new(AtomicBool::new(false));
        // The restarter keeps a frame sender: the segmenter drops it once `stop()` begins, so
        // the frame channel still closes when the captures stop.
        let restarter = Restarter {
            frame_tx,
            err_tx,
            err_rx,
            mic_device: cfg.mic_device.clone(),
            captures: captures.clone(),
            stopping: stopping.clone(),
        };
        Self::run_pipeline(cfg, frame_rx, captures, stopping, Some(restarter), system_active, engine, embedder, sink)
    }

    /// Run the pipeline on frames from any source (files, tests). Capture ends when all
    /// senders of `frame_rx` are dropped.
    pub fn from_frames(
        cfg: SessionConfig,
        frame_rx: Receiver<AudioFrame>,
        engine: Arc<Mutex<Transcriber>>,
        embedder: Option<Arc<Mutex<SpeakerEmbedder>>>,
        sink: Arc<dyn SessionSink>,
    ) -> Result<Self> {
        Self::run_pipeline(cfg, frame_rx, Default::default(), Default::default(), None, true, engine, embedder, sink)
    }

    /// Like `from_frames`, with handles that are stopped (so their senders close) on `stop()`.
    pub fn from_frames_with(
        cfg: SessionConfig,
        frame_rx: Receiver<AudioFrame>,
        handles: Vec<CaptureHandle>,
        engine: Arc<Mutex<Transcriber>>,
        embedder: Option<Arc<Mutex<SpeakerEmbedder>>>,
        sink: Arc<dyn SessionSink>,
    ) -> Result<Self> {
        let captures = Arc::new(Mutex::new(Captures { other: handles, ..Default::default() }));
        Self::run_pipeline(cfg, frame_rx, captures, Default::default(), None, true, engine, embedder, sink)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_pipeline(
        cfg: SessionConfig,
        frame_rx: Receiver<AudioFrame>,
        captures: Arc<Mutex<Captures>>,
        stopping: Arc<AtomicBool>,
        restarter: Option<Restarter>,
        system_active: bool,
        engine: Arc<Mutex<Transcriber>>,
        embedder: Option<Arc<Mutex<SpeakerEmbedder>>>,
        sink: Arc<dyn SessionSink>,
    ) -> Result<Self> {
        let paused = Arc::new(AtomicBool::new(false));
        let elapsed = Arc::new(AtomicU64::new(0));
        let (utt_tx, utt_rx) = unbounded::<SourcedUtterance>();
        let capture_count = captures.lock().count();
        let sys_progress = Arc::new(SysProgress::default());
        let seg_progress = sys_progress.clone();
        let seg_stopping = stopping.clone();

        let seg_sink = sink.clone();
        let seg_paused = paused.clone();
        let seg_elapsed = elapsed.clone();
        let vad_cfg = cfg.vad.clone();
        let offset = cfg.offset_ms;
        let mut recorder = cfg.record_to.as_deref().and_then(|p| match crate::audio_rec::AudioRecorder::create(p) {
            Ok(r) => Some(r),
            Err(e) => {
                log::warn!("not keeping audio: {e}");
                None
            }
        });
        let segmenter = std::thread::Builder::new().name("milkrice-vad".into()).spawn(move || {
            let mut restarter = restarter;
            let mut watch: [SourceWatch; 2] = Default::default();
            let mut last_watch = Instant::now();
            let mut sys_samples = 0u64;
            let mut mic_vad = Vad::new(vad_cfg.clone());
            // Remote audio is cleaner (no room noise): a slightly lower threshold catches quiet talkers.
            let mut sys_vad = Vad::new(VadConfig { threshold_db: 7.0, min_rms: 0.002, ..vad_cfg });
            let mut mic_samples = 0u64;
            // macOS delivers exact digital silence when the app lacks microphone permission.
            let mut mic_zero_run = 0u64;
            let mut warned_silent = false;
            // Some mics / voice processing emit exact zeros during quiet moments, so only a mic
            // that has never produced any sound counts as blocked.
            let mut mic_ever_live = false;
            // A mic that never gets above a whisper is probably the wrong device.
            let mut mic_max_peak = 0f32;
            let mut warned_quiet = false;
            let mut last_level = Instant::now();
            let (mut mic_peak, mut sys_peak) = (0f32, 0f32);
            let send = |source, mut u: Utterance| {
                u.start_ms += offset;
                let _ = utt_tx.send(SourcedUtterance { source, utt: u });
            };
            loop {
                let frame = match frame_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(f) => Some(f),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                if let Some(f) = frame.as_ref() {
                    let w = &mut watch[f.source as usize];
                    w.last_frame = Some(Instant::now());
                    if w.lost {
                        w.lost = false;
                        w.last_try = None;
                        log::info!("{} capture restored", source_name(f.source));
                        seg_sink.warning(&format!("CAPTURE_RESTORED:{}", source_name(f.source)));
                    }
                }
                if seg_stopping.load(Ordering::SeqCst) {
                    // Drop our frame sender so the channel closes once the captures stop.
                    restarter = None;
                } else if last_watch.elapsed() >= Duration::from_secs(1) {
                    last_watch = Instant::now();
                    if let Some(r) = restarter.as_ref() {
                        check_captures(r, &mut watch, seg_paused.load(Ordering::Relaxed), &*seg_sink);
                    }
                }
                let Some(frame) = frame else { continue };
                if seg_paused.load(Ordering::Relaxed) {
                    continue;
                }
                if let Some(r) = recorder.as_mut() {
                    r.write(frame.source, &frame.samples, &frame.hires);
                }
                let lvl = frame.samples.iter().fold(0f32, |m, v| m.max(v.abs()));
                match frame.source {
                    AudioSource::Mic => {
                        mic_samples += frame.samples.len() as u64;
                        if frame.samples.iter().all(|v| *v == 0.0) {
                            mic_zero_run += frame.samples.len() as u64;
                            if !warned_silent && !mic_ever_live && mic_zero_run > 4 * SAMPLE_RATE as u64 {
                                warned_silent = true;
                                seg_sink.warning("MIC_SILENT: Your microphone is sending no sound at all.");
                            }
                        } else {
                            mic_zero_run = 0;
                            mic_ever_live = true;
                        }
                        mic_max_peak = mic_max_peak.max(lvl);
                        if !warned_quiet && !warned_silent && mic_ever_live && mic_max_peak < 0.02
                            && mic_samples > 20 * SAMPLE_RATE as u64
                        {
                            warned_quiet = true;
                            seg_sink.warning("MIC_QUIET: Your microphone has been nearly silent since recording started.");
                        }
                        seg_elapsed.store(mic_samples * 1000 / SAMPLE_RATE as u64, Ordering::Relaxed);
                        mic_peak = mic_peak.max(lvl);
                        for u in mic_vad.push(&frame.samples) {
                            send(AudioSource::Mic, u);
                        }
                    }
                    AudioSource::System => {
                        sys_peak = sys_peak.max(lvl);
                        sys_samples += frame.samples.len() as u64;
                        for u in sys_vad.push(&frame.samples) {
                            seg_progress.inflight.fetch_add(1, Ordering::SeqCst);
                            send(AudioSource::System, u);
                        }
                        seg_progress.active.store(sys_vad.in_speech(), Ordering::SeqCst);
                        seg_progress.pos_ms.store(offset + sys_samples * 1000 / SAMPLE_RATE as u64, Ordering::SeqCst);
                        seg_progress.seen.store(true, Ordering::SeqCst);
                    }
                }
                if last_level.elapsed() >= Duration::from_millis(100) {
                    seg_sink.levels(mic_peak.min(1.0), sys_peak.min(1.0));
                    mic_peak = 0.0;
                    sys_peak = 0.0;
                    last_level = Instant::now();
                }
            }
            if let Some(u) = mic_vad.flush() {
                send(AudioSource::Mic, u);
            }
            if let Some(u) = sys_vad.flush() {
                seg_progress.inflight.fetch_add(1, Ordering::SeqCst);
                send(AudioSource::System, u);
            }
            seg_progress.active.store(false, Ordering::SeqCst);
            if let Some(r) = recorder.take() {
                match r.finish() {
                    Ok(p) => log::info!("kept meeting audio: {}", p.display()),
                    Err(e) => log::warn!("saving meeting audio failed: {e}"),
                }
            }
        })?;

        let clusterer = cfg
            .clusterer
            .clone()
            .unwrap_or_else(|| SpeakerClusterer::new(crate::diarize::DEFAULT_THRESHOLD));
        let worker_sink = sink;
        let speaker_base = cfg.speaker_base;
        let worker = std::thread::Builder::new()
            .name("milkrice-stt".into())
            .spawn(move || worker_loop(utt_rx, engine, embedder, clusterer, speaker_base, sys_progress, worker_sink))?;

        Ok(Self {
            captures,
            capture_count,
            stopping,
            paused,
            elapsed,
            segmenter: Some(segmenter),
            worker: Some(worker),
            started: Instant::now(),
            system_active,
        })
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
    /// Recorded (unpaused) audio length in ms.
    pub fn elapsed_ms(&self) -> u64 {
        let e = self.elapsed.load(Ordering::Relaxed);
        if e == 0 && self.capture_count == 1 && !self.is_paused() {
            // System-only capture: fall back to wall clock.
            return self.started.elapsed().as_millis() as u64;
        }
        e
    }

    /// Stop capture and wait for the transcription backlog to drain.
    pub fn stop(mut self) -> SessionResult {
        let duration_ms = self.elapsed_ms();
        self.stopping.store(true, Ordering::SeqCst);
        let handles = self.captures.lock().take_all();
        for c in handles {
            c.stop();
        }
        if let Some(h) = self.segmenter.take() {
            let _ = h.join();
        }
        let (clusterer, relabel) = self
            .worker
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_else(|| (SpeakerClusterer::new(crate::diarize::DEFAULT_THRESHOLD), vec![]));
        SessionResult { duration_ms, clusterer, relabel }
    }
}

fn words(t: &str) -> Vec<String> {
    t.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(|w| w.to_lowercase())
        .collect()
}

/// Mic and system speech this close in time can be the same audio.
const ECHO_WINDOW_MS: u64 = 1500;
/// Shorter mic lines are never treated as echo: short replies ("yeah, that's right") share
/// their words with almost anything the other side says.
const ECHO_MIN_WORDS: usize = 5;
/// Share of the mic words that must be covered by word trigrams also said on the system side.
const ECHO_MIN_COVERAGE: f32 = 0.6;
/// A mic line waits at most this long for the system side to catch up before it is shown.
const ECHO_MAX_HOLD: Duration = Duration::from_secs(6);

/// Share of `mic` words that sit inside a 3-word run also found (in order) in `sys`.
fn trigram_coverage(mic: &[String], sys: &[String]) -> f32 {
    if mic.len() < 3 || sys.len() < 3 {
        return 0.0;
    }
    let grams: std::collections::HashSet<&[String]> = sys.windows(3).collect();
    let mut covered = vec![false; mic.len()];
    for (i, g) in mic.windows(3).enumerate() {
        if grams.contains(g) {
            covered[i..i + 3].iter_mut().for_each(|c| *c = true);
        }
    }
    covered.iter().filter(|c| **c).count() as f32 / mic.len() as f32
}

/// Whether a mic line is the system audio leaking back through the speakers: long enough,
/// close in time, and repeating the system speech word-for-word in order.
fn is_echo<'a>(mic: &Segment, recent_sys: impl IntoIterator<Item = &'a Segment>) -> bool {
    let m = words(&mic.text);
    if m.len() < ECHO_MIN_WORDS {
        return false;
    }
    let mut near: Vec<&Segment> = recent_sys
        .into_iter()
        .filter(|s| mic.start_ms < s.end_ms + ECHO_WINDOW_MS && s.start_ms < mic.end_ms + ECHO_WINDOW_MS)
        .collect();
    if near.is_empty() {
        return false;
    }
    // An echo can straddle two system lines: compare against them joined in time order.
    near.sort_by_key(|s| s.start_ms);
    let sys: Vec<String> = near.iter().flat_map(|s| words(&s.text)).collect();
    trigram_coverage(&m, &sys) >= ECHO_MIN_COVERAGE
}

/// Watchdog tick (about once a second): note capture errors and stalled sources, warn once
/// per loss and try to restart.
fn check_captures(r: &Restarter, watch: &mut [SourceWatch; 2], paused: bool, sink: &dyn SessionSink) {
    let lose = |src: AudioSource, why: String, watch: &mut [SourceWatch; 2]| {
        let w = &mut watch[src as usize];
        if w.lost {
            return;
        }
        w.lost = true;
        w.last_try = None;
        let what = match src {
            AudioSource::Mic => "Your microphone stopped sending audio",
            AudioSource::System => "Audio from the other people on the call stopped",
        };
        log::warn!("{} capture lost: {why}", source_name(src));
        sink.warning(&format!("CAPTURE_LOST:{}: {what} ({why}). Reconnecting…", source_name(src)));
    };
    for (src, msg) in r.err_rx.try_iter() {
        lose(src, msg, watch);
    }
    if !paused {
        for src in [AudioSource::Mic, AudioSource::System] {
            let w = &watch[src as usize];
            let stalled = !w.lost && w.last_frame.map(|t| t.elapsed() > CAPTURE_STALL).unwrap_or(false);
            // ScreenCaptureKit may send nothing while nothing plays: a system gap only counts
            // while the output device is in use (an SCK stop is also reported by its delegate).
            if stalled && (src == AudioSource::Mic || crate::detect::output_in_use()) {
                lose(src, format!("no audio for {} s", CAPTURE_STALL.as_secs()), watch);
            }
        }
    }
    for src in [AudioSource::Mic, AudioSource::System] {
        let w = &mut watch[src as usize];
        if !w.lost || w.last_try.map(|t| t.elapsed() < CAPTURE_RETRY).unwrap_or(false) {
            continue;
        }
        w.last_try = Some(Instant::now());
        match r.restart(src) {
            // Give the new stream a full stall period to deliver its first frames.
            Ok(()) => {
                log::info!("{} capture restarted", source_name(src));
                w.last_frame = Some(Instant::now());
            }
            Err(e) => log::warn!("restarting {} capture failed: {e:#}", source_name(src)),
        }
    }
}

/// Pause-finding frame: 20 ms at 16 kHz.
const TURN_FRAME: usize = 320;
/// A dip this long (in frames) between words can be a change of speaker.
const TURN_MIN_PAUSE_FRAMES: usize = 8; // 160 ms
/// Voice-print window on each side of a candidate cut.
const TURN_WINDOW: usize = 2 * SAMPLE_RATE as usize;
/// Each side needs at least this much audio for a usable voice print.
const TURN_MIN_SIDE: usize = SAMPLE_RATE as usize;
/// Below this cosine similarity, the two sides of a pause are different people.
const TURN_CHANGE_SIM: f32 = 0.35;

/// Midpoints (sample index) of pauses inside `samples` that are long enough to separate turns.
/// A pause is a run of frames well below the utterance's speech level.
fn pause_points(samples: &[f32]) -> Vec<usize> {
    let rms: Vec<f32> = samples
        .chunks(TURN_FRAME)
        .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    if rms.len() < TURN_MIN_PAUSE_FRAMES * 3 {
        return vec![];
    }
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let loud = sorted[sorted.len() * 9 / 10];
    let quiet = (loud * 0.1).max(1e-4);
    let mut out = Vec::new();
    let mut run = 0usize;
    for (i, &r) in rms.iter().enumerate() {
        if r < quiet {
            run += 1;
            continue;
        }
        if run >= TURN_MIN_PAUSE_FRAMES && i > run {
            out.push((i - run / 2) * TURN_FRAME);
        }
        run = 0;
    }
    out
}

/// Split a call-audio utterance where the speaker changes. Returns sample ranges covering
/// the whole utterance. Candidate cuts are pauses; a cut is kept when the voice prints just
/// before and just after it don't match.
fn split_turns(samples: &[f32], emb: &mut SpeakerEmbedder) -> Vec<(usize, usize)> {
    let whole = vec![(0, samples.len())];
    if samples.len() < 2 * TURN_MIN_SIDE + SAMPLE_RATE as usize {
        return whole;
    }
    let mut cuts = Vec::new();
    let mut prev = 0usize;
    for c in pause_points(samples) {
        let left = (c.saturating_sub(TURN_WINDOW)).max(prev);
        let right = (c + TURN_WINDOW).min(samples.len());
        if c - left < TURN_MIN_SIDE || right - c < TURN_MIN_SIDE {
            continue;
        }
        let (Ok(l), Ok(r)) = (emb.embed(&samples[left..c]), emb.embed(&samples[c..right])) else { continue };
        let sim = crate::diarize::cosine(&l, &r);
        log::debug!("turn candidate at {} ms: similarity {sim:.2}", c * 1000 / SAMPLE_RATE as usize);
        if sim < TURN_CHANGE_SIM {
            cuts.push(c);
            prev = c;
        }
    }
    if cuts.is_empty() {
        return whole;
    }
    let mut out = Vec::with_capacity(cuts.len() + 1);
    let mut a = 0;
    for c in cuts {
        out.push((a, c));
        a = c;
    }
    out.push((a, samples.len()));
    out
}

/// Utterances shorter than this can't create a new speaker (see worker_loop).
const MIN_NEW_SPEAKER_SECS: f32 = 2.5;
/// ...nor can ones with fewer words than this (a long clip of "mm", noise or a cough).
const MIN_NEW_SPEAKER_WORDS: usize = 4;
/// A short clip joins its nearest speaker only when at least this similar; otherwise it is
/// given to whoever spoke last.
const SHORT_CLIP_MIN_SIM: f32 = 0.3;

fn worker_loop(
    rx: Receiver<SourcedUtterance>,
    engine: Arc<Mutex<Transcriber>>,
    embedder: Option<Arc<Mutex<SpeakerEmbedder>>>,
    mut clusterer: SpeakerClusterer,
    speaker_base: usize,
    sys_progress: Arc<SysProgress>,
    sink: Arc<dyn SessionSink>,
) -> (SpeakerClusterer, Vec<(String, String)>) {
    // Every remote segment's label and voice print, for the end-of-meeting tidy-up.
    let mut labelled: Vec<crate::diarize::Labelled> = Vec::new();
    // Mic segments wait until the system side has caught up past them (or ECHO_MAX_HOLD), so
    // overlapping system speech can mark them as echo.
    let mut pending_mic: Vec<(Segment, Instant)> = Vec::new();
    let mut recent_sys: std::collections::VecDeque<Segment> = Default::default();
    let mut last_sys_speaker = "s1".to_string();

    let flush = |pending: &mut Vec<(Segment, Instant)>, recent: &std::collections::VecDeque<Segment>, all: bool| {
        let mut keep = Vec::new();
        for (seg, t) in pending.drain(..) {
            if is_echo(&seg, recent) {
                // Not shown as a line of yours. Logged without the text: meeting content stays out of logs.
                log::info!(
                    "mic line at {} ms treated as speaker echo, not shown ({} chars, {} words)",
                    seg.start_ms,
                    seg.text.len(),
                    words(&seg.text).len()
                );
                continue;
            }
            if all || t.elapsed() >= ECHO_MAX_HOLD || sys_progress.caught_up(seg.end_ms) {
                sink.segment(seg);
            } else {
                keep.push((seg, t));
            }
        }
        *pending = keep;
    };

    loop {
        let item = match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(i) => Some(i),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if let Some(SourcedUtterance { source, utt }) = item {
            // Call audio often holds several people with only a short pause between turns, so a
            // long utterance is cut wherever the voice changes; each piece gets its own speaker.
            let pieces = match (&embedder, source) {
                (Some(emb), AudioSource::System) => split_turns(&utt.samples, &mut emb.lock()),
                _ => vec![(0, utt.samples.len())],
            };
            for (a, b) in pieces {
                let samples = &utt.samples[a..b];
                let start_ms = utt.start_ms + a as u64 * 1000 / SAMPLE_RATE as u64;
                let end_ms = utt.start_ms + b as u64 * 1000 / SAMPLE_RATE as u64;
                let text = match engine.lock().transcribe(samples) {
                    Ok(t) => t,
                    Err(e) => {
                        sink.warning(&format!("Transcription error: {e}"));
                        String::new()
                    }
                };
                if text.is_empty() {
                    continue;
                }
                let dur = samples.len() as f32 / SAMPLE_RATE as f32;
                let mut voice: Option<Vec<f32>> = None;
                let speaker = match source {
                    AudioSource::Mic => "me".to_string(),
                    AudioSource::System => match &embedder {
                        // Short or near-empty clips give noisy voice prints: they may only join a
                        // speaker already heard, never create one. Before anyone is known, they
                        // take the first speaker's label without seeding it, so a noisy first
                        // "hello?" can't become a phantom speaker.
                        Some(emb) if dur >= 0.8 => match emb.lock().embed(samples) {
                            Ok(v) => {
                                let can_seed = dur >= MIN_NEW_SPEAKER_SECS && words(&text).len() >= MIN_NEW_SPEAKER_WORDS;
                                let label = if can_seed {
                                    format!("s{}", clusterer.assign(&v) + 1 + speaker_base)
                                } else if clusterer.centroids.is_empty() {
                                    format!("s{}", 1 + speaker_base)
                                } else {
                                    match clusterer.nearest_above(&v, SHORT_CLIP_MIN_SIM) {
                                        Some(i) => format!("s{}", i + 1 + speaker_base),
                                        None => last_sys_speaker.clone(),
                                    }
                                };
                                voice = Some(v);
                                label
                            }
                            Err(_) => last_sys_speaker.clone(),
                        },
                        Some(_) => last_sys_speaker.clone(),
                        None => "them".into(),
                    },
                };
                if source == AudioSource::System && embedder.is_some() {
                    labelled.push(crate::diarize::Labelled { label: speaker.clone(), emb: voice, secs: dur });
                }
                let seg = Segment { id: 0, start_ms, end_ms, source, speaker: speaker.clone(), text };
                match source {
                    AudioSource::Mic => pending_mic.push((seg, Instant::now())),
                    AudioSource::System => {
                        last_sys_speaker = speaker;
                        recent_sys.push_back(seg.clone());
                        while recent_sys.len() > 20 {
                            recent_sys.pop_front();
                        }
                        sink.segment(seg);
                    }
                }
            }
            if source == AudioSource::System {
                sys_progress.inflight.fetch_sub(1, Ordering::SeqCst);
            }
        }
        flush(&mut pending_mic, &recent_sys, false);
    }
    flush(&mut pending_mic, &recent_sys, true);
    let relabel = crate::diarize::consolidate(&labelled, clusterer.threshold);
    (clusterer, relabel)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(secs: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE as f32) as usize).map(|i| 0.3 * (i as f32 * 0.07).sin()).collect()
    }

    #[test]
    fn finds_short_pauses_between_turns() {
        let gap = vec![0.0f32; SAMPLE_RATE as usize * 3 / 10]; // 300 ms
        let tiny = vec![0.0f32; SAMPLE_RATE as usize / 20]; // 50 ms: a gap inside a word
        let mut a = tone(2.0);
        a.extend(&gap);
        a.extend(tone(1.5));
        a.extend(&tiny);
        a.extend(tone(1.5));
        let p = pause_points(&a);
        assert_eq!(p.len(), 1, "{p:?}");
        let at = p[0] as f32 / SAMPLE_RATE as f32;
        assert!((2.0..2.3).contains(&at), "cut at {at}s");
        assert!(pause_points(&tone(5.0)).is_empty());
    }

    fn seg(src: AudioSource, a: u64, b: u64, t: &str) -> Segment {
        Segment { id: 0, start_ms: a, end_ms: b, source: src, speaker: String::new(), text: t.into() }
    }

    #[test]
    fn echo_detection() {
        const LONG_REMOTE: &str = "Yeah so I think that's right in principle, but honestly we should ship the beta \
            on Friday after QA signs off and then see what the numbers look like next week";
        // (name, system lines, mic line, expected echo)
        #[allow(clippy::type_complexity)]
        let cases: &[(&str, &[(u64, u64, &str)], (u64, u64, &str), bool)] = &[
            (
                "true echo of a long remote line",
                &[(1000, 5000, "We should ship the beta on Friday after QA signs off")],
                (1200, 5100, "we should ship the beta on friday after qa signs"),
                true,
            ),
            (
                "echo with one misheard word",
                &[(1000, 9000, LONG_REMOTE)],
                (1300, 9100, "we should ship the beta on friday after qa signs of and then see what the numbers"),
                true,
            ),
            (
                "echo straddling two system lines",
                &[(1000, 3000, "We should ship the beta"), (3100, 5000, "on Friday after QA signs off")],
                (1200, 5100, "we should ship the beta on friday after qa"),
                true,
            ),
            (
                "short reply next to a long remote line with the same words",
                &[(1000, 9000, LONG_REMOTE)],
                (4000, 5500, "yeah that's right"),
                false,
            ),
            (
                "short reply, review example",
                &[(1000, 9000, LONG_REMOTE)],
                (4000, 5500, "Yeah I think that's right"),
                false,
            ),
            (
                "longer reply reusing remote words out of order",
                &[(1000, 9000, LONG_REMOTE)],
                (4000, 6500, "Right, the numbers next week should look like the beta, yeah"),
                false,
            ),
            (
                "unrelated speech during remote speech",
                &[(1000, 5000, "We should ship the beta on Friday after QA signs off")],
                (2000, 4000, "Sorry, can everyone see my screen right now"),
                false,
            ),
            (
                "same words but far apart in time",
                &[(1000, 5000, "We should ship the beta on Friday after QA signs off")],
                (20_000, 24_000, "we should ship the beta on friday after qa signs"),
                false,
            ),
        ];
        for (name, sys, mic, want) in cases {
            let sys: Vec<Segment> = sys.iter().map(|(a, b, t)| seg(AudioSource::System, *a, *b, t)).collect();
            let mic = seg(AudioSource::Mic, mic.0, mic.1, mic.2);
            assert_eq!(is_echo(&mic, &sys), *want, "{name}");
        }
    }

    #[test]
    fn mic_lines_wait_for_system_side() {
        let p = SysProgress::default();
        // No system audio at all: nothing to wait for.
        assert!(p.caught_up(5000));
        p.seen.store(true, Ordering::SeqCst);
        p.pos_ms.store(5500, Ordering::SeqCst);
        assert!(!p.caught_up(5000), "system audio not yet past the echo window");
        p.pos_ms.store(7000, Ordering::SeqCst);
        assert!(p.caught_up(5000));
        p.active.store(true, Ordering::SeqCst);
        assert!(!p.caught_up(5000), "remote still talking");
        p.active.store(false, Ordering::SeqCst);
        p.inflight.store(1, Ordering::SeqCst);
        assert!(!p.caught_up(5000), "a system line is still being transcribed");
    }
}
