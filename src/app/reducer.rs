//! Pure state transitions. All I/O is expressed as `Effect`s for main.rs to run.

use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::library::Library;
use super::queue::PlayContext;
use super::views::{Drill, Row, Tab};
use super::{App, MESSAGE_TTL, OPTIMISTIC_WINDOW};
use crate::art::{ArtRequest, ArtResult};
use crate::config::Orientation;
use crate::music::model::{PlayerState, TrackId};
use crate::music::{Command, Event};
use crate::viz::{Control, VizEvent};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Key(KeyEvent),
    Tick,
    Bridge(Event),
    Viz(VizEvent),
    Art(ArtResult),
    Resize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Send(Command),
    Viz(Control),
    LookupArt(ArtRequest),
    Quit,
}

/// How many tracks of a long list (Songs, Search) are sent to Music.app at
/// once. Copying a track into the playlist costs about 17 ms (one Apple Event
/// per track, serviced once per display frame), so this bounds the delay
/// before playback starts.
pub const WINDOW: usize = 25;

/// How far a status poll may disagree with the locally running clock before the
/// clock is re-anchored to it. Polls arrive tens of milliseconds late, so
/// re-anchoring on every one made the seconds counter hiccup.
pub const CLOCK_TOLERANCE_SECS: f64 = 0.5;

pub fn reduce(app: &mut App, action: Action) -> Vec<Effect> {
    match action {
        Action::Key(key) => on_key(app, key),
        Action::Tick => {
            if app
                .message
                .as_ref()
                .is_some_and(|(_, at)| at.elapsed() > MESSAGE_TTL)
            {
                app.message = None;
            }
            Vec::new()
        }
        Action::Bridge(ev) => on_bridge(app, ev),
        Action::Viz(VizEvent::Frame(f)) => {
            app.viz_frame = Some(f);
            Vec::new()
        }
        Action::Viz(VizEvent::Fallback(msg)) => {
            app.viz_simulated = true;
            app.notify(msg);
            Vec::new()
        }
        Action::Art(res) => {
            if app.art_key.as_deref() == Some(res.key.as_str()) {
                app.art = res
                    .image
                    .map(|img| (res.key, app.picker.new_resize_protocol(img)));
            }
            Vec::new()
        }
        Action::Resize => Vec::new(),
    }
}

fn on_bridge(app: &mut App, ev: Event) -> Vec<Effect> {
    match ev {
        Event::Library(tracks) => {
            app.library = Some(Library::new(tracks));
            app.invalidate_rows();
        }
        Event::Playlists(p) => {
            app.playlists = p;
            app.playlists_loaded = true;
            app.invalidate_rows();
        }
        Event::Status(s) => {
            let mut effects = Vec::new();
            let was_playing = app.status.state == PlayerState::Playing;
            if let Some(id) = &s.track_id {
                let before = app.context.index;
                app.context.resync(id);
                if app.context.index != before {
                    app.invalidate_rows();
                }
            }
            let mut s = s;
            let optimistic = app
                .optimistic_at
                .is_some_and(|t| t.elapsed() < OPTIMISTIC_WINDOW);
            if optimistic {
                s.volume = app.status.volume;
                s.shuffle = app.status.shuffle;
                s.repeat = app.status.repeat;
            }
            // Keep the local clock (state, position and its timestamp) when the
            // poll predates a local play/pause/seek, or when it merely confirms
            // a running clock within tolerance. Anything else re-anchors.
            let same_track = s.track_id == app.status.track_id;
            let local_playing = app.status.state == PlayerState::Playing;
            let keep_clock = same_track
                && if optimistic {
                    s.state != app.status.state || local_playing
                } else {
                    local_playing
                        && s.state == PlayerState::Playing
                        && (s.position_secs - app.position_now()).abs() < CLOCK_TOLERANCE_SECS
                };
            if keep_clock {
                s.state = app.status.state;
                s.position_secs = app.status.position_secs;
            } else {
                app.status_at = Instant::now();
            }
            app.status = s;
            let playing = app.status.state == PlayerState::Playing;
            if playing != was_playing {
                effects.push(Effect::Viz(Control::Playing(playing)));
            }
            effects.extend(art_for_current_track(app));
            return effects;
        }
        Event::MusicPid(pid) => {
            app.music_pid = Some(pid);
            return vec![Effect::Viz(Control::MusicPid(pid))];
        }
        Event::Error(e) => app.notify(e),
    }
    Vec::new()
}

/// Move the local clock by `delta` seconds now and ask Music.app to follow.
fn seek_by(app: &mut App, delta: f64) -> Vec<Effect> {
    let pos = (app.position_now() + delta).max(0.0);
    let now = Instant::now();
    app.status.position_secs = pos;
    app.status_at = now;
    app.optimistic_at = Some(now);
    vec![Effect::Send(Command::Seek(pos))]
}

