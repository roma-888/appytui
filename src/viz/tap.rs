//! Core Audio process tap on Music.app (or a global tap as fallback), wrapped
//! in a private aggregate device. Mirrors cidre's `core-audio-record` example.

use anyhow::{Result, anyhow};
use cidre::core_audio::aggregate_device_keys as agg_keys;
use cidre::core_audio::sub_device_keys as sub_keys;
use cidre::{cat, cf, core_audio as ca, ns, os};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};

/// Interleaved stereo frames kept between reads (~0.17 s at 48 kHz).
const RING_FRAMES: usize = 8192;

struct TapCtx {
    prod: ringbuf::HeapProd<f32>,
}

extern "C" fn io_proc(
    _device: ca::Device,
    _now: &cat::AudioTimeStamp,
    input: &cat::AudioBufList<1>,
    _input_time: &cat::AudioTimeStamp,
    _output: &mut cat::AudioBufList<1>,
    _output_time: &cat::AudioTimeStamp,
    ctx: Option<&mut TapCtx>,
) -> os::Status {
    if let Some(ctx) = ctx {
        let buf = &input.buffers[0];
        let n = buf.data_bytes_size as usize / std::mem::size_of::<f32>();
        if n > 0 && !buf.data.is_null() {
            // SAFETY: Core Audio hands us `data_bytes_size` bytes of f32 samples
            // (format verified as float32 in `Tap::open`) valid for this callback.
            let samples = unsafe { std::slice::from_raw_parts(buf.data as *const f32, n) };
            let _ = ctx.prod.push_slice(samples); // drop on overflow; the UI only needs the newest window
        }
    }
    Default::default()
}

pub struct Tap {
    // Field order is drop order: stop the device, destroy the tap, then free the buffers.
    _started: ca::hardware::StartedDevice<ca::AggregateDevice>,
    _tap: ca::TapGuard,
    _ctx: Box<TapCtx>,
    cons: ringbuf::HeapCons<f32>,
    sample_rate: f32,
    device_uid: String,
    process_tap: bool,
}

fn default_output_uid() -> Result<(ca::Device, String)> {
    let dev = ca::System::default_output_device().map_err(|e| anyhow!("default output device: {e:?}"))?;
    let uid = dev.uid().map_err(|e| anyhow!("device uid: {e:?}"))?;
    Ok((dev, uid.to_string()))
}

impl Tap {
    /// Tap Music.app (by `pid`) or, if that fails or `pid` is `None`, all system audio.
    pub fn open(pid: Option<u32>) -> Result<Tap> {
        let (desc, process_tap) = match pid.and_then(|p| ca::Process::with_pid(p as i32).ok()) {
            Some(proc_obj) => {
                let num = ns::Number::with_u32(proc_obj.0.0);
                let n: &ns::Number = &num;
                (ca::TapDesc::with_stereo_mixdown_of_processes(&ns::Array::from_slice(&[n])), true)
            }
            None => (ca::TapDesc::with_stereo_global_tap_excluding_processes(&ns::Array::new()), false),
        };
        let tap = desc.create_process_tap().map_err(|e| {
            anyhow!(
                "creating audio tap ({e:?}). Allow system audio recording for your terminal in System Settings > Privacy & Security > Screen & System Audio Recording"
            )
        })?;
        let asbd = tap.asbd().map_err(|e| anyhow!("tap format: {e:?}"))?;
        if !asbd.format_flags.contains(cat::AudioFormatFlags::IS_FLOAT) || asbd.channels_per_frame != 2 {
            return Err(anyhow!(
                "unexpected tap format: {} ch, flags {:?}",
                asbd.channels_per_frame,
                asbd.format_flags
            ));
        }

        let (output, device_uid) = default_output_uid()?;
        let output_uid = output.uid().map_err(|e| anyhow!("device uid: {e:?}"))?;
        let sub_device = cf::DictionaryOf::with_keys_values(&[sub_keys::uid()], &[output_uid.as_type_ref()]);
        let tap_uid = tap.uid().map_err(|e| anyhow!("tap uid: {e:?}"))?;
        let sub_tap = cf::DictionaryOf::with_keys_values(&[sub_keys::uid()], &[tap_uid.as_type_ref()]);
        let dict = cf::DictionaryOf::with_keys_values(
            &[
                agg_keys::is_private(),
                agg_keys::is_stacked(),
                agg_keys::tap_auto_start(),
                agg_keys::name(),
                agg_keys::main_sub_device(),
                agg_keys::uid(),
                agg_keys::sub_device_list(),
                agg_keys::tap_list(),
            ],
            &[
                cf::Boolean::value_true().as_type_ref(),
                cf::Boolean::value_false(),
                cf::Boolean::value_true(),
                cf::str!(c"appytui"),
                &output_uid,
                &cf::Uuid::new().to_cf_string(),
                &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
                &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
            ],
        );
        let agg = ca::AggregateDevice::with_desc(&dict).map_err(|e| anyhow!("aggregate device: {e:?}"))?;

        let (prod, cons) = HeapRb::<f32>::new(RING_FRAMES * 2).split();
        let mut ctx = Box::new(TapCtx { prod });
        let proc_id = agg.create_io_proc_id(io_proc, Some(&mut *ctx)).map_err(|e| anyhow!("io proc: {e:?}"))?;
        let started = ca::device_start(agg, Some(proc_id)).map_err(|e| anyhow!("starting device: {e:?}"))?;

        Ok(Tap {
            _started: started,
            _tap: tap,
            _ctx: ctx,
            cons,
            sample_rate: asbd.sample_rate as f32,
            device_uid,
            process_tap,
        })
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn is_process_tap(&self) -> bool {
        self.process_tap
    }

    /// Append every sample captured since the last call.
    pub fn read(&mut self, out: &mut Vec<f32>) {
        let mut chunk = [0.0f32; 1024];
        loop {
            let n = self.cons.pop_slice(&mut chunk);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..n]);
        }
    }

    /// True when the default output device is no longer the one we tapped.
    pub fn device_changed(&self) -> bool {
        default_output_uid().map(|(_, uid)| uid != self.device_uid).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn music_pid() -> Option<u32> {
        std::process::Command::new("pgrep")
            .args(["-x", "Music"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().and_then(|l| l.trim().parse().ok()))
    }

    /// Open a tap, sample it for a while, report (samples, rms) over the whole window.
    fn measure(pid: Option<u32>, secs: u64) -> (usize, f32) {
        let mut tap = Tap::open(pid).unwrap();
        println!("process tap: {} at {} Hz", tap.is_process_tap(), tap.sample_rate());
        let mut total = 0usize;
        let mut sq = 0.0f32;
        let mut out = Vec::new();
        for _ in 0..(secs * 10) {
            std::thread::sleep(std::time::Duration::from_millis(100));
            out.clear();
            tap.read(&mut out);
            total += out.len();
            sq += out.iter().map(|s| s * s).sum::<f32>();
        }
        let rms = (sq / total.max(1) as f32).sqrt();
        println!("samples: {total} rms: {rms}");
        (total, rms)
    }

    #[test]
    #[ignore = "needs audio capture permission and Music.app playing"]
    fn live_tap_delivers_samples() {
        let (n, _) = measure(music_pid(), 2);
        assert!(n > 10_000, "got {n} samples");
    }

    #[test]
    #[ignore = "needs audio capture permission and Music.app playing"]
    fn live_global_tap_delivers_samples() {
        let (n, _) = measure(None, 2);
        assert!(n > 10_000, "got {n} samples");
    }
}
