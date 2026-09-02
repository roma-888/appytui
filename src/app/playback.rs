//! Starting playback: which tracks go to Music.app, in what order.

use std::time::Instant;

use super::App;
use super::library::Library;
use super::queue::PlayContext;
use super::reducer::{Effect, art_for_current_track};
use super::views::{Row, Tab};
use crate::music::Command;
use crate::music::model::{PlayerState, TrackId};

/// How many tracks of a long list (Songs, Search) are sent to Music.app at
/// once. Copying a track into the playlist costs about 17 ms (one Apple Event
/// per track, serviced once per display frame), so this bounds the delay
/// before playback starts.
pub const WINDOW: usize = 25;

/// `a`: play the list under the cursor from its first track. On an album,
/// artist or playlist row that is the collection itself; anywhere else it is
/// the visible list.
pub fn play_all(app: &mut App) -> Vec<Effect> {
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

pub fn track_ids(lib: &Library, rows: &[Row]) -> Vec<TrackId> {
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
pub fn play_list(app: &mut App, list: Vec<TrackId>, index: usize) -> Vec<Effect> {
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

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;

    use super::super::reducer::{Action, Effect, reduce};
    use super::super::testing::*;
    use super::super::views::{Drill, Row, Tab};
    use crate::art::ArtRequest;
    use crate::music::Command;
    use crate::music::Event;
    use crate::music::model::{PlayerState, PlayerStatus, TrackId};

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
}