/// Request album art when the current track's album differs from the one
/// shown, and clear the art when nothing is playing.
fn art_for_current_track(app: &mut App) -> Option<Effect> {
    match app.current_track().cloned() {
        Some(track) if app.art_enabled => {
            let key = crate::art::cache_key(&track.artist, &track.album);
            if app.art_key.as_deref() == Some(key.as_str()) {
                return None;
            }
            app.art_key = Some(key.clone());
            app.art = None;
            Some(Effect::LookupArt(ArtRequest {
                key,
                artist: track.artist,
                album: track.album,
                name: track.name,
            }))
        }
        Some(_) => None,
        None => {
            app.art = None;
            app.art_key = None;
            None
        }
    }
}

fn on_key(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return vec![Effect::Quit];
    }
    if app.show_help {
        if matches!(
            key.code,
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')
        ) {
            app.show_help = false;
        }
        return Vec::new();
    }
    if app.editing_filter {
        return on_filter_key(app, key);
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let len = app.rows(app.tab).len();
    match key.code {
        KeyCode::Char('q') => {
            app.should_quit = true;
            return vec![Effect::Quit];
        }
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Char(c @ '1'..='6') => {
            let tab = Tab::from_index(c as usize - '1' as usize).unwrap_or(Tab::Songs);
            app.tab = tab;
            if tab == Tab::Search {
                app.editing_filter = true;
            }
        }
        KeyCode::Tab => {
            app.tab = Tab::from_index((app.tab.index() + 1) % Tab::ALL.len()).unwrap_or(Tab::Songs)
        }
        KeyCode::Char('j') | KeyCode::Down => app.view_mut().move_cursor(1, len),
        KeyCode::Char('k') | KeyCode::Up => app.view_mut().move_cursor(-1, len),
        KeyCode::Char('d') if ctrl => app.view_mut().move_cursor(10, len),
        KeyCode::Char('u') if ctrl => app.view_mut().move_cursor(-10, len),
        KeyCode::PageDown => app.view_mut().move_cursor(20, len),
        KeyCode::PageUp => app.view_mut().move_cursor(-20, len),
        KeyCode::Char('g') | KeyCode::Home => app.view_mut().move_cursor(isize::MIN / 2, len),
        KeyCode::Char('G') | KeyCode::End => app.view_mut().move_cursor(isize::MAX / 2, len),
        KeyCode::Enter => return on_enter(app),
        KeyCode::Char('a') => return play_all(app),
        KeyCode::Backspace => {
            let view = app.view_mut();
            if view.drill != Drill::Top {
                view.drill = Drill::Top;
                view.cursor = view.parent_cursor;
                view.filter.clear();
            }
        }
        KeyCode::Char('/') => {
            app.editing_filter = true;
        }
        KeyCode::Esc => {
            app.view_mut().filter.clear();
            app.view_mut().cursor = 0;
        }
        KeyCode::Char(' ') => {
            // Flip the local clock now; the poll after the command confirms it.
            let now = Instant::now();
            let playing = match app.status.state {
                PlayerState::Playing => {
                    app.status.position_secs = app.position_now();
                    app.status.state = PlayerState::Paused;
                    false
                }
                _ => {
                    app.status.state = PlayerState::Playing;
                    true
                }
            };
            app.status_at = now;
            app.optimistic_at = Some(now);
            return vec![
                Effect::Send(Command::PlayPause),
                Effect::Viz(Control::Playing(playing)),
            ];
        }
        KeyCode::Char('n') => return vec![Effect::Send(Command::Next)],
        KeyCode::Char('p') => return vec![Effect::Send(Command::Previous)],
        KeyCode::Right => return seek_by(app, 5.0),
        KeyCode::Left => return seek_by(app, -5.0),
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.optimistic_at = Some(Instant::now());
            app.status.volume = app.status.volume.saturating_add(5).min(100);
            return vec![Effect::Send(Command::SetVolume(app.status.volume))];
        }
        KeyCode::Char('-') => {
            app.optimistic_at = Some(Instant::now());
            app.status.volume = app.status.volume.saturating_sub(5);
            return vec![Effect::Send(Command::SetVolume(app.status.volume))];
        }
        KeyCode::Char('s') => {
            app.optimistic_at = Some(Instant::now());
            app.status.shuffle = !app.status.shuffle;
            return vec![Effect::Send(Command::SetShuffle(app.status.shuffle))];
        }
        KeyCode::Char('r') => {
            app.optimistic_at = Some(Instant::now());
            app.status.repeat = app.status.repeat.next();
            return vec![Effect::Send(Command::SetRepeat(app.status.repeat))];
        }
        KeyCode::Char('v') => {
            app.viz.enabled = !app.viz.enabled;
            return vec![Effect::Viz(Control::Settings(app.viz.clone()))];
        }
        KeyCode::Char('V') => {
            app.viz.orientation = match app.viz.orientation {
                Orientation::Bottom => Orientation::Top,
                Orientation::Top => Orientation::Horizontal,
                Orientation::Horizontal => Orientation::Bottom,
            };
            return vec![Effect::Viz(Control::Settings(app.viz.clone()))];
        }
        KeyCode::Char('w') => {
            app.viz.waveform = !app.viz.waveform;
            return vec![Effect::Viz(Control::Settings(app.viz.clone()))];
        }
        _ => {}
    }
    Vec::new()
}

