//! The app-side play context. Music.app does not expose Up Next over
//! AppleScript, so the queue is derived from the list the user played from.

use crate::music::model::{PlaySource, TrackId};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayContext {
    pub track_ids: Vec<TrackId>,
    pub index: usize,
    pub source: PlaySource,
}

impl PlayContext {
    pub fn new(track_ids: Vec<TrackId>, index: usize, source: PlaySource) -> Self {
        Self {
            track_ids,
            index,
            source,
        }
    }

    /// Point `index` at `id`: the nearest occurrence at or after the current
    /// index, else the first occurrence. Unknown ids leave the index alone.
    pub fn resync(&mut self, id: &TrackId) {
        let from = self.index.min(self.track_ids.len());
        if let Some(pos) = self.track_ids[from..].iter().position(|t| t == id) {
            self.index = from + pos;
        } else if let Some(pos) = self.track_ids.iter().position(|t| t == id) {
            self.index = pos;
        }
    }

    /// Tracks after the current one.
    pub fn upcoming(&self) -> &[TrackId] {
        let start = (self.index + 1).min(self.track_ids.len());
        &self.track_ids[start..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(s: &[&str]) -> Vec<TrackId> {
        s.iter().map(|x| TrackId(x.to_string())).collect()
    }

    #[test]
    fn resync_prefers_next_occurrence() {
        let mut c = PlayContext::new(ids(&["a", "b", "a", "c"]), 1, PlaySource::Library);
        c.resync(&TrackId("a".into()));
        assert_eq!(c.index, 2);
        c.resync(&TrackId("b".into()));
        assert_eq!(c.index, 1);
        c.resync(&TrackId("zzz".into()));
        assert_eq!(c.index, 1);
    }

    #[test]
    fn upcoming_is_after_current() {
        let c = PlayContext::new(ids(&["a", "b", "c"]), 0, PlaySource::Library);
        assert_eq!(c.upcoming(), &ids(&["b", "c"])[..]);
        let end = PlayContext::new(ids(&["a"]), 0, PlaySource::Library);
        assert!(end.upcoming().is_empty());
        assert!(PlayContext::default().upcoming().is_empty());
    }
}
