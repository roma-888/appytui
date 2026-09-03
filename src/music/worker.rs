//! Runs a `MusicBridge` on its own thread so osascript latency never blocks the UI.

use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use super::model::{PlayerState, PlayerStatus};
use super::{Command, Event, FailedCommand, MusicBridge};

pub fn spawn(
    mut bridge: Box<dyn MusicBridge>,
    commands: Receiver<Command>,
    events: Sender<Event>,
    poll_interval: Duration,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("music-bridge".into())
        .spawn(move || {
            let mut last: Option<PlayerStatus> = None;
            let mut failures: u32 = 0;
            match bridge.music_pid() {
                Ok(pid) => {
                    let _ = events.send(Event::MusicPid(pid));
                }
                Err(e) => {
                    let _ = events.send(Event::Error(format!("music pid: {e:#}")));
                }
            }
            poll_status(&mut *bridge, &events, &mut last, &mut failures, false);
            loop {
                // Poll less often while nothing is playing: each poll spawns osascript.
                let playing = last
                    .as_ref()
                    .is_some_and(|s| s.state == PlayerState::Playing);
                let interval = if playing {
                    poll_interval
                } else {
                    poll_interval * 3
                };
                match commands.recv_timeout(interval) {
                    Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                    Ok(cmd) => {
                        handle(&mut *bridge, &events, cmd);
                        // Always report after a command: the app applied it
                        // optimistically and needs the truth even if unchanged.
                        poll_status(&mut *bridge, &events, &mut last, &mut failures, true);
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        poll_status(&mut *bridge, &events, &mut last, &mut failures, false)
                    }
                }
            }
        })
        .expect("spawn music bridge thread")
}

/// Poll once. Music.app quitting mid-session must not spam the status bar: report
/// once, try to relaunch it, and report again only every tenth failure.
fn poll_status(
    bridge: &mut dyn MusicBridge,
    events: &Sender<Event>,
    last: &mut Option<PlayerStatus>,
    failures: &mut u32,
    force: bool,
) {
    match bridge.status() {
        Ok(s) => {
            if *failures > 0 {
                *failures = 0;
                let _ = events.send(Event::Error("Music.app reconnected".into()));
            }
            if force || last.as_ref() != Some(&s) {
                *last = Some(s.clone());
                let _ = events.send(Event::Status(s));
            }
        }
        Err(e) => {
            *failures += 1;
            if *failures == 1 {
                let _ = events.send(Event::Error(
                    "Music.app is not responding, relaunching it".into(),
                ));
                let _ = bridge.ensure_running();
            } else if failures.is_multiple_of(10) {
                let _ = events.send(Event::Error(format!(
                    "Music.app still not responding: {e:#}"
                )));
            }
        }
    }
}

