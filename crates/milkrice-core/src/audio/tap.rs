//! Call-side audio through a Core Audio process tap (macOS 14.2+).
//!
//! ScreenCaptureKit only hears regular apps. iPhone calls relayed to the Mac (Continuity) and
//! FaceTime are played by the `avconferenced` system service, so ScreenCaptureKit records
//! silence for them. A global process tap hears every process that plays audio, at the output
//! device's native rate, with no video stream behind it. Milkrice's own process is excluded.
//!
//! The tap needs the "System Audio Recording" permission (TCC service kTCCServiceAudioCapture).
//! Without it macOS delivers silence rather than an error, so `start` refuses to run when the
//! permission is known to be denied and the caller falls back to ScreenCaptureKit.
use super::resample::{downmix_interleaved, downmix_planar, Mono16k};
use super::CaptureHandle;
use crate::types::{AudioFrame, AudioSource};
use anyhow::{anyhow, bail, Result};
use crossbeam_channel::{bounded, RecvTimeoutError, Sender};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioSubDeviceUIDKey,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, CATapDescription, CATapMuteBehavior,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};
use std::ffi::{c_void, CStr};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

type ObjectId = u32;
type OSStatus = i32;
type IOProcId = *mut c_void;

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct StreamDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    channels: u32,
    byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    count: u32,
    buffers: [AudioBuffer; 1],
}

type IOProc = unsafe extern "C" fn(
    ObjectId,
    *const c_void,
    *const AudioBufferList,
    *const c_void,
    *mut AudioBufferList,
    *const c_void,
    *mut c_void,
) -> OSStatus;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(
        object: ObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
    fn AudioHardwareCreateProcessTap(description: *const AnyObject, tap: *mut ObjectId) -> OSStatus;
    fn AudioHardwareDestroyProcessTap(tap: ObjectId) -> OSStatus;
    fn AudioHardwareCreateAggregateDevice(description: *const c_void, device: *mut ObjectId) -> OSStatus;
    fn AudioHardwareDestroyAggregateDevice(device: ObjectId) -> OSStatus;
    fn AudioDeviceCreateIOProcID(device: ObjectId, proc_: IOProc, client: *mut c_void, id: *mut IOProcId) -> OSStatus;
    fn AudioDeviceDestroyIOProcID(device: ObjectId, id: IOProcId) -> OSStatus;
    fn AudioDeviceStart(device: ObjectId, id: IOProcId) -> OSStatus;
    fn AudioDeviceStop(device: ObjectId, id: IOProcId) -> OSStatus;
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}
const SYSTEM_OBJECT: ObjectId = 1;
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const ELEMENT_MAIN: u32 = 0;
const DEFAULT_OUTPUT_DEVICE: u32 = fourcc(b"dOut");
const DEVICE_UID: u32 = fourcc(b"uid ");
const NOMINAL_SAMPLE_RATE: u32 = fourcc(b"nsrt");
const TRANSLATE_PID_TO_PROCESS: u32 = fourcc(b"id2p");
const TAP_FORMAT: u32 = fourcc(b"tfmt");
const FORMAT_LINEAR_PCM: u32 = fourcc(b"lpcm");
const FLAG_IS_FLOAT: u32 = 1;

/// With the aggregate device running, buffers arrive every few ms even in silence: a longer
/// gap means the device went away.
const IO_STALL: Duration = Duration::from_secs(2);

fn get<T: Copy>(object: ObjectId, selector: u32, qualifier: Option<&u32>, mut value: T) -> Result<T> {
    let addr = PropertyAddress { selector, scope: SCOPE_GLOBAL, element: ELEMENT_MAIN };
    let mut size = std::mem::size_of::<T>() as u32;
    let (q_size, q_ptr) = match qualifier {
        Some(q) => (4, q as *const u32 as *const c_void),
        None => (0, null()),
    };
    // SAFETY: `value` is a plain-old-data out parameter of exactly `size` bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(object, &addr, q_size, q_ptr, &mut size, &mut value as *mut T as *mut c_void)
    };
    if status != 0 {
        bail!("CoreAudio property {selector:#x} on object {object}: status {status}");
    }
    Ok(value)
}

fn default_output_device() -> Result<ObjectId> {
    match get(SYSTEM_OBJECT, DEFAULT_OUTPUT_DEVICE, None, 0u32)? {
        0 => bail!("no default output device"),
        d => Ok(d),
    }
}

