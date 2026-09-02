//! Runs a `MusicBridge` on its own thread so osascript latency never blocks the UI.

use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use super::model::{PlayerState, PlayerStatus};
use super::{Command, Event, MusicBridge};

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
            poll_status(&mut *bridge, &events, &mut last, &mut failures);
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
                        poll_status(&mut *bridge, &events, &mut last, &mut failures);
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        poll_status(&mut *bridge, &events, &mut last, &mut failures)
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
) {
    match bridge.status() {
        Ok(s) => {
            if *failures > 0 {
                *failures = 0;
                let _ = events.send(Event::Error("Music.app reconnected".into()));
            }
            if last.as_ref() != Some(&s) {
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
    let result = match cmd {
        Command::LoadLibrary => bridge.load_library().map(|t| {
            let _ = events.send(Event::Library(t));
        }),
        Command::LoadPlaylists => bridge.load_playlists().map(|p| {
            let _ = events.send(Event::Playlists(p));
        }),
        Command::PlayTrack(track) => bridge.play_track(&track),
        Command::PlayTracks(tracks) => bridge.play_tracks(&tracks),
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
        let _ = events.send(Event::Error(format!("{e:#}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::fake::{FakeBridge, track};
    use crate::music::model::{PlayerState, TrackId};

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

        cmd_tx
            .send(Command::PlayTrack(TrackId("A".into())))
            .unwrap();
        let ev = ev_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(ev, Event::Status(s) if s.state == PlayerState::Playing));

        assert!(ev_rx.recv_timeout(Duration::from_millis(100)).is_err());

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
