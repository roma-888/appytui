//! Real bridge: JavaScript for Automation scripts evaluated by one long-lived
//! `osascript` process. Spawning osascript costs about 100 ms per call, so the
//! bridge starts it once with a small server script that reads JSON lines from
//! stdin (`{id, script, args}`), evaluates each script with `Music`, `lib` and
//! `ARGS` in scope, and writes one JSON line back. Arguments always travel as
//! JSON, never by string interpolation into the script.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command as Proc, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossbeam_channel::{Receiver, RecvTimeoutError, unbounded};
use serde::Deserialize;
use serde_json::{Value, json};

use super::MusicBridge;
use super::model::{PlayerStatus, Playlist, RepeatMode, Track, TrackId};

pub const TIMEOUT: Duration = Duration::from_secs(5);

// The server loop. `Music` and `lib` are re-resolved per request so a
// relaunched Music.app is picked up. `eval` sees them because they are
// top-level `var`s. `availableData` blocks until input arrives; the Rust side
// kills the process on shutdown or timeout, so end-of-input handling is best
// effort.
const SERVER: &str = r#"
ObjC.import("stdlib");
var stdin = $.NSFileHandle.fileHandleWithStandardInput;
var stdout = $.NSFileHandle.fileHandleWithStandardOutput;
var Music = null, lib = null, ARGS = {};
var buf = "";
function send(obj) {
  var s = JSON.stringify(obj) + "\n";
  stdout.writeData($.NSString.alloc.initWithUTF8String(s).dataUsingEncoding($.NSUTF8StringEncoding));
}
// With shuffle on, playing a playlist starts at a random track. Music.app's
// own behaviour is the chosen track first, then the rest shuffled: turning
// shuffle off around `play` and back on gives exactly that.
function playPlaylist(pl) {
  var shuffled = Music.shuffleEnabled();
  if (shuffled) { Music.shuffleEnabled = false; }
  pl.play();
  if (shuffled) { Music.shuffleEnabled = true; }
}
function handle(line) {
  var req;
  try { req = JSON.parse(line); } catch (e) { send({ id: null, ok: false, error: "bad request" }); return; }
  try {
    Music = Application("Music");
    lib = Music.libraryPlaylists[0];
    ARGS = req.args || {};
    var result = eval(req.script);
    send({ id: req.id, ok: true, result: result === undefined ? null : result });
  } catch (e) {
    send({ id: req.id, ok: false, error: String(e) });
  }
}
while (true) {
  var data = stdin.availableData;
  if (!ObjC.unwrap(data.length)) break;
  buf += ObjC.unwrap($.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding));
  var nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    var line = buf.slice(0, nl); buf = buf.slice(nl + 1);
    if (line.trim()) handle(line);
  }
}
"#;

const LIBRARY: &str = r#"
const tr = lib.tracks;
const ids = tr.persistentID(), names = tr.name(), artists = tr.artist(), albums = tr.album(),
      aa = tr.albumArtist(), durs = tr.duration(), tn = tr.trackNumber(), dn = tr.discNumber(), yr = tr.year();
const out = ids.map((id, i) => ({
  id, name: names[i] || "", artist: artists[i] || "", album: albums[i] || "", album_artist: aa[i] || "",
  duration_secs: durs[i] || 0, track_number: tn[i] || 0, disc_number: dn[i] || 0, year: yr[i] || 0 }));
JSON.stringify(out);
"#;

const PLAYLISTS: &str = r#"
const out = [];
for (const p of Music.userPlaylists()) {
  let cls = ""; try { cls = p.class(); } catch (e) {}
  if (cls === "folderPlaylist") continue;
  if (p.specialKind() !== "none") continue;
  if (ARGS.skip.includes(p.name())) continue;
  out.push({ id: p.persistentID(), name: p.name(), smart: p.smart(), track_ids: p.tracks.persistentID() });
}
JSON.stringify(out);
"#;

