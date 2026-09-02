//! Pure state transitions. All I/O is expressed as `Effect`s for main.rs to run.

use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::library::Library;
use super::queue::PlayContext;
use super::views::{Drill, Row, Tab};
use super::{App, MESSAGE_TTL};
use crate::music::model::TrackId;
use crate::music::{Command, Event};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Key(KeyEvent),
    Tick,
    Bridge(Event),
    Resize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Send(Command),
    Quit,
}

pub fn reduce(app: &mut App, action: Action) -> Vec<Effect> {
    match action {
        Action::Key(key) => on_key(app, key),
        Action::Tick => {
            if app.message.as_ref().is_some_and(|(_, at)| at.elapsed() > MESSAGE_TTL) {
                app.message = None;
            }
            Vec::new()
        }
        Action::Bridge(ev) => on_bridge(app, ev),
        Action::Resize => Vec::new(),
    }
}

fn on_bridge(app: &mut App, ev: Event) -> Vec<Effect> {
    match ev {
        Event::Library(tracks) => {
            app.library = Some(Library::new(tracks));
        }
        Event::Playlists(p) => app.playlists = p,
        Event::Status(s) => {
            if let Some(id) = &s.track_id {
                app.context.resync(id);
            }
            app.status = s;
            app.status_at = Instant::now();
        }
        Event::MusicPid(pid) => app.music_pid = Some(pid),
        Event::Error(e) => app.notify(e),
    }
    Vec::new()
}

