//! Real bridge: JavaScript for Automation scripts run through `osascript`.
//! Arguments travel in the APPYTUI_ARGS environment variable as JSON, never by
//! string interpolation into the script.

use std::io::Read;
use std::process::{Command as Proc, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::MusicBridge;
use super::model::{PlayerStatus, Playlist, PlaylistId, RepeatMode, Track, TrackId};

pub const TIMEOUT: Duration = Duration::from_secs(5);

const PRELUDE: &str = r#"
ObjC.import("stdlib");
const ARGS = JSON.parse($.getenv("APPYTUI_ARGS"));
const Music = Application("Music");
const lib = Music.libraryPlaylists[0];
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
  out.push({ id: p.persistentID(), name: p.name(), smart: p.smart(), track_ids: p.tracks.persistentID() });
}
JSON.stringify(out);
"#;

const STATUS: &str = r#"
const st = Music.playerState();
let track_id = null, track = null;
if (st !== "stopped") {
  try {
    const t = Music.currentTrack;
    track_id = t.persistentID();
    // Snapshot so streamed tracks that are not in the library still show up.
    track = { id: track_id, name: t.name() || "", artist: t.artist() || "", album: t.album() || "",
      album_artist: t.albumArtist() || "", duration_secs: t.duration() || 0,
      track_number: t.trackNumber() || 0, disc_number: t.discNumber() || 0, year: t.year() || 0 };
  } catch (e) {}
}
JSON.stringify({ state: st, track_id, track, position_secs: Music.playerPosition() || 0,
  volume: Music.soundVolume(), shuffle: Music.shuffleEnabled(), repeat: Music.songRepeat() });
"#;

const PLAY_TRACK: &str = r#"
let t;
if (ARGS.playlist) {
  const pl = Music.playlists.whose({ persistentID: ARGS.playlist })[0];
  t = pl.tracks.whose({ persistentID: ARGS.track })[0];
} else {
  t = lib.tracks.whose({ persistentID: ARGS.track })[0];
}
t.play();
"ok";
"#;

const PLAY_PAUSE: &str = "Music.playpause(); \"ok\";";
const NEXT: &str = "Music.nextTrack(); \"ok\";";
const PREVIOUS: &str = "Music.backTrack(); \"ok\";";
const SEEK: &str = "Music.playerPosition = ARGS.seconds; \"ok\";";
const SET_VOLUME: &str = "Music.soundVolume = ARGS.volume; \"ok\";";
const SET_SHUFFLE: &str = "Music.shuffleEnabled = ARGS.on; \"ok\";";
const SET_REPEAT: &str = "Music.songRepeat = ARGS.mode; \"ok\";";
const LAUNCH: &str = "if (!Music.running()) { Music.activate(); } \"ok\";";

/// Run a JXA script with `args` available as the `ARGS` constant. Kills the
/// process after `timeout`.
pub fn run_script(script: &str, args: &Value, timeout: Duration) -> Result<String> {
    let full = format!("{PRELUDE}\n{script}");
    let mut child = Proc::new("osascript")
        .args(["-l", "JavaScript", "-e", &full])
        .env("APPYTUI_ARGS", args.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning osascript")?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let mut err = Vec::new();
        let _ = stderr.read_to_end(&mut err);
        (out, err)
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting for osascript")? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("osascript timed out after {}s (is Music.app showing a dialog?)", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let (out, err) = reader.join().expect("reader thread");
    if !status.success() {
        let msg = String::from_utf8_lossy(&err);
        bail!("osascript failed: {}", msg.trim());
    }
    Ok(String::from_utf8_lossy(&out).trim_end().to_string())
}

pub struct JxaBridge {
    timeout: Duration,
}

impl JxaBridge {
    pub fn new() -> Self {
        Self { timeout: TIMEOUT }
    }

    fn run(&self, script: &str, args: Value) -> Result<String> {
        run_script(script, &args, self.timeout)
    }

    /// Launch Music.app if needed. Called once at startup.
    pub fn ensure_running(&self) -> Result<()> {
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
    fn load_library(&mut self) -> Result<Vec<Track>> {
        parse_library(&self.run(LIBRARY, json!({}))?)
    }
    fn load_playlists(&mut self) -> Result<Vec<Playlist>> {
        // Bulk dump of every playlist takes a few seconds on large libraries.
        parse_playlists(&run_script(PLAYLISTS, &json!({}), Duration::from_secs(60))?)
    }
    fn status(&mut self) -> Result<PlayerStatus> {
        parse_status(&self.run(STATUS, json!({}))?)
    }
    fn music_pid(&mut self) -> Result<u32> {
        let out = Proc::new("pgrep").args(["-x", "Music"]).output().context("running pgrep")?;
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines().next().and_then(|l| l.trim().parse().ok()).context("Music.app is not running")
    }
    fn play_track(&mut self, track: &TrackId, context: Option<&PlaylistId>) -> Result<()> {
        let args = json!({ "track": track.0, "playlist": context.map(|c| c.0.clone()) });
        self.run(PLAY_TRACK, args).map(|_| ())
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
        self.run(SEEK, json!({ "seconds": seconds.max(0.0) })).map(|_| ())
    }
    fn set_volume(&mut self, percent: u8) -> Result<()> {
        self.run(SET_VOLUME, json!({ "volume": percent.min(100) })).map(|_| ())
    }
    fn set_shuffle(&mut self, on: bool) -> Result<()> {
        self.run(SET_SHUFFLE, json!({ "on": on })).map(|_| ())
    }
    fn set_repeat(&mut self, mode: RepeatMode) -> Result<()> {
        self.run(SET_REPEAT, json!({ "mode": mode.as_str() })).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_functions_accept_captured_output() {
        let status = parse_status(r#"{"state":"playing","track_id":"X","position_secs":1.5,"volume":80,"shuffle":true,"repeat":"one"}"#).unwrap();
        assert_eq!(status.volume, 80);
        let lib = parse_library(r#"[{"id":"A","name":"n","artist":"a","album":"b","album_artist":"","duration_secs":1,"track_number":0,"disc_number":0,"year":0}]"#).unwrap();
        assert_eq!(lib.len(), 1);
        let pls = parse_playlists(r#"[{"id":"P","name":"p","smart":false,"track_ids":[]}]"#).unwrap();
        assert_eq!(pls[0].name, "p");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_status("execution error: blah").is_err());
    }

    #[test]
    fn run_script_passes_args_through_env() {
        let out = run_script("ARGS.x + 1;", &json!({"x": 41}), TIMEOUT).unwrap();
        assert_eq!(out, "42");
    }

    #[test]
    fn run_script_times_out() {
        let err = run_script("while (true) {}", &json!({}), Duration::from_millis(300)).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[test]
    #[ignore = "needs Music.app"]
    fn live_status_and_library() {
        let mut b = JxaBridge::new();
        b.ensure_running().unwrap();
        let s = b.status().unwrap();
        println!("{s:?}");
        let lib = b.load_library().unwrap();
        assert!(!lib.is_empty());
        assert!(b.music_pid().unwrap() > 0);
    }
}
