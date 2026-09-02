//! Application state and the pure reducer.

use std::time::{Duration, Instant};

use crate::config::VizSettings;
use crate::config::theme::Theme;
use crate::music::model::{PlayerState, PlayerStatus, Playlist, Track};

pub mod library;
pub mod queue;
pub mod reducer;
pub mod views;

use library::Library;
use queue::PlayContext;
use views::{Drill, Filter, Row, Tab, TabView};

pub const MESSAGE_TTL: Duration = Duration::from_secs(5);

pub struct App {
    pub library: Option<Library>,
    pub playlists: Vec<Playlist>,
    pub tab: Tab,
    pub views: [TabView; 6],
    /// True while the filter line is being edited.
    pub editing_filter: bool,
    pub status: PlayerStatus,
    pub status_at: Instant,
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
    /// Decoded cover for the current album, keyed by `art::cache_key`.
    pub art: Option<(String, image::RgbImage)>,
    pub art_key: Option<String>,
    pub music_pid: Option<u32>,
    pub should_quit: bool,
    filter: Filter,
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
            art: None,
            art_key: None,
            music_pid: None,
            should_quit: false,
            filter: Filter::new(),
        }
    }

    pub fn view(&self) -> &TabView {
        &self.views[self.tab.index()]
    }

    pub fn view_mut(&mut self) -> &mut TabView {
        &mut self.views[self.tab.index()]
    }

    pub fn current_track(&self) -> Option<&Track> {
        let id = self.status.track_id.as_ref()?;
        self.library.as_ref()?.get(id)
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

    /// Visible rows for `tab`, honouring drill-down and the filter.
    pub fn rows(&mut self, tab: Tab) -> Vec<Row> {
        let Some(lib) = self.library.as_ref() else { return Vec::new() };
        let view = &self.views[tab.index()];
        let unfiltered: Vec<Row> = match (tab, view.drill) {
            (Tab::Songs, _) => (0..lib.tracks.len()).map(Row::Track).collect(),
            (Tab::Albums, Drill::Top) => (0..lib.albums.len()).map(Row::Album).collect(),
            (Tab::Albums, Drill::Album(a)) => lib.albums[a].tracks.iter().map(|&i| Row::Track(i)).collect(),
            (Tab::Artists, Drill::Top) => (0..lib.artists.len()).map(Row::Artist).collect(),
            (Tab::Artists, Drill::Artist(a)) => lib.artists[a].tracks.iter().map(|&i| Row::Track(i)).collect(),
            (Tab::Playlists, Drill::Top) => (0..self.playlists.len()).map(Row::Playlist).collect(),
            (Tab::Playlists, Drill::Playlist(p)) => {
                self.playlists[p].track_ids.iter().filter_map(|id| lib.index_of(id)).map(Row::Track).collect()
            }
            (Tab::Search, _) => (0..lib.tracks.len()).map(Row::Track).collect(),
            (Tab::Queue, _) => {
                self.context.upcoming().iter().filter_map(|id| lib.index_of(id)).map(Row::Track).collect()
            }
            _ => Vec::new(),
        };
        let query = view.filter.clone();
        if query.trim().is_empty() {
            // Search shows nothing until something is typed.
            return if tab == Tab::Search { Vec::new() } else { unfiltered };
        }
        let items: Vec<(usize, String)> =
            unfiltered.iter().enumerate().map(|(i, row)| (i, self.row_text(*row))).collect();
        let ranked = self.filter.rank(&query, &items);
        ranked.into_iter().map(|i| unfiltered[i]).collect()
    }

    /// Text the fuzzy filter matches against.
    pub fn row_text(&self, row: Row) -> String {
        let Some(lib) = self.library.as_ref() else { return String::new() };
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
