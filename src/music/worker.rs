//! Runs a `MusicBridge` on its own thread so osascript latency never blocks the UI.

use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use super::model::PlayerStatus;
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
            match bridge.music_pid() {
                Ok(pid) => {
                    let _ = events.send(Event::MusicPid(pid));
                }
                Err(e) => {
                    let _ = events.send(Event::Error(format!("music pid: {e:#}")));
                }
            }
            poll_status(&mut *bridge, &events, &mut last);
            loop {
                match commands.recv_timeout(poll_interval) {
                    Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                    Ok(cmd) => {
                        handle(&mut *bridge, &events, cmd);
                        poll_status(&mut *bridge, &events, &mut last);
                    }
                    Err(RecvTimeoutError::Timeout) => poll_status(&mut *bridge, &events, &mut last),
                }
            }
        })
        .expect("spawn music bridge thread")
}

fn poll_status(bridge: &mut dyn MusicBridge, events: &Sender<Event>, last: &mut Option<PlayerStatus>) {
    match bridge.status() {
        Ok(s) => {
            if last.as_ref() != Some(&s) {
                *last = Some(s.clone());
                let _ = events.send(Event::Status(s));
            }
        }
        Err(e) => {
            let _ = events.send(Event::Error(format!("status: {e:#}")));
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
        Command::PlayTrack { track, context } => bridge.play_track(&track, context.as_ref()),
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

        assert_eq!(ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(), Event::MusicPid(4242));
        assert!(matches!(ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(), Event::Status(_)));

        cmd_tx.send(Command::LoadLibrary).unwrap();
        assert!(matches!(ev_rx.recv_timeout(Duration::from_secs(1)).unwrap(), Event::Library(t) if t.len() == 1));

        cmd_tx.send(Command::PlayTrack { track: TrackId("A".into()), context: None }).unwrap();
        let ev = ev_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(ev, Event::Status(s) if s.state == PlayerState::Playing));

        assert!(ev_rx.recv_timeout(Duration::from_millis(100)).is_err());

        cmd_tx.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
    }
}
