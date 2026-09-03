//! Starting playback: which tracks go to Music.app, in what order.

use std::time::Instant;

use super::App;
use super::library::Library;
use super::queue::PlayContext;
use super::reducer::{Effect, art_for_current_track};
use super::views::{Drill, Row, Tab};
use crate::music::Command;
use crate::music::model::{PlayerState, TrackId};

/// How many tracks of a long list (Songs, Search) are sent to Music.app at
/// once. Copying a track into the playlist costs about 17 ms (one Apple Event
/// per track, serviced once per display frame), so this bounds the delay
/// before playback starts.
pub const WINDOW: usize = 25;

/// How long before the end of the current track the prepared queue is started.
/// Late enough to lose almost nothing, early enough for the call to land.
pub const SWITCH_LEAD_SECS: f64 = 0.35;

/// `a`: play the list under the cursor from its first track. On an album,
/// artist or playlist row that is the collection itself; anywhere else it is
/// the visible list.
pub fn play_all(app: &mut App) -> Vec<Effect> {
    let rows = app.rows(app.tab);
    let Some(lib) = app.library.as_ref() else {
        return Vec::new();
    };
    let list = collection_tracks(app, rows.get(app.view().cursor))
        .unwrap_or_else(|| track_ids(lib, &rows));
    play_list(app, list, 0)
}

/// The tracks of an album, artist or playlist row; `None` for anything else.
fn collection_tracks(app: &App, row: Option<&Row>) -> Option<Vec<TrackId>> {
    let lib = app.library.as_ref()?;
    let by_index =
        |idx: &[usize]| -> Vec<TrackId> { idx.iter().map(|&i| lib.tracks[i].id.clone()).collect() };
    match row? {
        Row::Album(a) => Some(by_index(&lib.albums[*a].tracks)),
        Row::Artist(a) => Some(by_index(&lib.artists[*a].tracks)),
        Row::Playlist(p) => Some(
            app.playlists[*p]
                .track_ids
                .iter()
                .filter(|id| lib.index_of(id).is_some())
                .cloned()
                .collect(),
        ),
        Row::Track(_) => None,
    }
}

/// `e` / `E`: add the tracks under the cursor (a collection's, or the one
/// track) to the end of the queue, or right after the current track. Starts
/// playback instead when nothing is playing.
pub fn enqueue(app: &mut App, next: bool) -> Vec<Effect> {
    let rows = app.rows(app.tab);
    let row = rows.get(app.view().cursor);
    let tracks = match (collection_tracks(app, row), row, app.library.as_ref()) {
        (Some(tracks), _, _) => tracks,
        (None, Some(Row::Track(i)), Some(lib)) => vec![lib.tracks[*i].id.clone()],
        _ => return Vec::new(),
    };
    if tracks.is_empty() {
        return Vec::new();
    }
    if app.context.track_ids.is_empty() || app.status.track_id.is_none() {
        return play_list(app, tracks, 0);
    }
    let what = describe(app, &tracks);
    // Music.app picks the order under shuffle, so "next" degrades to "later".
    let next = next && !app.status.shuffle;
    let len = app.context.track_ids.len();
    let at = if next {
        (app.context.index + 1).min(len)
    } else {
        len
    };
    app.context.track_ids.splice(at..at, tracks);
    app.notify(if next {
        format!("Playing next: {what}")
    } else {
        format!("Added to queue: {what}")
    });
    requeue(app)
}

/// `d` on the Queue tab: drop the row under the cursor from the queue.
pub fn dequeue(app: &mut App) -> Vec<Effect> {
    if app.tab != Tab::Queue {
        return Vec::new();
    }
    let cursor = app.view().cursor;
    let Some(lib) = app.library.as_ref() else {
        return Vec::new();
    };
    // Queue rows are the upcoming tracks that are in the library, in order.
    let Some(pos) = app
        .context
        .track_ids
        .iter()
        .enumerate()
        .skip(app.context.index + 1)
        .filter(|(_, id)| lib.index_of(id).is_some())
        .nth(cursor)
        .map(|(i, _)| i)
    else {
        return Vec::new();
    };
    let removed = app.context.track_ids.remove(pos);
    let what = describe(app, std::slice::from_ref(&removed));
    app.notify(format!("Removed from queue: {what}"));
    requeue(app)
}

fn describe(app: &App, tracks: &[TrackId]) -> String {
    match tracks {
        [one] => app
            .library
            .as_ref()
            .and_then(|l| l.get(one))
            .map(|t| t.name.clone())
            .unwrap_or_default(),
        many => format!("{} tracks", many.len()),
    }
}

/// Copy the queue after the current track into the idle playlist, so the
/// switch at the end of the track is a single call. The current track goes
/// last, matching how a fresh play wraps the earlier tracks.
fn requeue(app: &mut App) -> Vec<Effect> {
    app.invalidate_rows();
    let len = app.context.track_ids.len();
    if len < 2 {
        app.pending_requeue = false;
        return Vec::new();
    }
    let mut prepared = app.context.track_ids.clone();
    prepared.rotate_left((app.context.index + 1) % len);
    app.pending_requeue = true;
    vec![Effect::Send(Command::PrepareTracks(prepared))]
}

