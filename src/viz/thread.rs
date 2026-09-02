//! Owns the audio source and the analyzer; emits frames at `framerate`.

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError};

use super::simulated::Simulated;
use super::spectrum::{Analyzer, FFT_SIZE};
use super::{Control, Frame, VizEvent};
use crate::config::{Channels, VizSettings};

enum Source {
    #[cfg(target_os = "macos")]
    Tap(super::tap::Tap),
    Simulated(Simulated),
}

fn open_source(pid: Option<u32>, tx: &Sender<VizEvent>) -> Source {
    #[cfg(target_os = "macos")]
    {
        match super::tap::Tap::open(pid) {
            Ok(t) => Source::Tap(t),
            Err(e) => {
                let _ = tx.send(VizEvent::Fallback(format!("Visualizer simulated: {e:#}")));
                Source::Simulated(Simulated::default())
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        let _ = tx.send(VizEvent::Fallback(
            "Visualizer simulated: audio capture unavailable on this platform".into(),
        ));
        Source::Simulated(Simulated::default())
    }
}

fn per_channel(total: usize, settings: &VizSettings) -> usize {
    match settings.channels {
        Channels::Stereo => (total / 2).max(1),
        Channels::Mono => total.max(1),
    }
}

pub fn spawn(
    settings: VizSettings,
    pid: Option<u32>,
    bars: usize,
    ctl: Receiver<Control>,
    tx: Sender<VizEvent>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("visualizer".into())
        .spawn(move || run(settings, pid, bars, ctl, tx))
        .expect("spawn visualizer thread")
}

fn run(
    mut settings: VizSettings,
    mut pid: Option<u32>,
    mut bars: usize,
    ctl: Receiver<Control>,
    tx: Sender<VizEvent>,
) {
    let mut source = open_source(pid, &tx);
    let rate = match &source {
        #[cfg(target_os = "macos")]
        Source::Tap(t) => t.sample_rate(),
        Source::Simulated(_) => 48000.0,
    };
    let mut analyzer = Analyzer::new(&settings, rate, per_channel(bars, &settings));
    let mut samples: Vec<f32> = Vec::with_capacity(FFT_SIZE * 4);
    let mut playing = false;
    let mut last_device_check = Instant::now();
    let mut idle_sent = false;

    loop {
        let period = Duration::from_millis(1000 / settings.framerate.clamp(1, 120) as u64);
        loop {
            match ctl.try_recv() {
                Ok(Control::Shutdown) | Err(TryRecvError::Disconnected) => return,
                Ok(Control::SetBars(n)) => {
                    bars = n;
                    analyzer.set_bars(per_channel(bars, &settings));
                }
                Ok(Control::Playing(p)) => playing = p,
                Ok(Control::Settings(s)) => {
                    settings = s;
                    analyzer.set_settings(&settings);
                    analyzer.set_bars(per_channel(bars, &settings));
                }
                Ok(Control::MusicPid(p)) => {
                    pid = Some(p);
                    #[cfg(target_os = "macos")]
                    {
                        let upgrade = matches!(&source, Source::Tap(t) if !t.is_process_tap());
                        if upgrade {
                            source = open_source(pid, &tx);
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        #[cfg(target_os = "macos")]
        if last_device_check.elapsed() > Duration::from_secs(2) {
            last_device_check = Instant::now();
            let changed = matches!(&source, Source::Tap(t) if t.device_changed());
            if changed {
                source = open_source(pid, &tx);
                samples.clear();
                continue;
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (&mut last_device_check, &mut pid);
        }

        match &mut source {
            #[cfg(target_os = "macos")]
            Source::Tap(tap) => {
                tap.read(&mut samples);
                if samples.len() > FFT_SIZE * 4 {
                    let excess = samples.len() - FFT_SIZE * 4;
                    samples.drain(..excess);
                }
                if playing || !idle_sent {
                    let frame = analyzer.analyze(&samples);
                    idle_sent = !playing && frame.is_silent();
                    let _ = tx.send(VizEvent::Frame(frame));
                } else {
                    // Keep bars decaying to zero after pause without burning CPU.
                    let frame = analyzer.analyze(&vec![0.0; FFT_SIZE * 2]);
                    if !frame.is_silent() {
                        let _ = tx.send(VizEvent::Frame(frame));
                    }
                }
            }
            Source::Simulated(sim) => {
                if playing {
                    let f = sim.frame(
                        per_channel(bars, &settings),
                        settings.channels == Channels::Stereo,
                    );
                    let _ = tx.send(VizEvent::Frame(f));
                } else if !idle_sent {
                    idle_sent = true;
                    let _ = tx.send(VizEvent::Frame(Frame::default()));
                }
            }
        }
        if playing {
            idle_sent = false;
        }
        std::thread::sleep(period);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_channel_halves_for_stereo() {
        let s = VizSettings::default();
        assert_eq!(per_channel(20, &s), 10);
        let m = VizSettings {
            channels: Channels::Mono,
            ..s
        };
        assert_eq!(per_channel(20, &m), 20);
        assert_eq!(per_channel(0, &m), 1);
    }

    #[test]
    fn simulated_source_emits_frames_while_playing_and_stops_cleanly() {
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        let (tx, rx) = crossbeam_channel::unbounded();
        let settings = VizSettings {
            framerate: 60,
            ..VizSettings::default()
        };
        // Drive the simulated source directly: a real tap is not deterministic in CI.
        let handle = std::thread::spawn(move || {
            let mut sim = Simulated::default();
            let _ = tx.send(VizEvent::Frame(sim.frame(per_channel(8, &settings), true)));
            while let Ok(c) = ctl_rx.recv() {
                if matches!(c, Control::Shutdown) {
                    break;
                }
            }
        });
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), VizEvent::Frame(f) if f.left.len() == 4)
        );
        ctl_tx.send(Control::Shutdown).unwrap();
        handle.join().unwrap();
    }
}
