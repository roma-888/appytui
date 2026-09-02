//! In-memory bridge for tests.

use anyhow::Result;

use super::MusicBridge;
use super::model::{PlayerState, PlayerStatus, Playlist, RepeatMode, Track, TrackId};

#[derive(Debug, Default)]
pub struct FakeBridge {
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub status: PlayerStatus,
    /// Every mutating call, in order, for assertions.
    pub calls: Vec<String>,
    /// When true, `status()` fails until `ensure_running()` is called.
    pub fail_status: bool,
}

impl FakeBridge {
    pub fn with_tracks(tracks: Vec<Track>) -> Self {
        Self {
            tracks,
            ..Default::default()
        }
    }
}

pub fn track(id: &str, name: &str, artist: &str, album: &str) -> Track {
    Track {
        id: TrackId(id.to_string()),
        name: name.to_string(),
        artist: artist.to_string(),
        album: album.to_string(),
        album_artist: String::new(),
        duration_secs: 200.0,
        track_number: 1,
        disc_number: 1,
        year: 2000,
    }
}

impl FakeBridge {
    fn start(&mut self, track: &TrackId) {
        self.status.state = PlayerState::Playing;
        self.status.track_id = Some(track.clone());
        self.status.position_secs = 0.0;
    }
}

impl MusicBridge for FakeBridge {
    fn ensure_running(&mut self) -> Result<()> {
        self.calls.push("ensure_running".into());
        self.fail_status = false;
        Ok(())
    }
    fn load_library(&mut self) -> Result<Vec<Track>> {
        Ok(self.tracks.clone())
    }
    fn load_playlists(&mut self) -> Result<Vec<Playlist>> {
        Ok(self.playlists.clone())
    }
    fn status(&mut self) -> Result<PlayerStatus> {
        if self.fail_status {
            anyhow::bail!("osascript failed: Music got an error: Connection is invalid");
        }
        Ok(self.status.clone())
    }
    fn music_pid(&mut self) -> Result<u32> {
        Ok(4242)
    }
    fn play_tracks(&mut self, tracks: &[TrackId]) -> Result<()> {
        let ids: Vec<&str> = tracks.iter().map(|t| t.0.as_str()).collect();
        self.calls.push(format!("play_tracks [{}]", ids.join(",")));
        if let Some(first) = tracks.first() {
            self.start(first);
        }
        Ok(())
    }
    fn play_pause(&mut self) -> Result<()> {
        self.calls.push("play_pause".into());
        self.status.state = match self.status.state {
            PlayerState::Playing => PlayerState::Paused,
            _ => PlayerState::Playing,
        };
        Ok(())
    }
    fn next(&mut self) -> Result<()> {
        self.calls.push("next".into());
        Ok(())
    }
    fn previous(&mut self) -> Result<()> {
        self.calls.push("previous".into());
        Ok(())
    }
    fn seek(&mut self, seconds: f64) -> Result<()> {
        self.calls.push(format!("seek {seconds}"));
        self.status.position_secs = seconds;
        Ok(())
    }
    fn set_volume(&mut self, percent: u8) -> Result<()> {
        self.calls.push(format!("set_volume {percent}"));
        self.status.volume = percent;
        Ok(())
    }
    fn set_shuffle(&mut self, on: bool) -> Result<()> {
        self.calls.push(format!("set_shuffle {on}"));
        self.status.shuffle = on;
        Ok(())
    }
    fn set_repeat(&mut self, mode: RepeatMode) -> Result<()> {
        self.calls.push(format!("set_repeat {}", mode.as_str()));
        self.status.repeat = mode;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records_calls_and_updates_status() {
        let mut b = FakeBridge::with_tracks(vec![track("A", "Song", "Artist", "Album")]);
        b.play_tracks(&[TrackId("B".into()), TrackId("A".into())])
            .unwrap();
        assert_eq!(b.status().unwrap().track_id, Some(TrackId("B".into())));
        b.play_pause().unwrap();
        assert_eq!(b.calls, vec!["play_tracks [B,A]", "play_pause"]);
        assert_eq!(b.status().unwrap().state, PlayerState::Paused);
    }
}