/// Start the prepared queue now: at the track boundary, on `n`, or when
/// Music.app wandered off the queue. Shows the next track as playing at once.
pub fn switch_now(app: &mut App) -> Vec<Effect> {
    app.pending_requeue = false;
    let len = app.context.track_ids.len();
    if len == 0 {
        return Vec::new();
    }
    app.switching_from = app.status.track_id.take();
    app.context.index = (app.context.index + 1) % len;
    let now = Instant::now();
    app.status.track_id = Some(app.context.track_ids[app.context.index].clone());
    app.status.state = PlayerState::Playing;
    app.status.position_secs = 0.0;
    app.status_at = now;
    app.optimistic_at = Some(now);
    app.invalidate_rows();
    let mut effects = vec![Effect::Send(Command::PlayPrepared)];
    effects.extend(art_for_current_track(app));
    effects
}

/// Tick: start the prepared queue just before the current track ends.
pub fn maybe_switch(app: &mut App) -> Vec<Effect> {
    if !app.pending_requeue || app.status.state != PlayerState::Playing {
        return Vec::new();
    }
    let Some(duration) = app
        .current_track()
        .map(|t| t.duration_secs)
        .filter(|d| *d > 0.0)
    else {
        return Vec::new();
    };
    if duration - app.position_now() <= SWITCH_LEAD_SECS {
        switch_now(app)
    } else {
        Vec::new()
    }
}

