//! FFT → log-spaced bands → cava-style smoothing. No I/O; fully unit tested.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::config::{Channels, MonoOption, VizSettings};

pub const FFT_SIZE: usize = 2048;
/// Samples kept for waveform mode.
pub const WAVEFORM_LEN: usize = 512;

/// One visualizer frame. Bar values are 0..=1. `right` is empty in mono mode.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frame {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    /// Recent mono samples in -1..=1, newest last.
    pub waveform: Vec<f32>,
}

impl Frame {
    pub fn is_silent(&self) -> bool {
        self.left.iter().chain(self.right.iter()).all(|v| *v < 0.01)
    }
}

#[derive(Debug, Clone, Default)]
struct Smoother {
    /// Displayed value per bar after gravity + integral smoothing.
    value: Vec<f32>,
    /// Frames since each bar started falling (gravity accelerates).
    fall: Vec<u32>,
}

impl Smoother {
    fn resize(&mut self, n: usize) {
        self.value = vec![0.0; n];
        self.fall = vec![0; n];
    }

    /// `noise` is 0..1 (noise_reduction / 100).
    fn apply(&mut self, raw: &[f32], noise: f32) -> Vec<f32> {
        let gravity = (1.0 - noise) * 0.02 + 0.002;
        let integral = noise * 0.9;
        raw.iter()
            .enumerate()
            .map(|(i, &r)| {
                let prev = self.value[i];
                let target = if r >= prev {
                    self.fall[i] = 0;
                    r
                } else {
                    self.fall[i] += 1;
                    let f = self.fall[i] as f32;
                    (prev - gravity * f * f).max(r)
                };
                let v = (prev * integral + target * (1.0 - integral)).min(target.max(r));
                let v = if r >= prev {
                    v.max(r * (1.0 - integral))
                } else {
                    v
                };
                self.value[i] = v.clamp(0.0, 1.0);
                self.value[i]
            })
            .collect()
    }
}

/// cava's "monstercat" filter: each bar lifts its neighbours by 1/1.5^distance
/// (or, with `waves`, by a wider quadratic falloff).
#[allow(clippy::needless_range_loop)] // nested index loops read clearer than iterator zips here
pub fn monstercat(bars: &mut [f32], waves: bool) {
    let n = bars.len();
    let src = bars.to_vec();
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let d = (i as isize - j as isize).unsigned_abs() as f32;
            let lifted = if waves {
                src[i] - d * d * 0.02
            } else {
                src[i] / 1.5f32.powf(d)
            };
            if lifted > bars[j] {
                bars[j] = lifted;
            }
        }
    }
}

pub struct Analyzer {
    settings: VizSettings,
    sample_rate: f32,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    scratch: Vec<Complex<f32>>,
    /// Inclusive-exclusive FFT bin ranges per bar.
    bands: Vec<(usize, usize)>,
    smooth: [Smoother; 2],
    /// Auto-sensitivity multiplier (cava style): drops fast on overshoot, creeps up otherwise.
    sens: f32,
    /// Per-band weight: a gentle treble tilt so the spectrum is not all bass.
    weights: Vec<f32>,
}