fn on_filter_key(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    match key.code {
        KeyCode::Esc => {
            app.editing_filter = false;
            app.view_mut().filter.clear();
            app.view_mut().cursor = 0;
        }
        KeyCode::Enter => {
            app.editing_filter = false;
            if app.tab == Tab::Search {
                return on_enter(app);
            }
        }
        KeyCode::Backspace => {
            app.view_mut().filter.pop();
            app.view_mut().cursor = 0;
        }
        KeyCode::Down => {
            let len = app.rows(app.tab).len();
            app.view_mut().move_cursor(1, len);
        }
        KeyCode::Up => {
            let len = app.rows(app.tab).len();
            app.view_mut().move_cursor(-1, len);
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.view_mut().filter.push(c);
            app.view_mut().cursor = 0;
        }
        _ => {}
    }
    Vec::new()
}

fn on_enter(app: &mut App) -> Vec<Effect> {
    let rows = app.rows(app.tab);
    let Some(&row) = rows.get(app.view().cursor) else {
        return Vec::new();
    };
    match row {
        Row::Album(a) => drill(app, Drill::Album(a)),
        Row::Artist(a) => drill(app, Drill::Artist(a)),
        Row::Playlist(p) => drill(app, Drill::Playlist(p)),
        Row::Track(i) => {
            let Some(lib) = app.library.as_ref() else {
                return Vec::new();
            };
            let track_id = lib.tracks[i].id.clone();
            let list = if app.tab == Tab::Queue {
                app.context.track_ids.clone()
            } else {
                track_ids(lib, &rows)
            };
            let index = list.iter().position(|id| *id == track_id).unwrap_or(0);
            play_list(app, list, index)
        }
    }
}

/// `a`: play the list under the cursor from its first track. On an album,
/// artist or playlist row that is the collection itself; anywhere else it is
/// the visible list.
fn play_all(app: &mut App) -> Vec<Effect> {
    let rows = app.rows(app.tab);
    let Some(lib) = app.library.as_ref() else {
        return Vec::new();
    };
    let by_index =
        |idx: &[usize]| -> Vec<TrackId> { idx.iter().map(|&i| lib.tracks[i].id.clone()).collect() };
    let list = match rows.get(app.view().cursor) {
        Some(Row::Album(a)) => by_index(&lib.albums[*a].tracks),
        Some(Row::Artist(a)) => by_index(&lib.artists[*a].tracks),
        Some(Row::Playlist(p)) => app.playlists[*p]
            .track_ids
            .iter()
            .filter(|id| lib.index_of(id).is_some())
            .cloned()
            .collect(),
        _ => track_ids(lib, &rows),
    };
    play_list(app, list, 0)
}

fn track_ids(lib: &Library, rows: &[Row]) -> Vec<TrackId> {
    rows.iter()
        .filter_map(|r| match r {
            Row::Track(j) => Some(lib.tracks[*j].id.clone()),
            _ => None,
        })
        .collect()
}

/// Start `list` at `index` through appytui's playlist, which always plays from
/// its first track. Albums, artists, playlists and the queue are sent whole,
/// rotated so the chosen track is first and the earlier ones follow at the end.
/// Songs and Search are long, so only a window is sent: the next `WINDOW`
/// tracks, or with shuffle on the chosen track plus a random sample of the
/// rest so shuffle still spans the whole list.
fn play_list(app: &mut App, list: Vec<TrackId>, index: usize) -> Vec<Effect> {
    if list.is_empty() {
        return Vec::new();
    }
    let index = index.min(list.len() - 1);
    let ids: Vec<TrackId> = if matches!(app.tab, Tab::Songs | Tab::Search) {
        if app.status.shuffle {
            let mut others: Vec<usize> = (0..list.len()).filter(|&i| i != index).collect();
            let take = others.len().min(WINDOW - 1);
            // Partial Fisher-Yates: the first `take` entries become the sample.
            for i in 0..take {
                let j = i + (app.random() as usize) % (others.len() - i);
                others.swap(i, j);
            }
            std::iter::once(index)
                .chain(others[..take].iter().copied())
                .map(|i| list[i].clone())
                .collect()
        } else {
            list[index..(index + WINDOW).min(list.len())].to_vec()
        }
    } else {
        let mut ids = list;
        ids.rotate_left(index);
        ids
    };
    app.context = PlayContext::new(ids.clone(), 0);
    app.invalidate_rows();
    // Filling the playlist takes up to a second; show the chosen track now and
    // let the next status poll correct anything.
    let first = ids[0].clone();
    let name = app
        .library
        .as_ref()
        .and_then(|l| l.get(&first))
        .map(|t| t.name.clone())
        .unwrap_or_default();
    app.status.track_id = Some(first);
    app.status.state = PlayerState::Playing;
    app.status.position_secs = 0.0;
    app.status_at = Instant::now();
    app.notify(format!("Starting {name}…"));
    let mut effects = vec![Effect::Send(Command::PlayTracks(ids))];
    effects.extend(art_for_current_track(app));
    effects
}