/// A status poll changed the track (or stopped) while an edited queue was
/// waiting. If Music.app moved along our queue, re-prepare from the new
/// track; if it left the queue or stopped, start the prepared one now.
pub fn on_track_changed_while_pending(app: &mut App) -> Vec<Effect> {
    if !app.pending_requeue {
        return Vec::new();
    }
    let on_queue = app
        .status
        .track_id
        .as_ref()
        .is_some_and(|id| app.context.track_ids.contains(id));
    if app.status.state == PlayerState::Stopped || !on_queue {
        switch_now(app)
    } else {
        requeue(app)
    }
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
    // Songs and Search results are long lists; an opened album or artist
    // inside Search is not.
    let long_list = matches!(app.tab, Tab::Songs | Tab::Search) && app.view().drill == Drill::Top;
    let ids: Vec<TrackId> = if long_list {
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
    app.pending_requeue = false;
    app.switching_from = None;
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

    use std::time::Instant;

    use super::super::App;
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
        // Results mix artists, albums and tracks; move to the first track row.
        let rows = a.rows(Tab::Search);
        let first_track = rows
            .iter()
            .position(|r| matches!(r, Row::Track(_)))
            .unwrap();
        for _ in 0..first_track {
            reduce(&mut a, code(KeyCode::Down));
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

    /// Album A ([1, 2]) playing from track 1, then move to the Songs tab with
    /// the cursor on track 3.
    fn queued_app() -> App {
        let mut a = app();
        reduce(&mut a, key('2'));
        reduce(&mut a, code(KeyCode::Enter));
        reduce(&mut a, code(KeyCode::Enter));
        assert_eq!(a.context.track_ids, vec![id(1), id(2)]);
        reduce(&mut a, key('1'));
        reduce(&mut a, key('G'));
        a
    }

    fn prepared(fx: &[Effect]) -> Vec<TrackId> {
        match fx {
            [Effect::Send(Command::PrepareTracks(ids))] => ids.clone(),
            other => panic!("expected one PrepareTracks, got {other:?}"),
        }
    }

    #[test]
    fn e_appends_to_the_queue_and_prepares_the_rest() {
        let mut a = queued_app();
        let fx = reduce(&mut a, key('e'));
        assert_eq!(a.context.track_ids, vec![id(1), id(2), id(3)]);
        assert_eq!(a.context.index, 0);
        assert_eq!(prepared(&fx), vec![id(2), id(3), id(1)]);
        assert!(a.pending_requeue);
        let msg = a
            .message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(msg.contains("Gamma"), "{msg:?}");
        reduce(&mut a, key('6'));
        assert_eq!(a.rows(Tab::Queue), vec![Row::Track(1), Row::Track(2)]);
    }

    #[test]
    fn shift_e_inserts_after_the_current_track() {
        let mut a = queued_app();
        let fx = reduce(&mut a, key('E'));
        assert_eq!(a.context.track_ids, vec![id(1), id(3), id(2)]);
        assert_eq!(prepared(&fx), vec![id(3), id(2), id(1)]);
    }

    #[test]
    fn shift_e_with_shuffle_on_appends() {
        let mut a = queued_app();
        a.status.shuffle = true;
        reduce(&mut a, key('E'));
        assert_eq!(a.context.track_ids, vec![id(1), id(2), id(3)]);
    }

    #[test]
    fn e_with_nothing_playing_starts_that_track() {
        let mut a = app();
        reduce(&mut a, key('j'));
        let fx = reduce(&mut a, key('e'));
        assert_eq!(sent_tracks(&fx), vec![id(2)]);
        assert!(!a.pending_requeue);
    }

    #[test]
    fn e_on_an_album_row_enqueues_the_whole_album() {
        let mut a = app();
        reduce(&mut a, key('4'));
        reduce(&mut a, code(KeyCode::Enter));
        reduce(&mut a, code(KeyCode::Enter));
        assert_eq!(a.context.track_ids, vec![id(3), id(1)]);
        reduce(&mut a, key('2'));
        let fx = reduce(&mut a, key('e'));
        assert_eq!(a.context.track_ids, vec![id(3), id(1), id(1), id(2)]);
        assert_eq!(prepared(&fx), vec![id(1), id(1), id(2), id(3)]);
    }

    #[test]
    fn d_on_the_queue_tab_removes_that_row() {
        let mut a = queued_app();
        reduce(&mut a, key('e'));
        reduce(&mut a, key('6'));
        reduce(&mut a, key('j'));
        let fx = reduce(&mut a, key('d'));
        assert_eq!(a.context.track_ids, vec![id(1), id(2)]);
        assert_eq!(prepared(&fx), vec![id(2), id(1)]);
        assert_eq!(a.rows(Tab::Queue), vec![Row::Track(1)]);
    }

    #[test]
    fn d_outside_the_queue_tab_does_nothing() {
        let mut a = queued_app();
        assert_eq!(reduce(&mut a, key('d')), Vec::new());
        assert_eq!(a.context.track_ids, vec![id(1), id(2)]);
    }

    fn pending_near_end(remaining: f64) -> App {
        let mut a = queued_app();
        reduce(&mut a, key('e'));
        a.status.state = PlayerState::Playing;
        a.status.track_id = Some(id(1));
        a.status.position_secs = 200.0 - remaining;
        a.status_at = Instant::now();
        a
    }

    #[test]
    fn tick_switches_to_the_prepared_list_at_the_end_of_the_track() {
        let mut a = pending_near_end(0.2);
        let fx = reduce(&mut a, Action::Tick);
        assert!(fx.contains(&Effect::Send(Command::PlayPrepared)), "{fx:?}");
        assert!(!a.pending_requeue);
        assert_eq!(a.context.index, 1);
        assert_eq!(a.status.track_id, Some(id(2)));
        assert!(a.position_now() < 0.5);
        // Only once.
        assert!(!reduce(&mut a, Action::Tick).contains(&Effect::Send(Command::PlayPrepared)));
    }

    #[test]
    fn tick_does_not_switch_early_or_while_paused() {
        let mut a = pending_near_end(5.0);
        assert!(!reduce(&mut a, Action::Tick).contains(&Effect::Send(Command::PlayPrepared)));
        assert!(a.pending_requeue);
        let mut p = pending_near_end(0.2);
        p.status.state = PlayerState::Paused;
        assert!(!reduce(&mut p, Action::Tick).contains(&Effect::Send(Command::PlayPrepared)));
        assert!(p.pending_requeue);
    }

    #[test]
    fn n_while_pending_plays_the_prepared_list() {
        let mut a = pending_near_end(100.0);
        let fx = reduce(&mut a, key('n'));
        assert!(fx.contains(&Effect::Send(Command::PlayPrepared)), "{fx:?}");
        assert!(!fx.contains(&Effect::Send(Command::Next)));
        assert_eq!(a.context.index, 1);
    }

    #[test]
    fn track_change_while_pending_reprepares_from_the_new_track() {
        let mut a = pending_near_end(100.0);
        let fx = poll(&mut a, PlayerState::Playing, 0.5, "2");
        assert_eq!(prepared(&fx), vec![id(3), id(1), id(2)]);
        assert!(a.pending_requeue);
        assert_eq!(a.context.index, 1);
    }

    #[test]
    fn unexpected_track_while_pending_switches_immediately() {
        let mut a = pending_near_end(100.0);
        let fx = poll(&mut a, PlayerState::Playing, 0.5, "9");
        assert!(fx.contains(&Effect::Send(Command::PlayPrepared)), "{fx:?}");
        assert!(!a.pending_requeue);
        let mut s = pending_near_end(100.0);
        let fx = poll(&mut s, PlayerState::Stopped, 0.0, "1");
        assert!(fx.contains(&Effect::Send(Command::PlayPrepared)), "{fx:?}");
    }

    #[test]
    fn enter_clears_a_pending_requeue() {
        let mut a = pending_near_end(100.0);
        reduce(&mut a, code(KeyCode::Enter));
        assert!(!a.pending_requeue);
    }

    #[test]
    fn stale_poll_of_the_previous_track_after_a_switch_is_ignored() {
        let mut a = pending_near_end(0.2);
        reduce(&mut a, Action::Tick);
        assert_eq!(a.status.track_id, Some(id(2)));
        poll(&mut a, PlayerState::Playing, 199.9, "1");
        assert_eq!(a.status.track_id, Some(id(2)));
        assert_eq!(a.context.index, 1);
        poll(&mut a, PlayerState::Playing, 0.3, "2");
        assert_eq!(a.status.track_id, Some(id(2)));
    }
}
