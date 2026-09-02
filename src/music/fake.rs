//! In-memory bridge for tests.

use anyhow::Result;

use super::MusicBridge;
use super::model::{PlayerState, PlayerStatus, Playlist, PlaylistId, RepeatMode, Track, TrackId};

#[derive(Debug, Default)]
pub struct FakeBridge {
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub status: PlayerStatus,
    /// Every mutating call, in order, for assertions.
    pub calls: Vec<String>,
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

impl MusicBridge for FakeBridge {
    fn load_library(&mut self) -> Result<Vec<Track>> {
        Ok(self.tracks.clone())
    }
    fn load_playlists(&mut self) -> Result<Vec<Playlist>> {
        Ok(self.playlists.clone())
    }
    fn status(&mut self) -> Result<PlayerStatus> {
        Ok(self.status.clone())
    }
    fn music_pid(&mut self) -> Result<u32> {
        Ok(4242)
    }
    fn play_track(&mut self, track: &TrackId, context: Option<&PlaylistId>) -> Result<()> {
        self.calls.push(format!(
            "play_track {} {:?}",
            track.0,
            context.map(|c| c.0.clone())
        ));
        self.status.state = PlayerState::Playing;
        self.status.track_id = Some(track.clone());
        self.status.position_secs = 0.0;
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
        b.play_track(&TrackId("A".into()), None).unwrap();
        b.play_pause().unwrap();
        assert_eq!(b.calls, vec!["play_track A None", "play_pause"]);
        assert_eq!(b.status().unwrap().state, PlayerState::Paused);
    }
}
