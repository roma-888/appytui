#![allow(dead_code)] // wired into the app in Task 11
//! Audio visualizer: capture (tap), analysis (spectrum) and the simulated fallback.

pub mod simulated;
pub mod spectrum;
#[cfg(target_os = "macos")]
pub mod tap;
pub mod thread;

pub use spectrum::Frame;

use crate::config::VizSettings;

/// App → visualizer thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// Total bars across the pane (stereo splits them between channels).
    SetBars(usize),
    Playing(bool),
    Settings(VizSettings),
    MusicPid(u32),
    Shutdown,
}

/// Visualizer thread → app.
#[derive(Debug, Clone, PartialEq)]
pub enum VizEvent {
    Frame(Frame),
    /// Capture failed; the simulated source is active. Carries the status hint.
    Fallback(String),
}
