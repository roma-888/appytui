//! Album art via Apple's public iTunes Search endpoint, cached on disk.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::thread::JoinHandle;

use anyhow::{Context, Result, anyhow};
use crossbeam_channel::{Receiver, Sender};
use image::RgbImage;

pub mod render;

#[derive(Debug, Clone, PartialEq)]
pub struct ArtRequest {
    pub key: String,
    pub artist: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ArtResult {
    pub key: String,
    pub image: Option<RgbImage>,
}

impl PartialEq for ArtResult {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.image.is_some() == other.image.is_some()
    }
}

pub fn cache_key(artist: &str, album: &str) -> String {
    let mut h = DefaultHasher::new();
    artist.to_lowercase().hash(&mut h);
    album.to_lowercase().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// `~/Library/Caches/appytui/art`
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("appytui")
        .join("art")
}

#[derive(serde::Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(serde::Deserialize)]
struct SearchResult {
    #[serde(rename = "artworkUrl100")]
    artwork_url_100: Option<String>,
}

pub fn parse_lookup(json: &str) -> Result<String> {
    let resp: SearchResponse =
        serde_json::from_str(json).context("parsing iTunes search response")?;
    resp.results
        .into_iter()
        .find_map(|r| r.artwork_url_100)
        .ok_or_else(|| anyhow!("no artwork in search results"))
}

pub fn lookup_url(artist: &str, name: &str) -> Result<String> {
    let term: String = format!("{artist} {name}")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("+");
    let url = format!("https://itunes.apple.com/search?term={term}&entity=song&limit=1");
    let body = ureq::get(&url)
        .call()
        .context("iTunes search request")?
        .body_mut()
        .read_to_string()
        .context("reading search body")?;
    parse_lookup(&body)
}

fn fetch(cache_dir: &PathBuf, req: &ArtRequest) -> Result<RgbImage> {
    let path = cache_dir.join(format!("{}.jpg", req.key));
    let bytes = if path.is_file() {
        std::fs::read(&path).context("reading cached art")?
    } else {
        let url = lookup_url(&req.artist, &req.name)?;
        let bytes = ureq::get(&url)
            .call()
            .context("downloading art")?
            .body_mut()
            .read_to_vec()
            .context("reading art body")?;
        std::fs::create_dir_all(cache_dir).ok();
        std::fs::write(&path, &bytes).ok();
        bytes
    };
    Ok(image::load_from_memory(&bytes)
        .context("decoding art")?
        .to_rgb8())
}

pub fn spawn(
    cache_dir: PathBuf,
    rx: Receiver<ArtRequest>,
    tx: Sender<ArtResult>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("album-art".into())
        .spawn(move || {
            while let Ok(req) = rx.recv() {
                // Skip stale requests if the track changed several times quickly.
                let req = rx.try_iter().last().unwrap_or(req);
                let image = fetch(&cache_dir, &req).ok();
                if tx
                    .send(ArtResult {
                        key: req.key,
                        image,
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .expect("spawn art thread")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_and_case_insensitive() {
        assert_eq!(
            cache_key("a-ha", "Hunting High"),
            cache_key("A-HA", "hunting high")
        );
        assert_ne!(cache_key("a", "b"), cache_key("a", "c"));
        assert_eq!(cache_key("x", "y").len(), 16);
    }

    #[test]
    fn parse_lookup_takes_first_artwork() {
        let json =
            r#"{"resultCount":1,"results":[{"artworkUrl100":"https://example/100x100bb.jpg"}]}"#;
        assert_eq!(parse_lookup(json).unwrap(), "https://example/100x100bb.jpg");
        assert!(parse_lookup(r#"{"results":[]}"#).is_err());
    }

    #[test]
    fn worker_serves_from_cache_without_network() {
        let dir = std::env::temp_dir().join(format!("appytui-art-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = RgbImage::new(4, 4);
        img.put_pixel(0, 0, image::Rgb([9, 9, 9]));
        let key = cache_key("Artist", "Album");
        img.save(dir.join(format!("{key}.jpg"))).unwrap();

        let (req_tx, req_rx) = crossbeam_channel::unbounded();
        let (res_tx, res_rx) = crossbeam_channel::unbounded();
        let handle = spawn(dir.clone(), req_rx, res_tx);
        req_tx
            .send(ArtRequest {
                key: key.clone(),
                artist: "Artist".into(),
                name: "Song".into(),
            })
            .unwrap();
        let res = res_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(res.key, key);
        assert!(res.image.is_some());
        drop(req_tx);
        handle.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[ignore = "needs network"]
    fn live_lookup_finds_cover() {
        let url = lookup_url("a-ha", "Take On Me").unwrap();
        assert!(url.ends_with(".jpg"), "{url}");
    }
}
