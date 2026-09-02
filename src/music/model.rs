//! Data model shared by the bridge, the app state and the UI.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaylistId(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub duration_secs: f64,
    pub track_number: u32,
    pub disc_number: u32,
    pub year: u32,
}

impl Track {
    /// Artist used for grouping albums and artists.
    pub fn grouping_artist(&self) -> &str {
        if self.album_artist.is_empty() {
            &self.artist
        } else {
            &self.album_artist
        }
    }
}

/// How the current play context was started. Music.app continues playback
/// within the source, so it decides what "next" and shuffle mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaySource {
    /// A single library track, as from the Songs list; Music.app follows it
    /// with Autoplay.
    #[default]
    Library,
    /// A list (album, artist or playlist) copied into appytui's own playlist,
    /// so Music.app keeps playing within it.
    OwnPlaylist,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub smart: bool,
    pub track_ids: Vec<TrackId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    Off,
    One,
    All,
}

impl RepeatMode {
    pub fn next(self) -> RepeatMode {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::One => "one",
            RepeatMode::All => "all",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerStatus {
    pub state: PlayerState,
    pub track_id: Option<TrackId>,
    /// Metadata snapshot of the current track. Streamed Apple Music tracks are
    /// often not in the library, so this is the fallback for the now-playing pane.
    #[serde(default)]
    pub track: Option<Track>,
    pub position_secs: f64,
    pub volume: u8,
    pub shuffle: bool,
    pub repeat: RepeatMode,
}

impl Default for PlayerStatus {
    fn default() -> Self {
        Self {
            state: PlayerState::Stopped,
            track_id: None,
            track: None,
            position_secs: 0.0,
            volume: 100,
            shuffle: false,
            repeat: RepeatMode::Off,
        }
    }
}

/// Format seconds as m:ss (or h:mm:ss).
pub fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parses_captured_jxa_output() {
        let json = r#"{"state":"paused","track_id":"EFF3D244B345952B","position_secs":55.34,"volume":100,"shuffle":false,"repeat":"off"}"#;
        let s: PlayerStatus = serde_json::from_str(json).unwrap();
        assert_eq!(s.state, PlayerState::Paused);
        assert_eq!(s.track_id, Some(TrackId("EFF3D244B345952B".into())));
        assert_eq!(s.repeat, RepeatMode::Off);
    }

    #[test]
    fn status_carries_track_snapshot_when_present() {
        let json = r#"{"state":"playing","track_id":"X","track":{"id":"X","name":"Pretty Pure","artist":"whenyoung","album":"Single","album_artist":"","duration_secs":237.8,"track_number":0,"disc_number":0,"year":0},"position_secs":1,"volume":100,"shuffle":false,"repeat":"off"}"#;
        let s: PlayerStatus = serde_json::from_str(json).unwrap();
        assert_eq!(
            s.track.as_ref().map(|t| t.name.as_str()),
            Some("Pretty Pure")
        );
    }

    #[test]
    fn stopped_status_has_null_track() {
        let json = r#"{"state":"stopped","track_id":null,"position_secs":0,"volume":50,"shuffle":true,"repeat":"all"}"#;
        let s: PlayerStatus = serde_json::from_str(json).unwrap();
        assert_eq!(s.track_id, None);
        assert_eq!(s.repeat, RepeatMode::All);
    }

    #[test]
    fn track_parses_captured_jxa_output() {
        let json = r#"[{"id":"C0714DDF1C1331F5","name":"Take On Me","artist":"a-ha","album":"25 (Deluxe Version)","album_artist":"a-ha","duration_secs":229.013,"track_number":1,"disc_number":1,"year":2010}]"#;
        let t: Vec<Track> = serde_json::from_str(json).unwrap();
        assert_eq!(t[0].name, "Take On Me");
        assert_eq!(t[0].grouping_artist(), "a-ha");
    }

    #[test]
    fn playlist_parses_captured_jxa_output() {
        let json = r#"[{"id":"ABC","name":"Favorite Songs","smart":true,"track_ids":["A","B"]}]"#;
        let p: Vec<Playlist> = serde_json::from_str(json).unwrap();
        assert_eq!(p[0].track_ids.len(), 2);
        assert!(p[0].smart);
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(fmt_duration(0.0), "0:00");
        assert_eq!(fmt_duration(65.4), "1:05");
        assert_eq!(fmt_duration(3725.0), "1:02:05");
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(RepeatMode::Off.next(), RepeatMode::All);
        assert_eq!(RepeatMode::All.next(), RepeatMode::One);
        assert_eq!(RepeatMode::One.next(), RepeatMode::Off);
    }
}