impl Analyzer {
    pub fn new(settings: &VizSettings, sample_rate: f32, bars_per_channel: usize) -> Analyzer {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE as f32 - 1.0)).cos()
            })
            .collect();
        let mut a = Analyzer {
            settings: settings.clone(),
            sample_rate,
            fft,
            window,
            scratch: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            bands: Vec::new(),
            smooth: [Smoother::default(), Smoother::default()],
            sens: 0.0,
            weights: Vec::new(),
        };
        a.set_bars(bars_per_channel);
        a
    }

    pub fn bars(&self) -> usize {
        self.bands.len()
    }

    pub fn set_settings(&mut self, s: &VizSettings) {
        let n = self.bars();
        self.settings = s.clone();
        self.set_bars(n);
    }

    pub fn set_bars(&mut self, n: usize) {
        let n = n.max(1);
        let lo = self.settings.lower_cutoff_freq.max(1) as f32;
        let hi = (self.settings.higher_cutoff_freq as f32)
            .min(self.sample_rate / 2.0)
            .max(lo * 1.1);
        let bin_of = |f: f32| {
            ((f * FFT_SIZE as f32 / self.sample_rate).round() as usize).clamp(1, FFT_SIZE / 2 - 1)
        };
        self.bands = (0..n)
            .map(|k| {
                let f0 = lo * (hi / lo).powf(k as f32 / n as f32);
                let f1 = lo * (hi / lo).powf((k + 1) as f32 / n as f32);
                let (b0, mut b1) = (bin_of(f0), bin_of(f1));
                if b1 <= b0 {
                    b1 = b0 + 1;
                }
                (b0, b1)
            })
            .collect();
        let bin_hz = self.sample_rate / FFT_SIZE as f32;
        self.weights = self
            .bands
            .iter()
            .map(|&(b0, b1)| {
                let centre = (b0 + b1) as f32 * 0.5 * bin_hz;
                (centre / 1000.0).max(0.05).powf(0.35)
            })
            .collect();
        for s in &mut self.smooth {
            s.resize(n);
        }
    }

    /// Index of the bar whose band contains `freq`, if any.
    #[cfg(test)]
    pub fn band_for_freq(&self, freq: f32) -> Option<usize> {
        let bin = (freq * FFT_SIZE as f32 / self.sample_rate).round() as usize;
        self.bands
            .iter()
            .position(|(lo, hi)| bin >= *lo && bin < *hi)
    }

    /// `interleaved` holds L/R pairs; the newest `FFT_SIZE` frames are analysed.
    pub fn analyze(&mut self, interleaved: &[f32]) -> Frame {
        let frames = interleaved.len() / 2;
        let start = frames.saturating_sub(FFT_SIZE) * 2;
        let recent = &interleaved[start..];
        let mut left: Vec<f32> = recent.chunks(2).map(|c| c[0]).collect();
        let mut right: Vec<f32> = recent
            .chunks(2)
            .map(|c| c.get(1).copied().unwrap_or(c[0]))
            .collect();
        left.resize(FFT_SIZE, 0.0);
        right.resize(FFT_SIZE, 0.0);

        let waveform: Vec<f32> = left
            .iter()
            .zip(&right)
            .map(|(l, r)| (l + r) * 0.5)
            .skip(FFT_SIZE.saturating_sub(WAVEFORM_LEN))
            .collect();

        let noise = (self.settings.noise_reduction.min(100) as f32) / 100.0;
        match self.settings.channels {
            Channels::Mono => {
                let mono: Vec<f32> = match self.settings.mono_option {
                    MonoOption::Left => left,
                    MonoOption::Right => right,
                    MonoOption::Average => left
                        .iter()
                        .zip(&right)
                        .map(|(l, r)| (l + r) * 0.5)
                        .collect(),
                };
                let raw = self.magnitudes(&mono);
                let bars = self.finish(0, raw, noise);
                Frame {
                    left: bars,
                    right: Vec::new(),
                    waveform,
                }
            }
            Channels::Stereo => {
                let raw_l = self.magnitudes(&left);
                let raw_r = self.magnitudes(&right);
                let l = self.finish(0, raw_l, noise);
                let r = self.finish(1, raw_r, noise);
                Frame {
                    left: l,
                    right: r,
                    waveform,
                }
            }
        }
    }

    /// Windowed FFT → weighted, square-root-compressed mean magnitude per band.
    fn magnitudes(&mut self, samples: &[f32]) -> Vec<f32> {
        for (i, s) in self.scratch.iter_mut().enumerate() {
            *s = Complex::new(samples[i] * self.window[i], 0.0);
        }
        self.fft.process(&mut self.scratch);
        self.bands
            .iter()
            .zip(&self.weights)
            .map(|(&(lo, hi), w)| {
                let sum: f32 = self.scratch[lo..hi].iter().map(|c| c.norm()).sum();
                let mean = sum / (hi - lo) as f32 / (FFT_SIZE as f32 / 4.0);
                (mean * w).sqrt()
            })
            .collect()
    }

    /// Linear magnitudes → 0..1 with auto-sensitivity, then monstercat/waves and smoothing.
    fn finish(&mut self, ch: usize, raw: Vec<f32>, noise: f32) -> Vec<f32> {
        let max = raw.iter().cloned().fold(0.0, f32::max);
        // Digital silence: nothing to scale, render empty.
        if max < 1e-4 {
            return self.smooth[ch].apply(&vec![0.0; raw.len()], noise);
        }
        let user_gain = self.settings.sensitivity.max(1) as f32 / 100.0;
        if self.settings.autosens {
            if self.sens <= 0.0 {
                // First audible frame: put the loudest bar at 80 %.
                self.sens = 0.8 / max;
            } else if max * self.sens > 1.0 {
                // Overshoot: back off quickly so peaks are never clipped for long.
                self.sens *= 0.95;
            } else {
                // Creep up so quiet passages still fill the pane.
                self.sens *= 1.002;
            }
        } else {
            self.sens = 1.0;
        }
        let gain = self.sens * user_gain;
        let mut bars: Vec<f32> = raw.iter().map(|v| (v * gain).clamp(0.0, 1.0)).collect();
        if self.settings.monstercat || self.settings.waves {
            monstercat(&mut bars, self.settings.waves);
        }
        self.smooth[ch].apply(&bars, noise)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: f32, n: usize) -> Vec<f32> {
        (0..n)
            .flat_map(|i| {
                let v = (2.0 * std::f32::consts::PI * freq * i as f32 / rate).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    fn mono_settings() -> VizSettings {
        VizSettings {
            channels: Channels::Mono,
            monstercat: false,
            waves: false,
            ..VizSettings::default()
        }
    }

    #[test]
    fn band_edges_are_log_spaced_and_nonempty() {
        let a = Analyzer::new(&mono_settings(), 48000.0, 16);
        assert_eq!(a.bands.len(), 16);
        for (lo, hi) in &a.bands {
            assert!(hi > lo, "{lo}..{hi}");
        }
        assert!(a.bands[0].0 >= 2);
        assert!(a.bands[15].1 <= 214);
    }

    #[test]
    fn silence_is_all_zero() {
        let mut a = Analyzer::new(&mono_settings(), 48000.0, 16);
        let f = a.analyze(&vec![0.0; FFT_SIZE * 2]);
        assert_eq!(f.left.len(), 16);
        assert!(f.right.is_empty());
        assert!(f.is_silent());
    }

    #[test]
    fn sine_lights_the_matching_band() {
        let mut a = Analyzer::new(&mono_settings(), 48000.0, 16);
        let samples = sine(440.0, 48000.0, FFT_SIZE);
        let mut f = Frame::default();
        for _ in 0..5 {
            f = a.analyze(&samples);
        }
        let expected = a.band_for_freq(440.0).unwrap();
        let (best, _) = f
            .left
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(y.1))
            .unwrap();
        assert_eq!(best, expected, "{:?}", f.left);
        assert!(f.left[best] > 0.5);
    }

    #[test]
    fn stereo_analyses_both_channels() {
        let s = VizSettings {
            monstercat: false,
            waves: false,
            ..VizSettings::default()
        };
        let mut a = Analyzer::new(&s, 48000.0, 8);
        let samples: Vec<f32> = sine(1000.0, 48000.0, FFT_SIZE)
            .chunks(2)
            .flat_map(|c| [c[0], 0.0])
            .collect();
        let mut f = Frame::default();
        for _ in 0..5 {
            f = a.analyze(&samples);
        }
        assert!(f.left.iter().cloned().fold(0.0, f32::max) > 0.5);
        assert!(f.right.iter().all(|v| *v < 0.05));
    }

    #[test]
    fn smoothing_falls_monotonically_after_tone_stops() {
        let mut a = Analyzer::new(&mono_settings(), 48000.0, 16);
        let tone = sine(440.0, 48000.0, FFT_SIZE);
        for _ in 0..10 {
            a.analyze(&tone);
        }
        let silence = vec![0.0; FFT_SIZE * 2];
        let mut prev = f32::MAX;
        for _ in 0..30 {
            let f = a.analyze(&silence);
            let peak = f.left.iter().cloned().fold(0.0, f32::max);
            assert!(peak <= prev + 1e-6, "{peak} > {prev}");
            prev = peak;
        }
        assert!(prev < 0.05);
    }

    #[test]
    fn quieter_tone_renders_visibly_lower_than_the_loud_one() {
        let mut a = Analyzer::new(&mono_settings(), 48000.0, 24);
        // 200 Hz at full level plus 3 kHz at -20 dB.
        let n = FFT_SIZE;
        let samples: Vec<f32> = (0..n)
            .flat_map(|i| {
                let t = i as f32 / 48000.0;
                let v = (2.0 * std::f32::consts::PI * 200.0 * t).sin() * 0.5
                    + (2.0 * std::f32::consts::PI * 3000.0 * t).sin() * 0.05;
                [v, v]
            })
            .collect();
        let mut f = Frame::default();
        for _ in 0..40 {
            f = a.analyze(&samples);
        }
        let loud = f.left[a.band_for_freq(200.0).unwrap()];
        let quiet = f.left[a.band_for_freq(3000.0).unwrap()];
        assert!(loud > 0.6, "loud band {loud}");
        assert!(quiet < loud * 0.7, "quiet band {quiet} vs loud {loud}");
        assert!(quiet > 0.05, "quiet band should still be visible: {quiet}");
        // A band with no energy at all stays near zero.
        let empty = f.left[a.band_for_freq(900.0).unwrap()];
        assert!(empty < 0.25, "empty band {empty}");
    }

    #[test]
    fn autosens_recovers_after_a_loud_passage() {
        let mut a = Analyzer::new(&mono_settings(), 48000.0, 16);
        let loud = sine(440.0, 48000.0, FFT_SIZE);
        let quiet: Vec<f32> = loud.iter().map(|v| v * 0.1).collect();
        for _ in 0..20 {
            a.analyze(&loud);
        }
        // Let gravity settle the bars at the quiet level before measuring.
        let mut settled = 0.0;
        for _ in 0..40 {
            settled = a.analyze(&quiet).left.iter().cloned().fold(0.0, f32::max);
        }
        let mut later = 0.0;
        for _ in 0..600 {
            later = a.analyze(&quiet).left.iter().cloned().fold(0.0, f32::max);
        }
        assert!(
            later > settled + 0.1,
            "sens did not creep up: {settled} -> {later}"
        );
        assert!(later <= 1.0);
    }

    #[test]
    fn monstercat_spreads_to_neighbours() {
        let mut bars = vec![0.0, 0.0, 1.0, 0.0, 0.0];
        monstercat(&mut bars, false);
        assert!(bars[1] > 0.6 && bars[3] > 0.6);
        assert!(bars[0] > 0.4 && bars[0] < bars[1]);
    }

    #[test]
    fn set_bars_resizes_state() {
        let mut a = Analyzer::new(&mono_settings(), 48000.0, 4);
        a.set_bars(9);
        let f = a.analyze(&vec![0.0; FFT_SIZE * 2]);
        assert_eq!(f.left.len(), 9);
        assert_eq!(f.waveform.len(), WAVEFORM_LEN);
    }
}