fn on_key(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    if app.show_help {
        if matches!(key.code, KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')) {
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
        KeyCode::Tab => app.tab = Tab::from_index((app.tab.index() + 1) % Tab::ALL.len()).unwrap_or(Tab::Songs),
        KeyCode::Char('j') | KeyCode::Down => app.view_mut().move_cursor(1, len),
        KeyCode::Char('k') | KeyCode::Up => app.view_mut().move_cursor(-1, len),
        KeyCode::Char('d') if ctrl => app.view_mut().move_cursor(10, len),
        KeyCode::Char('u') if ctrl => app.view_mut().move_cursor(-10, len),
        KeyCode::PageDown => app.view_mut().move_cursor(20, len),
        KeyCode::PageUp => app.view_mut().move_cursor(-20, len),
        KeyCode::Char('g') | KeyCode::Home => app.view_mut().move_cursor(isize::MIN / 2, len),
        KeyCode::Char('G') | KeyCode::End => app.view_mut().move_cursor(isize::MAX / 2, len),
        KeyCode::Enter => return on_enter(app),
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
        KeyCode::Char(' ') => return vec![Effect::Send(Command::PlayPause)],
        KeyCode::Char('n') => return vec![Effect::Send(Command::Next)],
        KeyCode::Char('p') => return vec![Effect::Send(Command::Previous)],
        KeyCode::Right => {
            let pos = (app.position_now() + 5.0).max(0.0);
            return vec![Effect::Send(Command::Seek(pos))];
        }
        KeyCode::Left => {
            let pos = (app.position_now() - 5.0).max(0.0);
            return vec![Effect::Send(Command::Seek(pos))];
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.status.volume = app.status.volume.saturating_add(5).min(100);
            return vec![Effect::Send(Command::SetVolume(app.status.volume))];
        }
        KeyCode::Char('-') => {
            app.status.volume = app.status.volume.saturating_sub(5);
            return vec![Effect::Send(Command::SetVolume(app.status.volume))];
        }
        KeyCode::Char('s') => {
            app.status.shuffle = !app.status.shuffle;
            return vec![Effect::Send(Command::SetShuffle(app.status.shuffle))];
        }
        KeyCode::Char('r') => {
            app.status.repeat = app.status.repeat.next();
            return vec![Effect::Send(Command::SetRepeat(app.status.repeat))];
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
    let Some(&row) = rows.get(app.view().cursor) else { return Vec::new() };
    match row {
        Row::Album(a) => drill(app, Drill::Album(a)),
        Row::Artist(a) => drill(app, Drill::Artist(a)),
        Row::Playlist(p) => drill(app, Drill::Playlist(p)),
        Row::Track(i) => {
            let Some(lib) = app.library.as_ref() else { return Vec::new() };
            let track_id = lib.tracks[i].id.clone();
            let playlist = match (app.tab, app.view().drill) {
                (Tab::Playlists, Drill::Playlist(p)) => Some(app.playlists[p].id.clone()),
                (Tab::Queue, _) => app.context.playlist.clone(),
                _ => None,
            };
            if app.tab == Tab::Queue {
                app.context.resync(&track_id);
            } else {
                let ids: Vec<TrackId> = rows
                    .iter()
                    .filter_map(|r| match r {
                        Row::Track(j) => Some(lib.tracks[*j].id.clone()),
                        _ => None,
                    })
                    .collect();
                let index = ids.iter().position(|id| *id == track_id).unwrap_or(0);
                app.context = PlayContext::new(ids, index, playlist.clone());
            }
            vec![Effect::Send(Command::PlayTrack { track: track_id, context: playlist })]
        }
    }
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
    use super::*;
    use crate::config::VizSettings;
    use crate::config::theme::Theme;
    use crate::music::fake::track;
    use crate::music::model::{PlayerState, PlayerStatus, Playlist, PlaylistId, RepeatMode};

    fn app() -> App {
        let mut app = App::new(Theme::builtin("terminal").unwrap(), VizSettings::default(), false);
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
    fn enter_on_song_plays_with_list_context() {
        let mut a = app();
        reduce(&mut a, key('j'));
        let fx = reduce(&mut a, code(KeyCode::Enter));
        assert_eq!(fx, vec![Effect::Send(Command::PlayTrack { track: TrackId("2".into()), context: None })]);
        assert_eq!(a.context.index, 1);
        assert_eq!(a.context.track_ids.len(), 3);
        assert_eq!(a.context.playlist, None);
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
    fn playing_from_playlist_passes_playlist_context() {
        let mut a = app();
        reduce(&mut a, key('4'));
        reduce(&mut a, code(KeyCode::Enter));
        reduce(&mut a, code(KeyCode::Enter));
        assert_eq!(a.context.playlist, Some(PlaylistId("P1".into())));
        assert_eq!(a.context.track_ids, vec![TrackId("3".into()), TrackId("1".into())]);
        reduce(&mut a, key('6'));
        assert_eq!(a.rows(Tab::Queue), vec![Row::Track(0)]);
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
        assert_eq!(reduce(&mut a, key(' ')), vec![Effect::Send(Command::PlayPause)]);
        assert_eq!(reduce(&mut a, key('n')), vec![Effect::Send(Command::Next)]);
        assert_eq!(reduce(&mut a, key('p')), vec![Effect::Send(Command::Previous)]);
        assert_eq!(reduce(&mut a, key('-')), vec![Effect::Send(Command::SetVolume(95))]);
        assert_eq!(reduce(&mut a, key('-')), vec![Effect::Send(Command::SetVolume(90))]);
        assert_eq!(reduce(&mut a, key('s')), vec![Effect::Send(Command::SetShuffle(true))]);
        assert_eq!(reduce(&mut a, key('r')), vec![Effect::Send(Command::SetRepeat(RepeatMode::All))]);
        assert_eq!(reduce(&mut a, code(KeyCode::Right)), vec![Effect::Send(Command::Seek(5.0))]);
        assert_eq!(reduce(&mut a, code(KeyCode::Left)), vec![Effect::Send(Command::Seek(0.0))]);
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
    fn help_toggles_and_swallows_keys() {
        let mut a = app();
        reduce(&mut a, key('?'));
        assert!(a.show_help);
        assert_eq!(reduce(&mut a, key('j')), vec![]);
        reduce(&mut a, key('?'));
        assert!(!a.show_help);
    }
}