// Every property read is one Apple Event (~17 ms), so the nine-field track
// snapshot is only taken when the track id differs from ARGS.known; the bridge
// fills it back in from its cache otherwise.
const STATUS: &str = r#"
const st = Music.playerState();
let track_id = null, track = null;
if (st !== "stopped") {
  try {
    const t = Music.currentTrack;
    track_id = t.persistentID();
    if (track_id !== ARGS.known) {
      // Snapshot so streamed tracks that are not in the library still show up.
      track = { id: track_id, name: t.name() || "", artist: t.artist() || "", album: t.album() || "",
        album_artist: t.albumArtist() || "", duration_secs: t.duration() || 0,
        track_number: t.trackNumber() || 0, disc_number: t.discNumber() || 0, year: t.year() || 0 };
    }
  } catch (e) {}
}
// Position last: it is the value the clock anchors to, so read it as close
// to the reply as possible.
JSON.stringify({ state: st, track_id, track, volume: Music.soundVolume(),
  shuffle: Music.shuffleEnabled(), repeat: Music.songRepeat(),
  position_secs: Music.playerPosition() || 0 });
"#;

/// The two playlists appytui owns. Playback alternates between them: while
/// one plays, the other is filled with the next queue so the switch at a track
/// boundary is a single call. Both are hidden from the Playlists tab.
pub const OWN_PLAYLISTS: [&str; 2] = ["appytui", "appytui 2"];

// Playing a track object gives Music.app a one-track context followed by
// Autoplay, even for a track of a user playlist. Playing a playlist object is
// the only way to make it continue (and shuffle) within a list, and a playlist
// always starts from its first track, so the caller puts the chosen track first.
// `Music.delete(pl.tracks)` empties the playlist without touching the library.
// Every Apple Event costs ~17 ms, so the loop sends exactly one per track: the
// `whose` specifier is built locally and a missing track makes `duplicate`
// throw, which is cheaper than asking `exists()` first.
const FILL_TRACKS: &str = r#"
let pl = Music.userPlaylists.whose({ name: ARGS.name })[0];
if (!pl.exists()) {
  pl = Music.make({ new: "playlist", withProperties: { name: ARGS.name } });
}
Music.delete(pl.tracks);
for (const id of ARGS.tracks) {
  try { Music.duplicate(lib.tracks.whose({ persistentID: id })[0], { to: pl }); } catch (e) {}
}
if (ARGS.play) { playPlaylist(pl); }
"ok";
"#;

const PLAY_PLAYLIST: &str = r#"
playPlaylist(Music.userPlaylists.whose({ name: ARGS.name })[0]);
"ok";
"#;

const PLAY_PAUSE: &str = "Music.playpause(); \"ok\";";
const NEXT: &str = "Music.nextTrack(); \"ok\";";
const PREVIOUS: &str = "Music.backTrack(); \"ok\";";
const SEEK: &str = "Music.playerPosition = ARGS.seconds; \"ok\";";
const SET_VOLUME: &str = "Music.soundVolume = ARGS.volume; \"ok\";";
const SET_SHUFFLE: &str = "Music.shuffleEnabled = ARGS.on; \"ok\";";
const SET_REPEAT: &str = "Music.songRepeat = ARGS.mode; \"ok\";";
// `launch` starts Music.app in the background; `activate` would steal focus from the terminal.
const LAUNCH: &str = "if (!Music.running()) { Music.launch(); } \"ok\";";

#[derive(Debug, Deserialize)]
struct Reply {
    id: Option<u64>,
    ok: bool,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: String,
}

/// One long-lived osascript evaluating scripts sent as JSON lines.
struct Server {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<Reply>,
    next_id: u64,
    /// Set when a call timed out or the process died; the bridge then replaces it.
    broken: bool,
}