fn drill(app: &mut App, into: Drill) -> Vec<Effect> {
    let view = app.view_mut();
    view.parent_cursor = view.cursor;
    view.drill = into;
    view.cursor = 0;
    view.filter.clear();
    Vec::new()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::config::VizSettings;
    use crate::config::theme::Theme;
    use crate::music::fake::track;
    use crate::music::model::{PlayerState, PlayerStatus, Playlist, PlaylistId, RepeatMode};

    fn app() -> App {
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
    fn big_app() -> App {
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

    fn sent_tracks(fx: &[Effect]) -> Vec<TrackId> {
        match fx {
            [Effect::Send(Command::PlayTracks(ids))] => ids.clone(),
            other => panic!("expected one PlayTracks, got {other:?}"),
        }
    }

    fn id(n: usize) -> TrackId {
        TrackId(n.to_string())
    }

    fn key(c: char) -> Action {
        Action::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }
    fn code(k: KeyCode) -> Action {
        Action::Key(KeyEvent::new(k, KeyModifiers::NONE))
    }

    #[test]
    fn q_quits() {
        let mut a = app();
        assert_eq!(reduce(&mut a, key('q')), vec![Effect::Quit]);
        assert!(a.should_quit);
    }

    #[test]
    fn number_keys_switch_tabs_and_jk_moves() {
        let mut a = app();
        reduce(&mut a, key('2'));
        assert_eq!(a.tab, Tab::Albums);
        reduce(&mut a, key('1'));
        reduce(&mut a, key('j'));
        reduce(&mut a, key('j'));
        assert_eq!(a.view().cursor, 2);
        reduce(&mut a, key('k'));
        assert_eq!(a.view().cursor, 1);
        reduce(&mut a, key('G'));
        assert_eq!(a.view().cursor, 2);
        reduce(&mut a, key('g'));
        assert_eq!(a.view().cursor, 0);
    }

    #[test]
    fn enter_on_song_plays_onward_from_that_row() {
        let mut a = app();
        reduce(&mut a, key('j'));
        let fx = reduce(&mut a, code(KeyCode::Enter));
        assert_eq!(sent_tracks(&fx), vec![id(2), id(3)]);
        assert_eq!(a.context.track_ids, vec![id(2), id(3)]);
        assert_eq!(a.context.index, 0);
    }

    #[test]
    fn enter_shows_the_chosen_track_as_playing_before_music_confirms() {
        let mut a = app();
        reduce(&mut a, key('j'));
        reduce(&mut a, code(KeyCode::Enter));
        assert_eq!(a.status.track_id, Some(id(2)));
        assert_eq!(a.status.state, PlayerState::Playing);
        assert_eq!(a.current_track().map(|t| t.name.as_str()), Some("Beta"));
        assert!(a.position_now() < 1.0);
        let msg = a
            .message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(
            msg.contains("Beta"),
            "status message should name the track, got {msg:?}"
        );
    }

    #[test]
    fn enter_on_song_sends_at_most_the_window() {
        let mut a = big_app();
        for _ in 0..5 {
            reduce(&mut a, key('j'));
        }
        let fx = reduce(&mut a, code(KeyCode::Enter));
        let want: Vec<TrackId> = (5..30).map(id).collect();
        assert_eq!(sent_tracks(&fx), want);
    }

    #[test]
    fn enter_on_song_with_shuffle_samples_the_list_after_the_chosen_track() {
        let mut a = big_app();
        a.status.shuffle = true;
        for _ in 0..5 {
            reduce(&mut a, key('j'));
        }
        let sent = sent_tracks(&reduce(&mut a, code(KeyCode::Enter)));
        assert_eq!(sent.len(), 25);
        assert_eq!(sent[0], id(5));
        let mut uniq: Vec<&str> = sent.iter().map(|t| t.0.as_str()).collect();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 25, "no repeats");
        let sequential: Vec<TrackId> = (5..30).map(id).collect();
        assert_ne!(
            sent, sequential,
            "shuffle should not send the sequential window"
        );
        assert!(sent.iter().all(|t| t.0.parse::<usize>().unwrap() < 60));
    }

    #[test]
    fn enter_on_search_result_plays_onward_through_the_results() {
        let mut a = app();
        reduce(&mut a, key('5'));
        for c in "ann".chars() {
            reduce(&mut a, key(c));
        }
        let fx = reduce(&mut a, code(KeyCode::Enter));
        let sent = sent_tracks(&fx);
        assert_eq!(sent.len(), 2);
        assert!(sent.contains(&id(1)) && sent.contains(&id(2)));
    }

    #[test]
    fn a_plays_the_selected_album_from_the_top() {
        let mut a = app();
        reduce(&mut a, key('2'));
        assert_eq!(sent_tracks(&reduce(&mut a, key('a'))), vec![id(1), id(2)]);
        assert_eq!(a.context.track_ids, vec![id(1), id(2)]);
        assert_eq!(a.context.index, 0);
        assert_eq!(a.view().drill, Drill::Top);
    }

    #[test]
    fn a_plays_the_selected_artist_and_playlist() {
        let mut a = app();
        reduce(&mut a, key('3'));
        assert_eq!(sent_tracks(&reduce(&mut a, key('a'))), vec![id(1), id(2)]);
        reduce(&mut a, key('4'));
        assert_eq!(sent_tracks(&reduce(&mut a, key('a'))), vec![id(3), id(1)]);
    }

    #[test]
    fn a_inside_a_drill_plays_that_list_from_the_top() {
        let mut a = app();
        reduce(&mut a, key('2'));
        reduce(&mut a, code(KeyCode::Enter));
        reduce(&mut a, key('j'));
        assert_eq!(sent_tracks(&reduce(&mut a, key('a'))), vec![id(1), id(2)]);
    }

    #[test]
    fn a_on_songs_plays_the_list_from_the_top() {
        let mut a = app();
        reduce(&mut a, key('G'));
        assert_eq!(
            sent_tracks(&reduce(&mut a, key('a'))),
            vec![id(1), id(2), id(3)]
        );
        let mut big = big_app();
        assert_eq!(sent_tracks(&reduce(&mut big, key('a'))).len(), 25);
    }

    #[test]
    fn a_with_nothing_listed_does_nothing() {
        let mut a = app();
        reduce(&mut a, key('5'));
        assert_eq!(reduce(&mut a, key('a')), Vec::new());
    }

    #[test]
    fn enter_on_album_drills_down_and_backspace_returns() {
        let mut a = app();
        reduce(&mut a, key('2'));
        reduce(&mut a, key('j'));
        reduce(&mut a, code(KeyCode::Enter));
        assert_eq!(a.view().drill, Drill::Album(1));
        assert_eq!(a.rows(Tab::Albums), vec![Row::Track(2)]);
        reduce(&mut a, code(KeyCode::Backspace));
        assert_eq!(a.view().drill, Drill::Top);
        assert_eq!(a.view().cursor, 1);
    }

    #[test]
    fn playing_from_playlist_plays_its_tracks_through_own_playlist() {
        let mut a = app();
        reduce(&mut a, key('4'));
        reduce(&mut a, code(KeyCode::Enter));
        let fx = reduce(&mut a, code(KeyCode::Enter));
        let ids = vec![TrackId("3".into()), TrackId("1".into())];
        assert_eq!(fx, vec![Effect::Send(Command::PlayTracks(ids.clone()))]);
        assert_eq!(a.context.track_ids, ids);
        reduce(&mut a, key('6'));
        assert_eq!(a.rows(Tab::Queue), vec![Row::Track(0)]);
    }

    #[test]
    fn playing_from_album_starts_the_album_at_the_chosen_track() {
        let mut a = app();
        reduce(&mut a, key('2'));
        reduce(&mut a, code(KeyCode::Enter));
        reduce(&mut a, key('j'));
        let fx = reduce(&mut a, code(KeyCode::Enter));
        // Album A is [1, 2]; picking 2 rotates it so 2 plays first, then 1.
        let ids = vec![TrackId("2".into()), TrackId("1".into())];
        assert_eq!(fx, vec![Effect::Send(Command::PlayTracks(ids.clone()))]);
        assert_eq!(a.context.track_ids, ids);
        assert_eq!(a.context.index, 0);
    }

    #[test]
    fn playing_from_queue_restarts_own_playlist_at_that_track() {
        let mut a = app();
        reduce(&mut a, key('3'));
        reduce(&mut a, code(KeyCode::Enter));
        let fx = reduce(&mut a, code(KeyCode::Enter));
        let ids = vec![TrackId("1".into()), TrackId("2".into())];
        assert_eq!(fx, vec![Effect::Send(Command::PlayTracks(ids))]);
        reduce(&mut a, key('6'));
        assert_eq!(a.rows(Tab::Queue), vec![Row::Track(1)]);
        let fx = reduce(&mut a, code(KeyCode::Enter));
        let rotated = vec![TrackId("2".into()), TrackId("1".into())];
        assert_eq!(fx, vec![Effect::Send(Command::PlayTracks(rotated.clone()))]);
        assert_eq!(a.context.track_ids, rotated);
        assert_eq!(a.context.index, 0);
    }

    #[test]
    fn playing_from_queue_after_songs_restarts_the_window_at_that_track() {
        let mut a = app();
        reduce(&mut a, code(KeyCode::Enter));
        reduce(&mut a, key('6'));
        assert_eq!(a.rows(Tab::Queue), vec![Row::Track(1), Row::Track(2)]);
        let fx = reduce(&mut a, code(KeyCode::Enter));
        assert_eq!(sent_tracks(&fx), vec![id(2), id(3), id(1)]);
        assert_eq!(a.context.index, 0);
    }

    #[test]
    fn status_event_resyncs_context_and_records_time() {
        let mut a = app();
        reduce(&mut a, code(KeyCode::Enter));
        let before = a.status_at;
        reduce(
            &mut a,
            Action::Bridge(Event::Status(PlayerStatus {
                state: PlayerState::Playing,
                track_id: Some(TrackId("3".into())),
                track: None,
                position_secs: 10.0,
                volume: 50,
                shuffle: false,
                repeat: RepeatMode::Off,
            })),
        );
        assert_eq!(a.context.index, 2);
        assert!(a.status_at >= before);
        assert!(a.position_now() >= 10.0);
    }

    #[test]
    fn transport_keys_send_commands_optimistically() {
        let mut a = app();
        assert_eq!(
            reduce(&mut a, key(' ')),
            vec![
                Effect::Send(Command::PlayPause),
                Effect::Viz(Control::Playing(true)),
            ]
        );
        assert_eq!(reduce(&mut a, key('n')), vec![Effect::Send(Command::Next)]);
        assert_eq!(
            reduce(&mut a, key('p')),
            vec![Effect::Send(Command::Previous)]
        );
        assert_eq!(
            reduce(&mut a, key('-')),
            vec![Effect::Send(Command::SetVolume(95))]
        );
        assert_eq!(
            reduce(&mut a, key('-')),
            vec![Effect::Send(Command::SetVolume(90))]
        );
        assert_eq!(
            reduce(&mut a, key('s')),
            vec![Effect::Send(Command::SetShuffle(true))]
        );
        assert_eq!(
            reduce(&mut a, key('r')),
            vec![Effect::Send(Command::SetRepeat(RepeatMode::All))]
        );
        // Space above started the local clock, so the seek target is 5 s plus
        // the few microseconds elapsed since.
        let fx = reduce(&mut a, code(KeyCode::Right));
        assert!(matches!(fx[..], [Effect::Send(Command::Seek(p))] if (p - 5.0).abs() < 0.1));
        let fx = reduce(&mut a, code(KeyCode::Left));
        assert!(matches!(fx[..], [Effect::Send(Command::Seek(p))] if p.abs() < 0.1));
    }

    #[test]
    fn slash_edits_filter_and_esc_clears() {
        let mut a = app();
        reduce(&mut a, key('/'));
        assert!(a.editing_filter);
        reduce(&mut a, key('g'));
        reduce(&mut a, key('a'));
        assert_eq!(a.view().filter, "ga");
        assert_eq!(a.rows(Tab::Songs), vec![Row::Track(2)]);
        reduce(&mut a, code(KeyCode::Enter));
        assert!(!a.editing_filter);
        assert_eq!(a.view().filter, "ga");
        reduce(&mut a, code(KeyCode::Esc));
        assert_eq!(a.view().filter, "");
    }

    #[test]
    fn search_tab_types_directly() {
        let mut a = app();
        reduce(&mut a, key('5'));
        assert!(a.editing_filter);
        reduce(&mut a, key('b'));
        reduce(&mut a, key('e'));
        reduce(&mut a, key('t'));
        assert_eq!(a.rows(Tab::Search), vec![Row::Track(1)]);
    }

    #[test]
    fn errors_become_messages_and_expire() {
        let mut a = app();
        reduce(&mut a, Action::Bridge(Event::Error("boom".into())));
        assert_eq!(a.message.as_ref().map(|m| m.0.as_str()), Some("boom"));
        a.message = Some(("old".into(), Instant::now() - MESSAGE_TTL * 2));
        reduce(&mut a, Action::Tick);
        assert!(a.message.is_none());
    }

    #[test]
    fn viz_keys_update_settings_and_emit_control() {
        let mut a = app();
        let fx = reduce(&mut a, key('V'));
        assert_eq!(a.viz.orientation, Orientation::Bottom);
        assert_eq!(fx, vec![Effect::Viz(Control::Settings(a.viz.clone()))]);
        reduce(&mut a, key('w'));
        assert!(a.viz.waveform);
        reduce(&mut a, key('v'));
        assert!(!a.viz.enabled);
    }

    #[test]
    fn viz_frame_and_fallback_are_stored() {
        let mut a = app();
        reduce(
            &mut a,
            Action::Viz(VizEvent::Frame(crate::viz::Frame {
                left: vec![0.5],
                right: vec![],
                waveform: vec![],
            })),
        );
        assert!(a.viz_frame.is_some());
        reduce(&mut a, Action::Viz(VizEvent::Fallback("simulated".into())));
        assert!(a.viz_simulated);
        assert!(a.message.is_some());
    }

    #[test]
    fn play_state_change_emits_playing_control() {
        let mut a = app();
        let fx = reduce(
            &mut a,
            Action::Bridge(Event::Status(PlayerStatus {
                state: PlayerState::Playing,
                ..PlayerStatus::default()
            })),
        );
        assert_eq!(fx, vec![Effect::Viz(Control::Playing(true))]);
        let fx = reduce(
            &mut a,
            Action::Bridge(Event::Status(PlayerStatus {
                state: PlayerState::Playing,
                position_secs: 3.0,
                ..PlayerStatus::default()
            })),
        );
        assert!(fx.is_empty());
    }

    #[test]
    fn enter_requests_art_for_the_chosen_track_immediately() {
        let mut a = app();
        a.art_enabled = true;
        reduce(&mut a, key('j'));
        let fx = reduce(&mut a, code(KeyCode::Enter));
        assert!(fx.contains(&Effect::LookupArt(ArtRequest {
            key: crate::art::cache_key("Ann", "Album A"),
            artist: "Ann".into(),
            album: "Album A".into(),
            name: "Beta".into(),
        })));
        // Music.app confirming the same track must not request it again.
        let fx = reduce(
            &mut a,
            Action::Bridge(Event::Status(PlayerStatus {
                state: PlayerState::Playing,
                track_id: Some(id(2)),
                ..PlayerStatus::default()
            })),
        );
        assert!(!fx.iter().any(|e| matches!(e, Effect::LookupArt(_))));
    }

    fn playing_app(position: f64, secs_ago: u64) -> App {
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

    fn poll(a: &mut App, state: PlayerState, position: f64) -> Vec<Effect> {
        reduce(
            a,
            Action::Bridge(Event::Status(PlayerStatus {
                state,
                track_id: Some(id(1)),
                position_secs: position,
                ..PlayerStatus::default()
            })),
        )
    }

    #[test]
    fn space_pauses_the_clock_immediately() {
        let mut a = playing_app(10.0, 2);
        let fx = reduce(&mut a, key(' '));
        assert_eq!(
            fx,
            vec![
                Effect::Send(Command::PlayPause),
                Effect::Viz(Control::Playing(false)),
            ]
        );
        assert_eq!(a.status.state, PlayerState::Paused);
        assert!(
            (a.position_now() - 12.0).abs() < 0.2,
            "{}",
            a.position_now()
        );
        assert!(a.optimistic_at.is_some());
    }

    #[test]
    fn space_resumes_the_clock_immediately() {
        let mut a = playing_app(10.0, 2);
        a.status.state = PlayerState::Paused;
        reduce(&mut a, key(' '));
        assert_eq!(a.status.state, PlayerState::Playing);
        assert!(
            (a.position_now() - 10.0).abs() < 0.2,
            "{}",
            a.position_now()
        );
    }

    #[test]
    fn stale_poll_right_after_space_does_not_flip_state_or_clock() {
        let mut a = playing_app(10.0, 2);
        reduce(&mut a, key(' '));
        poll(&mut a, PlayerState::Playing, 12.3);
        assert_eq!(a.status.state, PlayerState::Paused);
        assert!(
            (a.position_now() - 12.0).abs() < 0.2,
            "{}",
            a.position_now()
        );
        // A poll that agrees with the toggle is taken as the new anchor.
        poll(&mut a, PlayerState::Paused, 12.1);
        assert_eq!(a.status.position_secs, 12.1);
    }

    #[test]
    fn poll_close_to_the_local_clock_keeps_the_anchor() {
        let mut a = playing_app(10.0, 1);
        let anchor = a.status_at;
        poll(&mut a, PlayerState::Playing, 10.8);
        assert_eq!(a.status_at, anchor);
        assert_eq!(a.status.position_secs, 10.0);
        assert!(
            (a.position_now() - 11.0).abs() < 0.2,
            "{}",
            a.position_now()
        );
    }

    #[test]
    fn poll_far_from_the_local_clock_re_anchors() {
        let mut a = playing_app(10.0, 1);
        let anchor = a.status_at;
        poll(&mut a, PlayerState::Playing, 30.0);
        assert!(a.status_at > anchor);
        assert_eq!(a.status.position_secs, 30.0);
    }

    #[test]
    fn poll_while_paused_always_takes_the_position() {
        let mut a = playing_app(10.0, 1);
        a.status.state = PlayerState::Paused;
        poll(&mut a, PlayerState::Paused, 10.4);
        assert_eq!(a.status.position_secs, 10.4);
    }

    #[test]
    fn seek_moves_the_clock_immediately() {
        let mut a = playing_app(10.0, 2);
        let fx = reduce(&mut a, code(KeyCode::Right));
        assert!(matches!(fx[..], [Effect::Send(Command::Seek(p))] if (p - 17.0).abs() < 0.2));
        assert!(
            (a.position_now() - 17.0).abs() < 0.2,
            "{}",
            a.position_now()
        );
        reduce(&mut a, code(KeyCode::Left));
        assert!(
            (a.position_now() - 12.0).abs() < 0.2,
            "{}",
            a.position_now()
        );
        // A stale poll from before the seek must not drag the clock back.
        poll(&mut a, PlayerState::Playing, 12.4);
        assert!(
            (a.position_now() - 12.0).abs() < 0.2,
            "{}",
            a.position_now()
        );
    }

    #[test]
    fn track_change_requests_art_once_and_result_is_kept_if_current() {
        let mut a = app();
        a.art_enabled = true;
        let status = PlayerStatus {
            state: PlayerState::Playing,
            track_id: Some(TrackId("1".into())),
            ..PlayerStatus::default()
        };
        let fx = reduce(&mut a, Action::Bridge(Event::Status(status.clone())));
        let key = crate::art::cache_key("Ann", "Album A");
        assert!(fx.contains(&Effect::LookupArt(ArtRequest {
            key: key.clone(),
            artist: "Ann".into(),
            album: "Album A".into(),
            name: "Alpha".into(),
        })));
        let fx = reduce(
            &mut a,
            Action::Bridge(Event::Status(PlayerStatus {
                position_secs: 5.0,
                ..status
            })),
        );
        assert!(!fx.iter().any(|e| matches!(e, Effect::LookupArt(_))));
        reduce(
            &mut a,
            Action::Art(ArtResult {
                key: key.clone(),
                image: Some(image::DynamicImage::new_rgb8(1, 1)),
            }),
        );
        assert!(a.art.is_some());
        reduce(
            &mut a,
            Action::Art(ArtResult {
                key: "stale".into(),
                image: Some(image::DynamicImage::new_rgb8(1, 1)),
            }),
        );
        assert_eq!(a.art.as_ref().map(|(k, _)| k.as_str()), Some(key.as_str()));
    }

    #[test]
    fn streamed_track_outside_library_still_shows_as_current() {
        let mut a = app();
        let snapshot = track("stream1", "Pretty Pure", "whenyoung", "Single");
        reduce(
            &mut a,
            Action::Bridge(Event::Status(PlayerStatus {
                state: PlayerState::Playing,
                track_id: Some(TrackId("stream1".into())),
                track: Some(snapshot),
                ..PlayerStatus::default()
            })),
        );
        assert_eq!(
            a.current_track().map(|t| t.name.as_str()),
            Some("Pretty Pure")
        );
    }

    #[test]
    fn stale_status_poll_cannot_undo_optimistic_toggle() {
        let mut a = app();
        reduce(&mut a, key('s'));
        assert!(a.status.shuffle);
        // A poll that started before the toggle reached Music.app still says off.
        reduce(
            &mut a,
            Action::Bridge(Event::Status(PlayerStatus {
                shuffle: false,
                ..PlayerStatus::default()
            })),
        );
        assert!(
            a.status.shuffle,
            "stale poll clobbered the optimistic value"
        );
        // Second press therefore turns it off, as the user expects.
        assert_eq!(
            reduce(&mut a, key('s')),
            vec![Effect::Send(Command::SetShuffle(false))]
        );
        // Once the window has passed, polls are authoritative again.
        a.optimistic_at = Some(Instant::now() - OPTIMISTIC_WINDOW * 2);
        reduce(
            &mut a,
            Action::Bridge(Event::Status(PlayerStatus {
                shuffle: true,
                ..PlayerStatus::default()
            })),
        );
        assert!(a.status.shuffle);
    }

    #[test]
    fn ctrl_c_quits_from_anywhere() {
        let mut a = app();
        reduce(&mut a, key('?'));
        let fx = reduce(
            &mut a,
            Action::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        assert_eq!(fx, vec![Effect::Quit]);
        let mut b = app();
        reduce(&mut b, key('/'));
        let fx = reduce(
            &mut b,
            Action::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        assert_eq!(fx, vec![Effect::Quit]);
    }

    #[test]
    fn rows_are_cached_until_something_relevant_changes() {
        let mut a = app();
        reduce(&mut a, key('/'));
        reduce(&mut a, key('g'));
        reduce(&mut a, key('a'));
        let n0 = a.rank_calls();
        let r1 = a.rows(Tab::Songs);
        let r2 = a.rows(Tab::Songs);
        assert_eq!(r1, r2);
        assert_eq!(a.rank_calls(), n0 + 1, "second call should hit the cache");
        reduce(
            &mut a,
            Action::Bridge(Event::Library(vec![track("9", "Gala", "Zed", "Z")])),
        );
        a.rows(Tab::Songs);
        assert_eq!(
            a.rank_calls(),
            n0 + 2,
            "library change must invalidate the cache"
        );
        reduce(&mut a, key('m'));
        a.rows(Tab::Songs);
        assert_eq!(
            a.rank_calls(),
            n0 + 3,
            "filter change must invalidate the cache"
        );
    }

    #[test]
    fn playlists_loaded_flag_flips_on_event() {
        let mut a = App::new(
            Theme::builtin("terminal").unwrap(),
            VizSettings::default(),
            false,
        );
        assert!(!a.playlists_loaded);
        reduce(&mut a, Action::Bridge(Event::Playlists(vec![])));
        assert!(a.playlists_loaded);
    }

    #[test]
    fn help_toggles_and_swallows_keys() {
        let mut a = app();
        reduce(&mut a, key('?'));
        assert!(a.show_help);
        assert_eq!(reduce(&mut a, key('j')), vec![]);
        reduce(&mut a, key('?'));
        assert!(!a.show_help);
    }
}
