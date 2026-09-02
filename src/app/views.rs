//! Tabs, drill-down state and fuzzy filtering.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Songs,
    Albums,
    Artists,
    Playlists,
    Search,
    Queue,
}

impl Tab {
    pub const ALL: [Tab; 6] = [Tab::Songs, Tab::Albums, Tab::Artists, Tab::Playlists, Tab::Search, Tab::Queue];

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Option<Tab> {
        Tab::ALL.get(i).copied()
    }

    pub fn title(self) -> &'static str {
        match self {
            Tab::Songs => "Songs",
            Tab::Albums => "Albums",
            Tab::Artists => "Artists",
            Tab::Playlists => "Playlists",
            Tab::Search => "Search",
            Tab::Queue => "Queue",
        }
    }
}

/// One visible row in the left pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Index into `Library::tracks`.
    Track(usize),
    /// Index into `Library::albums`.
    Album(usize),
    /// Index into `Library::artists`.
    Artist(usize),
    /// Index into `App::playlists`.
    Playlist(usize),
}

/// Where a drill-down tab currently is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Drill {
    #[default]
    Top,
    Album(usize),
    Artist(usize),
    Playlist(usize),
}

#[derive(Debug, Default, Clone)]
pub struct TabView {
    pub cursor: usize,
    pub drill: Drill,
    /// Cursor to restore when backing out of a drill-down.
    pub parent_cursor: usize,
    pub filter: String,
}

impl TabView {
    pub fn move_cursor(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.cursor = 0;
            return;
        }
        let next = (self.cursor as isize).saturating_add(delta);
        self.cursor = next.clamp(0, len as isize - 1) as usize;
    }
}

pub struct Filter {
    matcher: Matcher,
    buf: Vec<char>,
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter {
    pub fn new() -> Self {
        Self { matcher: Matcher::new(Config::DEFAULT), buf: Vec::new() }
    }

    /// Return the ids of matching items, best match first. Empty query matches nothing.
    pub fn rank(&mut self, query: &str, items: &[(usize, String)]) -> Vec<usize> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(u32, usize)> = items
            .iter()
            .filter_map(|(id, text)| {
                let hay = Utf32Str::new(text, &mut self.buf);
                pattern.score(hay, &mut self.matcher).map(|s| (s, *id))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, id)| id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_round_trip_index() {
        for (i, t) in Tab::ALL.iter().enumerate() {
            assert_eq!(t.index(), i);
            assert_eq!(Tab::from_index(i), Some(*t));
        }
        assert_eq!(Tab::from_index(6), None);
    }

    #[test]
    fn filter_ranks_fuzzy_matches_and_drops_misses() {
        let mut f = Filter::new();
        let items = vec![(0, "Take On Me a-ha".to_string()), (1, "Toxic Britney".to_string()), (2, "Zebra".to_string())];
        let hits = f.rank("take", &items);
        assert_eq!(hits, vec![0]);
        let hits = f.rank("t", &items);
        assert_eq!(hits.len(), 2);
        assert!(f.rank("", &items).is_empty());
    }

    #[test]
    fn tab_view_cursor_clamps() {
        let mut v = TabView::default();
        v.move_cursor(-1, 10);
        assert_eq!(v.cursor, 0);
        v.move_cursor(25, 10);
        assert_eq!(v.cursor, 9);
        v.move_cursor(-3, 0);
        assert_eq!(v.cursor, 0);
    }
}