fn handle(bridge: &mut dyn MusicBridge, events: &Sender<Event>, cmd: Command) {
    let (kind, name) = match &cmd {
        Command::PlayTracks(_) => (FailedCommand::Play, "play"),
        Command::PlayPrepared => (FailedCommand::Play, "queue switch"),
        Command::PrepareTracks(_) => (FailedCommand::Prepare, "prepare queue"),
        Command::LoadLibrary => (FailedCommand::Other, "load library"),
        Command::LoadPlaylists => (FailedCommand::Other, "load playlists"),
        _ => (FailedCommand::Other, "command"),
    };
    let result = match cmd {
        Command::LoadLibrary => bridge.load_library().map(|t| {
            let _ = events.send(Event::Library(t));
        }),
        Command::LoadPlaylists => bridge.load_playlists().map(|p| {
            let _ = events.send(Event::Playlists(p));
        }),
        Command::PlayTracks(tracks) => bridge.play_tracks(&tracks),
        Command::PrepareTracks(tracks) => bridge.prepare_tracks(&tracks),
        Command::PlayPrepared => bridge.play_prepared(),
        Command::PlayPause => bridge.play_pause(),
        Command::Next => bridge.next(),
        Command::Previous => bridge.previous(),
        Command::Seek(s) => bridge.seek(s),
        Command::SetVolume(v) => bridge.set_volume(v),
        Command::SetShuffle(on) => bridge.set_shuffle(on),
        Command::SetRepeat(m) => bridge.set_repeat(m),
        Command::Shutdown => Ok(()),
    };
    if let Err(e) = result {
        let _ = events.send(Event::Failed(kind, format!("{name}: {e:#}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::fake::{FakeBridge, track};
    use crate::music::model::{PlayerState, TrackId};
    use crossbeam_channel::unbounded;

    #[test]
    fn worker_answers_commands_and_polls_status_changes() {
        let bridge = FakeBridge::with_tracks(vec![track("A", "S", "Ar", "Al")]);
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let (ev_tx, ev_rx) = crossbeam_channel::unbounded();
        let handle = spawn(Box::new(bridge), cmd_rx, ev_tx, Duration::from_millis(20));

        assert_eq!(
            ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::MusicPid(4242)
        );
        assert!(matches!(
            ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::Status(_)
        ));

        cmd_tx.send(Command::LoadLibrary).unwrap();
        assert!(
            matches!(ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(), Event::Library(t) if t.len() == 1)
        );
        // Every command is followed by a status report, changed or not.
        assert!(matches!(
            ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::Status(_)
        ));

        cmd_tx
            .send(Command::PlayTracks(vec![TrackId("A".into())]))
            .unwrap();
        let ev = ev_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(ev, Event::Status(s) if s.state == PlayerState::Playing));

        assert!(ev_rx.recv_timeout(Duration::from_millis(100)).is_err());

        cmd_tx.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn every_command_is_followed_by_a_status_even_when_nothing_changed() {
        let bridge = FakeBridge::with_tracks(vec![track("A", "Song", "Artist", "Album")]);
        let (cmd_tx, cmd_rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let handle = spawn(Box::new(bridge), cmd_rx, ev_tx, Duration::from_secs(5));
        assert!(matches!(
            ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::MusicPid(_)
        ));
        assert!(matches!(
            ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::Status(_)
        ));
        // The fake's `next` leaves the status untouched.
        cmd_tx.send(Command::Next).unwrap();
        assert!(matches!(
            ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::Status(_)
        ));
        cmd_tx.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn failed_commands_report_what_failed() {
        let mut bridge = FakeBridge::with_tracks(vec![]);
        bridge.fail_prepare = true;
        let (cmd_tx, cmd_rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let handle = spawn(Box::new(bridge), cmd_rx, ev_tx, Duration::from_secs(5));
        cmd_tx
            .send(Command::PrepareTracks(vec![TrackId("A".into())]))
            .unwrap();
        let failed = loop {
            match ev_rx.recv_timeout(Duration::from_secs(1)).unwrap() {
                Event::Failed(what, msg) => break (what, msg),
                _ => continue,
            }
        };
        assert_eq!(failed.0, FailedCommand::Prepare);
        assert!(failed.1.contains("prepare"), "{}", failed.1);
        cmd_tx.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn music_quitting_reports_once_and_relaunches() {
        let bridge = FakeBridge {
            fail_status: true,
            ..FakeBridge::default()
        };
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let (ev_tx, ev_rx) = crossbeam_channel::unbounded();
        let handle = spawn(Box::new(bridge), cmd_rx, ev_tx, Duration::from_millis(10));
        assert_eq!(
            ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Event::MusicPid(4242)
        );
        let mut events = Vec::new();
        while let Ok(ev) = ev_rx.recv_timeout(Duration::from_millis(200)) {
            events.push(ev);
        }
        let errors: Vec<&Event> = events
            .iter()
            .filter(|e| matches!(e, Event::Error(_)))
            .collect();
        assert_eq!(
            errors.len(),
            2,
            "expected one failure notice and one reconnect notice: {events:?}"
        );
        assert!(matches!(errors[0], Event::Error(m) if m.contains("relaunching")));
        assert!(matches!(errors[1], Event::Error(m) if m.contains("reconnected")));
        assert!(events.iter().any(|e| matches!(e, Event::Status(_))));
        cmd_tx.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
    }
}
