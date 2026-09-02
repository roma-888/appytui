//! In-memory library with derived album and artist indexes.

use std::collections::HashMap;

use crate::music::model::{Track, TrackId};

#[derive(Debug, Clone, PartialEq)]
pub struct AlbumEntry {
    pub artist: String,
    pub album: String,
    /// Indexes into `Library::tracks`, ordered by disc then track number.
    pub tracks: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtistEntry {
    pub name: String,
    /// Indexes into `Library::tracks`, ordered by album, disc, track number.
    pub tracks: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct Library {
    /// Sorted by track name.
    pub tracks: Vec<Track>,
    by_id: HashMap<TrackId, usize>,
    pub albums: Vec<AlbumEntry>,
    pub artists: Vec<ArtistEntry>,
}

/// Case-insensitive sort key ignoring a leading "The ".
pub fn sort_key(s: &str) -> String {
    let lower = s.trim().to_lowercase();
    lower.strip_prefix("the ").map(str::to_string).unwrap_or(lower)
}

impl Library {
    pub fn new(mut tracks: Vec<Track>) -> Library {
        tracks.sort_by_cached_key(|t| (sort_key(&t.name), sort_key(&t.artist)));
        let by_id = tracks.iter().enumerate().map(|(i, t)| (t.id.clone(), i)).collect();

        let mut album_map: HashMap<(String, String), (String, String, Vec<usize>)> = HashMap::new();
        let mut artist_map: HashMap<String, (String, Vec<usize>)> = HashMap::new();
        for (i, t) in tracks.iter().enumerate() {
            let artist = t.grouping_artist().to_string();
            album_map
                .entry((sort_key(&artist), sort_key(&t.album)))
                .or_insert_with(|| (artist.clone(), t.album.clone(), Vec::new()))
                .2
                .push(i);
            artist_map.entry(sort_key(&artist)).or_insert_with(|| (artist.clone(), Vec::new())).1.push(i);
        }

        let disc_track = |tracks: &[Track], i: usize| (tracks[i].disc_number, tracks[i].track_number);
        let mut albums: Vec<AlbumEntry> = album_map
            .into_values()
            .map(|(artist, album, mut idx)| {
                idx.sort_by_key(|&i| disc_track(&tracks, i));
                AlbumEntry { artist, album, tracks: idx }
            })
            .collect();
        albums.sort_by_cached_key(|a| (sort_key(&a.album), sort_key(&a.artist)));

        let mut artists: Vec<ArtistEntry> = artist_map
            .into_values()
            .map(|(name, mut idx)| {
                idx.sort_by_cached_key(|&i| (sort_key(&tracks[i].album), disc_track(&tracks, i)));
                ArtistEntry { name, tracks: idx }
            })
            .collect();
        artists.sort_by_cached_key(|a| sort_key(&a.name));

        Library { tracks, by_id, albums, artists }
    }

    pub fn index_of(&self, id: &TrackId) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    pub fn get(&self, id: &TrackId) -> Option<&Track> {
        self.index_of(id).map(|i| &self.tracks[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::fake::track;

    fn t(id: &str, name: &str, artist: &str, album: &str, disc: u32, num: u32) -> Track {
        let mut t = track(id, name, artist, album);
        t.disc_number = disc;
        t.track_number = num;
        t
    }

    #[test]
    fn sort_key_ignores_case_and_leading_the() {
        assert_eq!(sort_key("The Beatles"), "beatles");
        assert_eq!(sort_key("  Beck"), "beck");
        assert_eq!(sort_key("the"), "the");
    }

    #[test]
    fn groups_albums_and_artists_sorted() {
        let lib = Library::new(vec![
            t("1", "B song", "Zed", "Z Album", 1, 2),
            t("2", "A song", "Zed", "Z Album", 1, 1),
            t("3", "C song", "The Alphas", "A Album", 1, 1),
            t("4", "D song", "Zed", "Y Album", 2, 1),
        ]);
        assert_eq!(lib.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["The Alphas", "Zed"]);
        assert_eq!(lib.albums.iter().map(|a| a.album.as_str()).collect::<Vec<_>>(), vec!["A Album", "Y Album", "Z Album"]);
        let z = &lib.albums[2];
        assert_eq!(z.tracks.iter().map(|&i| lib.tracks[i].id.0.as_str()).collect::<Vec<_>>(), vec!["2", "1"]);
        assert_eq!(lib.artists[1].tracks.len(), 3);
    }

    #[test]
    fn album_artist_wins_over_artist() {
        let mut a = t("1", "x", "feat. guy", "Comp", 1, 1);
        a.album_artist = "Main".into();
        let lib = Library::new(vec![a]);
        assert_eq!(lib.albums[0].artist, "Main");
        assert_eq!(lib.artists[0].name, "Main");
    }

    #[test]
    fn lookup_by_id() {
        let lib = Library::new(vec![t("abc", "x", "y", "z", 1, 1)]);
        assert_eq!(lib.index_of(&TrackId("abc".into())), Some(0));
        assert!(lib.get(&TrackId("nope".into())).is_none());
    }
}
