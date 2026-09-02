//! Bridge to Music.app. The real implementation is `jxa::JxaBridge`; tests use `fake::FakeBridge`.

use anyhow::Result;

#[cfg(test)]
pub mod fake;
pub mod jxa;
pub mod model;
pub mod worker;

use model::{PlayerStatus, Playlist, RepeatMode, Track, TrackId};

pub trait MusicBridge: Send {
    /// Start Music.app if it is not running (without stealing focus).
    fn ensure_running(&mut self) -> Result<()>;
    fn load_library(&mut self) -> Result<Vec<Track>>;
    fn load_playlists(&mut self) -> Result<Vec<Playlist>>;
    fn status(&mut self) -> Result<PlayerStatus>;
    fn music_pid(&mut self) -> Result<u32>;
    /// Play `tracks` in order, starting with the first, as one playlist.
    fn play_tracks(&mut self, tracks: &[TrackId]) -> Result<()>;
    /// Copy `tracks` into the idle playlist without disturbing playback, so
    /// `play_prepared` can switch to them with one call.
    fn prepare_tracks(&mut self, tracks: &[TrackId]) -> Result<()>;
    /// Start the playlist filled by the last `prepare_tracks`.
    fn play_prepared(&mut self) -> Result<()>;
    fn play_pause(&mut self) -> Result<()>;
    fn next(&mut self) -> Result<()>;
    fn previous(&mut self) -> Result<()>;
    fn seek(&mut self, seconds: f64) -> Result<()>;
    fn set_volume(&mut self, percent: u8) -> Result<()>;
    fn set_shuffle(&mut self, on: bool) -> Result<()>;
    fn set_repeat(&mut self, mode: RepeatMode) -> Result<()>;
}

/// Sent from the app to the bridge worker.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    LoadLibrary,
    LoadPlaylists,
    PlayTracks(Vec<TrackId>),
    PrepareTracks(Vec<TrackId>),
    PlayPrepared,
    PlayPause,
    Next,
    Previous,
    Seek(f64),
    SetVolume(u8),
    SetShuffle(bool),
    SetRepeat(RepeatMode),
    Shutdown,
}

/// Sent from the bridge worker to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Library(Vec<Track>),
    Playlists(Vec<Playlist>),
    Status(PlayerStatus),
    MusicPid(u32),
    Error(String),
}
