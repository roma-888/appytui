//! Fixtures shared by the app-level tests.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::App;
use super::reducer::{Action, Effect, reduce};
use crate::config::VizSettings;
use crate::config::theme::Theme;
use crate::music::Command;
use crate::music::Event;
use crate::music::fake::track;
use crate::music::model::{PlayerState, PlayerStatus, Playlist, PlaylistId, TrackId};

pub fn app() -> App {
    let mut app = App::new(
        Theme::builtin("terminal").unwrap(),
        VizSettings::default(),
        false,
    );
    let mut t3 = track("3", "Gamma", "Zed", "Album Z");
    t3.album_artist = "Zed".into();
    reduce(
        &mut app,
        Action::Bridge(Event::Library(vec![
            track("1", "Alpha", "Ann", "Album A"),
            track("2", "Beta", "Ann", "Album A"),
            t3,
        ])),
    );
    reduce(
        &mut app,
        Action::Bridge(Event::Playlists(vec![Playlist {
            id: PlaylistId("P1".into()),
            name: "Mix".into(),
            smart: false,
            track_ids: vec![TrackId("3".into()), TrackId("1".into())],
        }])),
    );
    app
}

/// Sixty tracks named T00..T59 so library order matches id order.
pub fn big_app() -> App {
    let mut app = App::new(
        Theme::builtin("terminal").unwrap(),
        VizSettings::default(),
        false,
    );
    let tracks = (0..60)
        .map(|i| track(&format!("{i}"), &format!("T{i:02}"), "Ann", "Big"))
        .collect();
    reduce(&mut app, Action::Bridge(Event::Library(tracks)));
    app
}

/// The one PlayTracks command among `fx` (other effects, such as the
/// visualizer control or an art request, may accompany it).
pub fn sent_tracks(fx: &[Effect]) -> Vec<TrackId> {
    let mut sent = fx.iter().filter_map(|e| match e {
        Effect::Send(Command::PlayTracks(ids)) => Some(ids.clone()),
        _ => None,
    });
    let first = sent
        .next()
        .unwrap_or_else(|| panic!("expected a PlayTracks, got {fx:?}"));
    assert!(sent.next().is_none(), "more than one PlayTracks in {fx:?}");
    first
}

pub fn id(n: usize) -> TrackId {
    TrackId(n.to_string())
}

pub fn key(c: char) -> Action {
    Action::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

pub fn code(k: KeyCode) -> Action {
    Action::Key(KeyEvent::new(k, KeyModifiers::NONE))
}

pub fn playing_app(position: f64, secs_ago: u64) -> App {
    let mut a = app();
    a.status = PlayerStatus {
        state: PlayerState::Playing,
        track_id: Some(id(1)),
        position_secs: position,
        ..PlayerStatus::default()
    };
    a.status_at = Instant::now() - Duration::from_secs(secs_ago);
    a
}

pub fn poll(a: &mut App, state: PlayerState, position: f64, track: &str) -> Vec<Effect> {
    reduce(
        a,
        Action::Bridge(Event::Status(PlayerStatus {
            state,
            track_id: Some(TrackId(track.into())),
            position_secs: position,
            ..PlayerStatus::default()
        })),
    )
}