fn device_uid(device: ObjectId) -> Result<Retained<NSString>> {
    let raw = get(device, DEVICE_UID, None, null_mut::<c_void>())?;
    // SAFETY: the property returns a +1 CFStringRef, toll-free bridged to NSString.
    unsafe { Retained::from_raw(raw as *mut NSString) }.ok_or_else(|| anyhow!("output device has no UID"))
}

/// Milkrice's own Core Audio process object, if it has one yet (it appears once we use audio).
fn own_process_object() -> Option<ObjectId> {
    let pid = std::process::id();
    get(SYSTEM_OBJECT, TRANSLATE_PID_TO_PROCESS, Some(&pid), 0u32).ok().filter(|o| *o != 0)
}

fn ns(s: &CStr) -> Retained<NSString> {
    NSString::from_str(s.to_str().unwrap_or_default())
}

fn dict(entries: &[(&CStr, &AnyObject)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = entries.iter().map(|(k, _)| ns(k)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = entries.iter().map(|(_, v)| *v).collect();
    NSDictionary::from_slices(&key_refs, &values)
}

/// Owns everything Core Audio created, torn down in reverse order.
struct Tap {
    tap: ObjectId,
    aggregate: ObjectId,
    proc_id: IOProcId,
    started: bool,
    /// Boxed so the IO callback's pointer stays put; freed only after the IOProc is destroyed.
    client: *mut Sender<Vec<f32>>,
}

// SAFETY: the raw ids are plain integers / opaque handles that Core Audio accepts from any thread.
unsafe impl Send for Tap {}

impl Drop for Tap {
    fn drop(&mut self) {
        // SAFETY: each id was created by the matching Create call and is destroyed once.
        unsafe {
            if self.started {
                AudioDeviceStop(self.aggregate, self.proc_id);
            }
            if !self.proc_id.is_null() {
                AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
            }
            if self.aggregate != 0 {
                AudioHardwareDestroyAggregateDevice(self.aggregate);
            }
            if self.tap != 0 {
                AudioHardwareDestroyProcessTap(self.tap);
            }
            if !self.client.is_null() {
                drop(Box::from_raw(self.client));
            }
        }
    }
}

/// Real-time IO callback: downmix to mono and hand off. Resampling happens on a worker thread.
unsafe extern "C" fn io_proc(
    _device: ObjectId,
    _now: *const c_void,
    input: *const AudioBufferList,
    _input_time: *const c_void,
    _output: *mut AudioBufferList,
    _output_time: *const c_void,
    client: *mut c_void,
) -> OSStatus {
    if input.is_null() || client.is_null() {
        return 0;
    }
    let tx = &*(client as *const Sender<Vec<f32>>);
    let list = &*input;
    let buffers = std::slice::from_raw_parts(list.buffers.as_ptr(), list.count as usize);
    let plane = |b: &AudioBuffer| -> &[f32] {
        if b.data.is_null() {
            &[]
        } else {
            std::slice::from_raw_parts(b.data as *const f32, b.byte_size as usize / 4)
        }
    };
    let mut mono = Vec::new();
    match buffers {
        [] => return 0,
        [one] => downmix_interleaved(plane(one), one.channels.max(1) as usize, &mut mono),
        many => {
            let planes: Vec<&[f32]> = many.iter().map(plane).collect();
            downmix_planar(&planes, &mut mono);
        }
    }
    if !mono.is_empty() {
        // Never block the IO thread: if the worker is behind, drop this buffer.
        let _ = tx.try_send(mono);
    }
    0
}

fn create(raw_tx: Sender<Vec<f32>>) -> Result<(Tap, f64, ObjectId)> {
    let mut t = Tap { tap: 0, aggregate: 0, proc_id: null_mut(), started: false, client: null_mut() };

    let exclude: Vec<Retained<NSNumber>> =
        own_process_object().into_iter().map(NSNumber::new_u32).collect();
    let exclude = NSArray::from_retained_slice(&exclude);
    // SAFETY: standard Objective-C alloc/init and setters on a fresh object.
    let desc = unsafe {
        let d = CATapDescription::initStereoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &exclude);
        d.setName(&NSString::from_str("Milkrice call audio"));
        d.setPrivate(true);
        d.setMuteBehavior(CATapMuteBehavior::Unmuted);
        d
    };
    // SAFETY: `desc` is a valid CATapDescription; `t.tap` is a valid out pointer.
    let status = unsafe { AudioHardwareCreateProcessTap(Retained::as_ptr(&desc) as *const AnyObject, &mut t.tap) };
    if status != 0 || t.tap == 0 {
        bail!("AudioHardwareCreateProcessTap failed (status {status})");
    }
    let format = get(t.tap, TAP_FORMAT, None, StreamDescription::default())?;
    if format.format_id != FORMAT_LINEAR_PCM || format.format_flags & FLAG_IS_FLOAT == 0 || format.bits_per_channel != 32 {
        bail!("unexpected tap format (id {:#x}, flags {:#x}, {} bits)", format.format_id, format.format_flags, format.bits_per_channel);
    }

    let output = default_output_device()?;
    let output_uid = device_uid(output)?;
    // SAFETY: plain getters on a valid CATapDescription / NSUUID.
    let tap_uid = unsafe { desc.UUID().UUIDString() };
    let agg_uid = NSUUID::new().UUIDString();
    let name = NSString::from_str("Milkrice call audio");
    let yes = NSNumber::new_bool(true);
    let no = NSNumber::new_bool(false);
    let sub = dict(&[(kAudioSubDeviceUIDKey, &output_uid)]);
    let tap_entry = dict(&[(kAudioSubTapUIDKey, &tap_uid), (kAudioSubTapDriftCompensationKey, &yes)]);
    let subs = NSArray::from_retained_slice(&[sub]);
    let taps = NSArray::from_retained_slice(&[tap_entry]);
    let description = dict(&[
        (kAudioAggregateDeviceNameKey, &name),
        (kAudioAggregateDeviceUIDKey, &agg_uid),
        (kAudioAggregateDeviceMainSubDeviceKey, &output_uid),
        (kAudioAggregateDeviceIsPrivateKey, &yes),
        (kAudioAggregateDeviceIsStackedKey, &no),
        (kAudioAggregateDeviceTapAutoStartKey, &yes),
        (kAudioAggregateDeviceSubDeviceListKey, &subs),
        (kAudioAggregateDeviceTapListKey, &taps),
    ]);
    // SAFETY: NSDictionary is toll-free bridged to CFDictionaryRef; out pointer is valid.
    let status = unsafe { AudioHardwareCreateAggregateDevice(Retained::as_ptr(&description) as *const c_void, &mut t.aggregate) };
    if status != 0 || t.aggregate == 0 {
        bail!("AudioHardwareCreateAggregateDevice failed (status {status})");
    }

    // With drift compensation the tap is delivered at the aggregate's clock rate.
    let rate = get(t.aggregate, NOMINAL_SAMPLE_RATE, None, 0f64)
        .ok()
        .filter(|r| *r > 0.0)
        .unwrap_or(format.sample_rate);
    if rate <= 0.0 {
        bail!("tap reports no sample rate");
    }

    t.client = Box::into_raw(Box::new(raw_tx));
    // SAFETY: `io_proc` matches the AudioDeviceIOProc ABI; `t.client` outlives the IOProc.
    let status = unsafe { AudioDeviceCreateIOProcID(t.aggregate, io_proc, t.client as *mut c_void, &mut t.proc_id) };
    if status != 0 || t.proc_id.is_null() {
        bail!("AudioDeviceCreateIOProcID failed (status {status})");
    }
    // SAFETY: valid device and IOProc id.
    let status = unsafe { AudioDeviceStart(t.aggregate, t.proc_id) };
    if status != 0 {
        bail!("AudioDeviceStart failed (status {status})");
    }
    t.started = true;
    log::info!(
        "call audio tap: {} ch @ {rate} Hz (tap format {} Hz), output '{}'",
        format.channels_per_frame,
        format.sample_rate,
        output_uid
    );
    Ok((t, rate, output))
}

pub fn start(tx: Sender<AudioFrame>, errors: Option<super::CaptureErrors>) -> Result<CaptureHandle> {
    if permission() == TapPermission::Denied {
        bail!("System Audio Recording permission denied");
    }
    let (raw_tx, raw_rx) = bounded::<Vec<f32>>(256);
    let (tap, rate, output) = create(raw_tx)?;
    let mut resampler = Mono16k::new(rate.round() as u32)?;

    let (stop_tx, stop_rx) = bounded::<()>(1);
    let worker = std::thread::Builder::new().name("milkrice-tap".into()).spawn(move || {
        let mut reported = false;
        let mut last_check = Instant::now();
        let mut last_buffer = Instant::now();
        let mut report = |why: String| {
            if !reported {
                reported = true;
                log::warn!("call audio tap: {why}");
                if let Some(e) = errors.as_ref() {
                    let _ = e.try_send((AudioSource::System, why));
                }
            }
        };
        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }
            match raw_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(mono) => {
                    last_buffer = Instant::now();
                    let mut closed = false;
                    resampler.push(&mono, |samples, hires| {
                        if !closed && tx.send(AudioFrame { source: AudioSource::System, samples, hires }).is_err() {
                            closed = true;
                        }
                    });
                    if closed {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if last_buffer.elapsed() > IO_STALL {
                report(format!("no audio buffers for {} s", IO_STALL.as_secs()));
            }
            // The aggregate is clocked by the output device it was built on: rebuild it when
            // the default output changes (e.g. AirPods connected).
            if last_check.elapsed() > Duration::from_secs(1) {
                last_check = Instant::now();
                if default_output_device().ok().is_some_and(|d| d != output) {
                    report("output device changed".into());
                }
            }
        }
    })?;
    log::info!("system audio capture started (process tap)");

    Ok(CaptureHandle::new(move || {
        let _ = stop_tx.try_send(());
        // Stopping first guarantees the IO callback no longer runs when the sender is freed.
        drop(tap);
        let _ = worker.join();
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapPermission {
    Granted,
    Denied,
    /// Never asked, or the check isn't available on this macOS.
    Unknown,
}

const TCC_SERVICE: &str = "kTCCServiceAudioCapture";

fn tcc_symbol(name: &CStr) -> *mut c_void {
    extern "C" {
        fn dlopen(path: *const std::ffi::c_char, mode: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
    }
    const RTLD_NOW: i32 = 2;
    // SAFETY: dlopen/dlsym with NUL-terminated strings; the framework stays loaded.
    unsafe {
        let lib = dlopen(c"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC".as_ptr(), RTLD_NOW);
        if lib.is_null() {
            return null_mut();
        }
        dlsym(lib, name.as_ptr())
    }
}

/// The "System Audio Recording" permission. macOS has no public API for it, so this asks TCC
/// directly (the same check Apple's own tools use); `Unknown` when that isn't possible.
pub fn permission() -> TapPermission {
    let f = tcc_symbol(c"TCCAccessPreflight");
    if f.is_null() {
        return TapPermission::Unknown;
    }
    // SAFETY: TCCAccessPreflight(CFStringRef service, CFDictionaryRef options) -> int.
    let preflight: unsafe extern "C" fn(*const c_void, *const c_void) -> i32 = unsafe { std::mem::transmute(f) };
    let service = NSString::from_str(TCC_SERVICE);
    match unsafe { preflight(Retained::as_ptr(&service) as *const c_void, null()) } {
        0 => TapPermission::Granted,
        1 => TapPermission::Denied,
        _ => TapPermission::Unknown,
    }
}

/// Show the System Audio Recording prompt if it hasn't been answered. Blocks until the user
/// answers (or 2 minutes pass); call from a worker thread.
pub fn request_permission() -> TapPermission {
    if permission() != TapPermission::Unknown {
        return permission();
    }
    let f = tcc_symbol(c"TCCAccessRequest");
    if f.is_null() {
        return TapPermission::Unknown;
    }
    // SAFETY: TCCAccessRequest(CFStringRef service, CFDictionaryRef options, void (^)(Bool)).
    let request: unsafe extern "C" fn(*const c_void, *const c_void, &block2::Block<dyn Fn(objc2::runtime::Bool)>) =
        unsafe { std::mem::transmute(f) };
    let (done_tx, done_rx) = bounded::<bool>(1);
    let block = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
        let _ = done_tx.try_send(granted.as_bool());
    });
    let service = NSString::from_str(TCC_SERVICE);
    unsafe { request(Retained::as_ptr(&service) as *const c_void, null(), &block) };
    match done_rx.recv_timeout(Duration::from_secs(120)) {
        Ok(true) => TapPermission::Granted,
        Ok(false) => TapPermission::Denied,
        Err(_) => permission(),
    }
}