impl Server {
    fn spawn() -> Result<Server> {
        let mut child = Proc::new("osascript")
            .args(["-l", "JavaScript", "-e", SERVER])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawning osascript")?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = unbounded();
        std::thread::Builder::new()
            .name("osascript-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(reply) = serde_json::from_str::<Reply>(&line)
                        && tx.send(reply).is_err()
                    {
                        break;
                    }
                }
            })
            .expect("spawn osascript reader thread");
        Ok(Server {
            child,
            stdin,
            replies: rx,
            next_id: 1,
            broken: false,
        })
    }

    fn call(&mut self, script: &str, args: &Value, timeout: Duration) -> Result<String> {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({ "id": id, "script": script, "args": args }).to_string();
        if writeln!(self.stdin, "{line}")
            .and_then(|_| self.stdin.flush())
            .is_err()
        {
            self.broken = true;
            bail!("osascript exited");
        }
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.replies.recv_timeout(remaining) {
                Ok(reply) if reply.id == Some(id) => {
                    if reply.ok {
                        return Ok(match reply.result {
                            Value::String(s) => s,
                            Value::Null => String::new(),
                            other => other.to_string(),
                        });
                    }
                    bail!("osascript failed: {}", reply.error);
                }
                // A reply to an earlier call that timed out; not possible once
                // the server is replaced, but harmless to skip.
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => {
                    self.broken = true;
                    bail!(
                        "osascript timed out after {:.1}s (is Music.app showing a dialog?)",
                        timeout.as_secs_f64()
                    );
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.broken = true;
                    bail!("osascript exited");
                }
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct JxaBridge {
    timeout: Duration,
    server: Option<Server>,
    /// Snapshot of the last track seen, so the status poll can skip re-reading it.
    last_track: Option<Track>,
    /// Which of `OWN_PLAYLISTS` was played last; the other one is idle.
    slot: usize,
}

/// Remember a fresh track snapshot, or restore the cached one when the poll
/// skipped it because the track id was unchanged.
fn fill_track(status: &mut PlayerStatus, cache: &mut Option<Track>) {
    match (&status.track, &status.track_id) {
        (Some(track), _) => *cache = Some(track.clone()),
        (None, Some(id)) => {
            if let Some(cached) = cache.as_ref()
                && cached.id == *id
            {
                status.track = Some(cached.clone());
            }
        }
        (None, None) => {}
    }
}

impl JxaBridge {
    pub fn new() -> Self {
        Self {
            timeout: TIMEOUT,
            server: None,
            last_track: None,
            slot: 0,
        }
    }

    /// Evaluate `script` with `args` bound to `ARGS`, starting or replacing the
    /// osascript process as needed. The result is the script's final
    /// expression as a string.
    fn eval(&mut self, script: &str, args: Value, timeout: Duration) -> Result<String> {
        if self.server.as_ref().is_some_and(|s| s.broken) {
            self.server = None;
        }
        let server = match self.server.as_mut() {
            Some(s) => s,
            None => self.server.insert(Server::spawn()?),
        };
        let result = server.call(script, &args, timeout);
        if server.broken {
            self.server = None;
        }
        result
    }

    fn run(&mut self, script: &str, args: Value) -> Result<String> {
        self.eval(script, args, self.timeout)
    }

    /// Fill the idle playlist with `tracks`, and play it if `play`.
    fn fill(&mut self, tracks: &[TrackId], play: bool) -> Result<()> {
        let ids: Vec<&str> = tracks.iter().map(|t| t.0.as_str()).collect();
        let name = OWN_PLAYLISTS[1 - self.slot];
        let args = json!({ "name": name, "tracks": ids, "play": play });
        // Each track copied into the playlist is a round trip inside Music.app
        // (about 17 ms), so a long artist list takes a few seconds.
        self.eval(FILL_TRACKS, args, Duration::from_secs(60))
            .map(|_| ())
    }

    #[cfg(test)]
    fn server_pid(&self) -> Option<u32> {
        self.server.as_ref().map(|s| s.child.id())
    }

    /// Launch Music.app if needed. Called once at startup.
    pub fn ensure_running(&mut self) -> Result<()> {
        self.run(LAUNCH, json!({})).map(|_| ())
    }
}

impl Default for JxaBridge {
    fn default() -> Self {
        Self::new()
    }
}

pub fn parse_library(json: &str) -> Result<Vec<Track>> {
    serde_json::from_str(json).context("parsing library JSON")
}

pub fn parse_playlists(json: &str) -> Result<Vec<Playlist>> {
    serde_json::from_str(json).context("parsing playlists JSON")
}

pub fn parse_status(json: &str) -> Result<PlayerStatus> {
    serde_json::from_str(json).context("parsing status JSON")
}

impl MusicBridge for JxaBridge {
    fn ensure_running(&mut self) -> Result<()> {
        JxaBridge::ensure_running(self)
    }
    fn load_library(&mut self) -> Result<Vec<Track>> {
        parse_library(&self.run(LIBRARY, json!({}))?)
    }
    fn load_playlists(&mut self) -> Result<Vec<Playlist>> {
        // Bulk dump of every playlist takes a few seconds on large libraries.
        parse_playlists(&self.eval(
            PLAYLISTS,
            json!({ "skip": OWN_PLAYLISTS }),
            Duration::from_secs(60),
        )?)
    }
    fn status(&mut self) -> Result<PlayerStatus> {
        let known = self.last_track.as_ref().map(|t| t.id.0.clone());
        let mut status = parse_status(&self.run(STATUS, json!({ "known": known }))?)?;
        fill_track(&mut status, &mut self.last_track);
        Ok(status)
    }
    fn music_pid(&mut self) -> Result<u32> {
        let out = Proc::new("pgrep")
            .args(["-x", "Music"])
            .output()
            .context("running pgrep")?;
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines()
            .next()
            .and_then(|l| l.trim().parse().ok())
            .context("Music.app is not running")
    }
    fn play_tracks(&mut self, tracks: &[TrackId]) -> Result<()> {
        self.fill(tracks, true)?;
        self.slot = 1 - self.slot;
        Ok(())
    }
    fn prepare_tracks(&mut self, tracks: &[TrackId]) -> Result<()> {
        self.fill(tracks, false)
    }
    fn play_prepared(&mut self) -> Result<()> {
        let name = OWN_PLAYLISTS[1 - self.slot];
        self.run(PLAY_PLAYLIST, json!({ "name": name }))?;
        self.slot = 1 - self.slot;
        Ok(())
    }
    fn play_pause(&mut self) -> Result<()> {
        self.run(PLAY_PAUSE, json!({})).map(|_| ())
    }
    fn next(&mut self) -> Result<()> {
        self.run(NEXT, json!({})).map(|_| ())
    }
    fn previous(&mut self) -> Result<()> {
        self.run(PREVIOUS, json!({})).map(|_| ())
    }
    fn seek(&mut self, seconds: f64) -> Result<()> {
        self.run(SEEK, json!({ "seconds": seconds.max(0.0) }))
            .map(|_| ())
    }
    fn set_volume(&mut self, percent: u8) -> Result<()> {
        self.run(SET_VOLUME, json!({ "volume": percent.min(100) }))
            .map(|_| ())
    }
    fn set_shuffle(&mut self, on: bool) -> Result<()> {
        self.run(SET_SHUFFLE, json!({ "on": on })).map(|_| ())
    }
    fn set_repeat(&mut self, mode: RepeatMode) -> Result<()> {
        self.run(SET_REPEAT, json!({ "mode": mode.as_str() }))
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::model::PlayerState;

    #[test]
    fn parse_functions_accept_captured_output() {
        let status = parse_status(r#"{"state":"playing","track_id":"X","position_secs":1.5,"volume":80,"shuffle":true,"repeat":"one"}"#).unwrap();
        assert_eq!(status.volume, 80);
        let lib = parse_library(r#"[{"id":"A","name":"n","artist":"a","album":"b","album_artist":"","duration_secs":1,"track_number":0,"disc_number":0,"year":0}]"#).unwrap();
        assert_eq!(lib.len(), 1);
        let pls =
            parse_playlists(r#"[{"id":"P","name":"p","smart":false,"track_ids":[]}]"#).unwrap();
        assert_eq!(pls[0].name, "p");
    }

    /// Plays two library tracks through the appytui playlist and checks that
    /// Music.app continues with the second one. Mutes while it runs.
    #[test]
    #[ignore = "drives the real Music.app"]
    fn live_play_tracks_continues_within_the_list() {
        let mut b = JxaBridge::new();
        b.ensure_running().unwrap();
        let lib = b.load_library().unwrap();
        let ids: Vec<TrackId> = lib.iter().skip(300).take(2).map(|t| t.id.clone()).collect();
        let volume = b.status().unwrap().volume;
        b.set_volume(0).unwrap();
        b.play_tracks(&ids).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let first = b.status().unwrap().track_id;
        b.next().unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let second = b.status().unwrap().track_id;
        // Prepare the idle playlist while playing, then switch to it.
        b.prepare_tracks(&[ids[0].clone()]).unwrap();
        assert_eq!(b.status().unwrap().track_id.as_ref(), Some(&ids[1]));
        b.play_prepared().unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let third = b.status().unwrap().track_id;
        b.play_pause().unwrap();
        b.set_volume(volume).unwrap();
        assert_eq!(first.as_ref(), Some(&ids[0]));
        assert_eq!(second.as_ref(), Some(&ids[1]));
        assert_eq!(third.as_ref(), Some(&ids[0]));
        let playlists = b.load_playlists().unwrap();
        assert!(
            playlists
                .iter()
                .all(|p| !OWN_PLAYLISTS.contains(&p.name.as_str()))
        );
    }

    fn status_with(track_id: Option<&str>, track: Option<Track>) -> PlayerStatus {
        PlayerStatus {
            state: PlayerState::Playing,
            track_id: track_id.map(|t| TrackId(t.into())),
            track,
            ..PlayerStatus::default()
        }
    }

    #[test]
    fn fill_track_caches_a_snapshot_and_reuses_it_for_the_same_id() {
        let snap = crate::music::fake::track("X", "Song", "Artist", "Album");
        let mut cache = None;
        let mut first = status_with(Some("X"), Some(snap.clone()));
        fill_track(&mut first, &mut cache);
        assert_eq!(cache.as_ref(), Some(&snap));
        let mut next = status_with(Some("X"), None);
        fill_track(&mut next, &mut cache);
        assert_eq!(next.track.as_ref(), Some(&snap));
    }

    #[test]
    fn fill_track_does_not_reuse_a_snapshot_for_another_id() {
        let mut cache = Some(crate::music::fake::track("X", "Song", "Artist", "Album"));
        let mut other = status_with(Some("Y"), None);
        fill_track(&mut other, &mut cache);
        assert_eq!(other.track, None);
        let mut stopped = status_with(None, None);
        stopped.state = PlayerState::Stopped;
        fill_track(&mut stopped, &mut cache);
        assert_eq!(stopped.track, None);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_status("execution error: blah").is_err());
    }

    #[test]
    fn server_passes_args_and_returns_the_result() {
        let mut b = JxaBridge::new();
        assert_eq!(
            b.eval("ARGS.x + 1;", json!({"x": 41}), TIMEOUT).unwrap(),
            "42"
        );
        assert_eq!(
            b.eval(
                "JSON.stringify({ a: ARGS.a });",
                json!({"a": "é\n"}),
                TIMEOUT
            )
            .unwrap(),
            r#"{"a":"é\n"}"#
        );
    }

    #[test]
    fn server_is_reused_between_calls() {
        let mut b = JxaBridge::new();
        b.eval("1;", json!({}), TIMEOUT).unwrap();
        let pid = b.server_pid();
        b.eval("2;", json!({}), TIMEOUT).unwrap();
        assert_eq!(b.server_pid(), pid);
    }

    #[test]
    fn server_reports_script_errors_and_keeps_running() {
        let mut b = JxaBridge::new();
        let err = b
            .eval("throw new Error('boom');", json!({}), TIMEOUT)
            .unwrap_err();
        assert!(err.to_string().contains("boom"), "{err}");
        let pid = b.server_pid();
        assert_eq!(b.eval("3;", json!({}), TIMEOUT).unwrap(), "3");
        assert_eq!(b.server_pid(), pid);
    }

    #[test]
    fn server_times_out_and_is_replaced() {
        let mut b = JxaBridge::new();
        b.eval("1;", json!({}), TIMEOUT).unwrap();
        let pid = b.server_pid();
        let err = b
            .eval("while (true) {}", json!({}), Duration::from_millis(300))
            .unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
        assert_eq!(b.eval("4;", json!({}), TIMEOUT).unwrap(), "4");
        assert_ne!(b.server_pid(), pid);
    }

    #[test]
    #[ignore = "needs Music.app"]
    fn live_status_and_library() {
        let mut b = JxaBridge::new();
        b.ensure_running().unwrap();
        let s = b.status().unwrap();
        println!("{s:?}");
        // The second poll skips the track snapshot but must still carry it.
        let t0 = Instant::now();
        let again = b.status().unwrap();
        println!("second status poll took {:?}", t0.elapsed());
        assert_eq!(again.track_id, s.track_id);
        assert_eq!(again.track.is_some(), s.track.is_some());
        let lib = b.load_library().unwrap();
        assert!(!lib.is_empty());
        assert!(b.music_pid().unwrap() > 0);
    }
}
