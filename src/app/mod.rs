//! Application state and the pure reducer.

use std::time::{Duration, Instant};

use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use crate::config::VizSettings;
use crate::config::theme::Theme;
use crate::music::model::{PlayerState, PlayerStatus, Playlist, Track, TrackId};

pub mod library;
pub mod playback;
pub mod queue;
pub mod reducer;
#[cfg(test)]
pub mod testing;
pub mod views;

use library::Library;
use queue::PlayContext;
use views::{Drill, Filter, Row, Tab, TabView};

pub const MESSAGE_TTL: Duration = Duration::from_secs(5);
/// After a volume/shuffle/repeat key, ignore those fields from status polls
/// that were already in flight, so they cannot undo the optimistic update.
pub const OPTIMISTIC_WINDOW: Duration = Duration::from_millis(1500);

pub struct App {
    pub library: Option<Library>,
    pub playlists: Vec<Playlist>,
    pub tab: Tab,
    pub views: [TabView; 6],
    /// True while the filter line is being edited.
    pub editing_filter: bool,
    pub status: PlayerStatus,
    pub status_at: Instant,
    /// Set when a transport key changed volume/shuffle/repeat locally.
    pub optimistic_at: Option<Instant>,
    pub context: PlayContext,
    pub message: Option<(String, Instant)>,
    pub show_help: bool,
    pub viz: VizSettings,
    pub viz_simulated: bool,
    pub viz_frame: Option<crate::viz::Frame>,
    /// Bars the now-playing pane can show; set during draw, read by main.rs.
    pub viz_bars_wanted: usize,
    pub theme: Theme,
    pub truecolor: bool,
    pub art_enabled: bool,
    /// Terminal graphics capability; set once at startup by main.rs.
    pub picker: Picker,
    /// Cover for the current album, keyed by `art::cache_key`, ready to render.
    pub art: Option<(String, StatefulProtocol)>,
    pub art_key: Option<String>,
    pub music_pid: Option<u32>,
    pub should_quit: bool,
    /// False until the bulk playlist dump arrives (it takes a few seconds).
    pub playlists_loaded: bool,
    filter: Filter,
    /// Rows for the last (tab, drill, filter, generation); avoids re-running the
    /// fuzzy matcher over the whole library on every animation frame.
    rows_cache: Option<(RowsKey, Vec<Row>)>,
    rows_gen: u64,
    /// The idle playlist holds an edited queue waiting for the track boundary.
    pub pending_requeue: bool,
    /// Track we just switched away from; polls still naming it are stale.
    pub switching_from: Option<TrackId>,
    /// xorshift state for shuffle sampling; avoids a dependency for one use.
    rng: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RowsKey {
    tab: Tab,
    drill: Drill,
    filter: String,
    generation: u64,
}

impl App {
    pub fn new(theme: Theme, viz: VizSettings, art_enabled: bool) -> App {
        App {
            library: None,
            playlists: Vec::new(),
            tab: Tab::Songs,
            views: Default::default(),
            editing_filter: false,
            status: PlayerStatus::default(),
            status_at: Instant::now(),
            optimistic_at: None,
            context: PlayContext::default(),
            message: None,
            show_help: false,
            viz,
            viz_simulated: false,
            viz_frame: None,
            viz_bars_wanted: 0,
            theme,
            truecolor: crate::config::theme::terminal_supports_truecolor(),
            art_enabled,
            picker: Picker::halfblocks(),
            art: None,
            art_key: None,
            music_pid: None,
            should_quit: false,
            playlists_loaded: false,
            filter: Filter::new(),
            rows_cache: None,
            rows_gen: 0,
            pending_requeue: false,
            switching_from: None,
            rng: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0)
                | 1,
        }
    }

    /// Next pseudo-random number (xorshift64*).
    pub fn random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn view(&self) -> &TabView {
        &self.views[self.tab.index()]
    }

    pub fn view_mut(&mut self) -> &mut TabView {
        &mut self.views[self.tab.index()]
    }

    /// The playing track: the library entry when it has one, else the
    /// snapshot Music.app reported (streamed tracks are often not in the library).
    pub fn current_track(&self) -> Option<&Track> {
        let id = self.status.track_id.as_ref()?;
        self.library
            .as_ref()
            .and_then(|l| l.get(id))
            .or(self.status.track.as_ref())
    }

