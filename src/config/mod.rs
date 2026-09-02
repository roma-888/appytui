//! User configuration: ~/.config/appytui/config.toml

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub mod theme;

pub const DEFAULT_CONFIG: &str = r#"# appytui configuration
# Delete any line to fall back to its default.

[visualizer]
enabled = true
# bottom | top | horizontal (bars grow up and down from a centre line)
orientation = "horizontal"
# stereo (mirrored, bass in the centre) | mono (low to high, left to right)
channels = "stereo"
# left | right | average — which channel mono mode analyses
mono_option = "average"
reverse = false
# horizontal orientation only: left channel above the line, right below
horizontal_stereo = false
bar_width = 2
bar_spacing = 1
lower_cutoff_freq = 50
higher_cutoff_freq = 5000
framerate = 30
autosens = true
sensitivity = 100
# 0 = fast and noisy, 100 = slow and smooth
noise_reduction = 77
monstercat = true
waves = true
waveform = false
show_idle_bar_heads = true

[theme]
# Built-in: catppuccin-mocha, catppuccin-mocha-h, solarized_dark, tricolor, terminal.
# Or the name of a cava theme file in ~/.config/cava/themes or ~/.config/appytui/themes.
name = "catppuccin-mocha"

[art]
enabled = true
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    Bottom,
    Top,
    Horizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channels {
    Stereo,
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MonoOption {
    Left,
    Right,
    Average,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VizSettings {
    pub enabled: bool,
    pub orientation: Orientation,
    pub channels: Channels,
    pub mono_option: MonoOption,
    pub reverse: bool,
    pub horizontal_stereo: bool,
    pub bar_width: u16,
    pub bar_spacing: u16,
    pub lower_cutoff_freq: u32,
    pub higher_cutoff_freq: u32,
    pub framerate: u32,
    pub autosens: bool,
    pub sensitivity: u32,
    pub noise_reduction: u8,
    pub monstercat: bool,
    pub waves: bool,
    pub waveform: bool,
    pub show_idle_bar_heads: bool,
}

impl Default for VizSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            orientation: Orientation::Horizontal,
            channels: Channels::Stereo,
            mono_option: MonoOption::Average,
            reverse: false,
            horizontal_stereo: false,
            bar_width: 2,
            bar_spacing: 1,
            lower_cutoff_freq: 50,
            higher_cutoff_freq: 5000,
            framerate: 30,
            autosens: true,
            sensitivity: 100,
            noise_reduction: 77,
            monstercat: true,
            waves: true,
            waveform: false,
            show_idle_bar_heads: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub name: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self { name: "catppuccin-mocha".to_string() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtConfig {
    pub enabled: bool,
}

impl Default for ArtConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub visualizer: VizSettings,
    pub theme: ThemeConfig,
    pub art: ArtConfig,
}

impl Config {
    /// `~/.config/appytui/config.toml`
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("appytui")
            .join("config.toml")
    }

    /// Load the config, writing the commented default file first if it does not exist.
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(path, DEFAULT_CONFIG)
                .with_context(|| format!("writing {}", path.display()))?;
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_text_matches_default_struct() {
        let parsed: Config = toml::from_str(DEFAULT_CONFIG).expect("DEFAULT_CONFIG parses");
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn defaults_match_users_cava_setup() {
        let v = VizSettings::default();
        assert_eq!(v.orientation, Orientation::Horizontal);
        assert_eq!(v.channels, Channels::Stereo);
        assert!(v.monstercat && v.waves);
        assert_eq!(v.noise_reduction, 77);
        assert_eq!((v.lower_cutoff_freq, v.higher_cutoff_freq), (50, 5000));
        assert_eq!((v.bar_width, v.bar_spacing), (2, 1));
        assert_eq!(Config::default().theme.name, "catppuccin-mocha");
    }

    #[test]
    fn load_creates_file_when_missing() {
        let dir = std::env::temp_dir().join(format!("appytui-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = Config::load(&path).expect("load creates file");
        assert!(path.exists());
        assert_eq!(cfg, Config::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn partial_file_fills_in_defaults() {
        let cfg: Config = toml::from_str("[visualizer]\nbar_width = 1\n").unwrap();
        assert_eq!(cfg.visualizer.bar_width, 1);
        assert_eq!(cfg.visualizer.bar_spacing, 1);
        assert_eq!(cfg.theme.name, "catppuccin-mocha");
    }
}