    /// Player position interpolated from the last status poll.
    pub fn position_now(&self) -> f64 {
        let base = self.status.position_secs;
        let pos = if self.status.state == PlayerState::Playing {
            base + self.status_at.elapsed().as_secs_f64()
        } else {
            base
        };
        match self.current_track() {
            Some(t) if t.duration_secs > 0.0 => pos.min(t.duration_secs),
            _ => pos,
        }
    }

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
    }

    /// Call whenever library, playlists or the play context change.
    pub fn invalidate_rows(&mut self) {
        self.rows_gen = self.rows_gen.wrapping_add(1);
    }

    /// How many times the fuzzy matcher has run (for cache tests).
    #[cfg(test)]
    pub fn rank_calls(&self) -> usize {
        self.filter.rank_calls
    }

    /// Visible rows for `tab`, honouring drill-down and the filter. Cached.
    pub fn rows(&mut self, tab: Tab) -> Vec<Row> {
        let view = &self.views[tab.index()];
        let key = RowsKey {
            tab,
            drill: view.drill,
            filter: view.filter.clone(),
            generation: self.rows_gen,
        };
        if let Some((k, rows)) = &self.rows_cache
            && *k == key
        {
            return rows.clone();
        }
        let rows = self.compute_rows(tab);
        self.rows_cache = Some((key, rows.clone()));
        rows
    }

    fn compute_rows(&mut self, tab: Tab) -> Vec<Row> {
        let Some(lib) = self.library.as_ref() else {
            return Vec::new();
        };
        let view = &self.views[tab.index()];
        let unfiltered: Vec<Row> = match (tab, view.drill) {
            (Tab::Songs, _) => (0..lib.tracks.len()).map(Row::Track).collect(),
            (Tab::Albums, Drill::Top) => (0..lib.albums.len()).map(Row::Album).collect(),
            (Tab::Albums, Drill::Album(a)) => lib.albums[a]
                .tracks
                .iter()
                .map(|&i| Row::Track(i))
                .collect(),
            (Tab::Artists, Drill::Top) => (0..lib.artists.len()).map(Row::Artist).collect(),
            (Tab::Artists, Drill::Artist(a)) => lib.artists[a]
                .tracks
                .iter()
                .map(|&i| Row::Track(i))
                .collect(),
            (Tab::Playlists, Drill::Top) => (0..self.playlists.len()).map(Row::Playlist).collect(),
            (Tab::Playlists, Drill::Playlist(p)) => self.playlists[p]
                .track_ids
                .iter()
                .filter_map(|id| lib.index_of(id))
                .map(Row::Track)
                .collect(),
            (Tab::Search, _) => (0..lib.tracks.len()).map(Row::Track).collect(),
            (Tab::Queue, _) => self
                .context
                .upcoming()
                .iter()
                .filter_map(|id| lib.index_of(id))
                .map(Row::Track)
                .collect(),
            _ => Vec::new(),
        };
        let query = view.filter.clone();
        if query.trim().is_empty() {
            // Search shows nothing until something is typed.
            return if tab == Tab::Search {
                Vec::new()
            } else {
                unfiltered
            };
        }
        let items: Vec<(usize, String)> = unfiltered
            .iter()
            .enumerate()
            .map(|(i, row)| (i, self.row_text(*row)))
            .collect();
        let ranked = self.filter.rank(&query, &items);
        ranked.into_iter().map(|i| unfiltered[i]).collect()
    }

    /// Text the fuzzy filter matches against.
    pub fn row_text(&self, row: Row) -> String {
        let Some(lib) = self.library.as_ref() else {
            return String::new();
        };
        match row {
            Row::Track(i) => {
                let t = &lib.tracks[i];
                format!("{} {} {}", t.name, t.artist, t.album)
            }
            Row::Album(i) => format!("{} {}", lib.albums[i].album, lib.albums[i].artist),
            Row::Artist(i) => lib.artists[i].name.clone(),
            Row::Playlist(i) => self.playlists[i].name.clone(),
        }
    }
}
