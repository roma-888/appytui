//! Colour themes. Files use cava's INI format so cava themes load unchanged.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlendDirection {
    #[default]
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub gradient: Vec<Rgb>,
    pub horizontal_gradient: Vec<Rgb>,
    pub blend_direction: BlendDirection,
    pub background: Option<Rgb>,
    pub foreground: Option<Rgb>,
    pub accent: Rgb,
    pub highlight: Rgb,
    pub selection_bg: Rgb,
    pub dim: Rgb,
}

const BUILTINS: &[(&str, &str)] = &[
    (
        "catppuccin-mocha",
        include_str!("../../themes/catppuccin-mocha"),
    ),
    (
        "catppuccin-mocha-h",
        include_str!("../../themes/catppuccin-mocha-h"),
    ),
    (
        "solarized_dark",
        include_str!("../../themes/solarized_dark"),
    ),
    ("tricolor", include_str!("../../themes/tricolor")),
    ("terminal", include_str!("../../themes/terminal")),
];

fn parse_color(raw: &str) -> Result<Option<Rgb>> {
    let v = raw.trim().trim_matches(|c| c == '\'' || c == '"').trim();
    let named = match v.to_ascii_lowercase().as_str() {
        "default" => return Ok(None),
        "black" => Some(Rgb(0, 0, 0)),
        "red" => Some(Rgb(255, 0, 0)),
        "green" => Some(Rgb(0, 255, 0)),
        "yellow" => Some(Rgb(255, 255, 0)),
        "blue" => Some(Rgb(0, 0, 255)),
        "magenta" => Some(Rgb(255, 0, 255)),
        "cyan" => Some(Rgb(0, 255, 255)),
        "white" => Some(Rgb(255, 255, 255)),
        _ => None,
    };
    if let Some(c) = named {
        return Ok(Some(c));
    }
    let hex = v.strip_prefix('#').unwrap_or(v);
    if hex.len() != 6 {
        bail!("bad colour {raw:?}");
    }
    let n = u32::from_str_radix(hex, 16).with_context(|| format!("bad colour {raw:?}"))?;
    Ok(Some(Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)))
}

impl Theme {
    /// Parse a cava-format theme. Unknown keys are ignored.
    pub fn parse(name: &str, text: &str) -> Result<Theme> {
        let mut gradient_on = false;
        let mut hgradient_on = false;
        let mut stops: Vec<(u8, Rgb)> = Vec::new();
        let mut hstops: Vec<(u8, Rgb)> = Vec::new();
        let mut blend = BlendDirection::Up;
        let mut background = None;
        let mut foreground = None;
        let mut ui: [Option<Rgb>; 4] = [None; 4];

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with(';')
                || line.starts_with('[')
            {
                continue;
            }
            let Some((key, val)) = line.split_once('=') else {
                continue;
            };
            let (key, val) = (key.trim(), val.trim());
            match key {
                "gradient" => gradient_on = val == "1",
                "horizontal_gradient" => hgradient_on = val == "1",
                "background" => background = parse_color(val)?,
                "foreground" => foreground = parse_color(val)?,
                "blend_direction" => {
                    blend = match val.trim_matches('\'').trim_matches('"') {
                        "down" => BlendDirection::Down,
                        "left" => BlendDirection::Left,
                        "right" => BlendDirection::Right,
                        _ => BlendDirection::Up,
                    }
                }
                "ui_accent" => ui[0] = parse_color(val)?,
                "ui_highlight" => ui[1] = parse_color(val)?,
                "ui_selection_bg" => ui[2] = parse_color(val)?,
                "ui_dim" => ui[3] = parse_color(val)?,
                _ => {
                    if let Some(n) = key.strip_prefix("gradient_color_") {
                        if let (Ok(i), Some(c)) = (n.parse::<u8>(), parse_color(val)?) {
                            stops.push((i, c));
                        }
                    } else if let Some(n) = key.strip_prefix("horizontal_gradient_color_")
                        && let (Ok(i), Some(c)) = (n.parse::<u8>(), parse_color(val)?)
                    {
                        hstops.push((i, c));
                    }
                }
            }
        }
        stops.sort_by_key(|(i, _)| *i);
        hstops.sort_by_key(|(i, _)| *i);
        let gradient: Vec<Rgb> = if gradient_on {
            stops.into_iter().map(|(_, c)| c).collect()
        } else {
            Vec::new()
        };
        let horizontal_gradient: Vec<Rgb> = if hgradient_on {
            hstops.into_iter().map(|(_, c)| c).collect()
        } else {
            Vec::new()
        };

        let fallback_fg = foreground.unwrap_or(Rgb(204, 204, 204));
        let first = gradient.first().copied().unwrap_or(fallback_fg);
        let middle = gradient
            .get(gradient.len() / 2)
            .copied()
            .unwrap_or(fallback_fg);
        Ok(Theme {
            name: name.to_string(),
            gradient,
            horizontal_gradient,
            blend_direction: blend,
            background,
            foreground,
            accent: ui[0].unwrap_or(first),
            highlight: ui[1].unwrap_or(middle),
            selection_bg: ui[2].unwrap_or(Rgb(48, 48, 48)),
            dim: ui[3].unwrap_or(Rgb(128, 128, 128)),
        })
    }

    pub fn builtin(name: &str) -> Option<Theme> {
        BUILTINS
            .iter()
            .find(|(n, _)| *n == name)
            .and_then(|(n, text)| Theme::parse(n, text).ok())
    }

    /// `~/.config/appytui/themes`, then `~/.config/cava/themes`.
    pub fn user_dirs() -> Vec<PathBuf> {
        let Some(cfg) = dirs::config_dir() else {
            return Vec::new();
        };
        vec![
            cfg.join("appytui").join("themes"),
            cfg.join("cava").join("themes"),
        ]
    }

    /// Search `dirs` in order for a file called `name`, then fall back to built-ins.
    pub fn load(name: &str, dirs: &[PathBuf]) -> Result<Theme> {
        for dir in dirs {
            let path = dir.join(name);
            if path.is_file() {
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading theme {}", path.display()))?;
                return Theme::parse(name, &text);
            }
        }
        Theme::builtin(name).with_context(|| format!("unknown theme {name:?}"))
    }

    /// Colour at position `t` in 0..=1 along the vertical gradient (0 = bottom).
    /// Falls back to the foreground colour when there is no gradient.
    pub fn gradient_at(&self, t: f32) -> Rgb {
        gradient_at(
            &self.gradient,
            self.foreground.unwrap_or(Rgb(204, 204, 204)),
            t,
        )
    }

    pub fn horizontal_gradient_at(&self, t: f32) -> Option<Rgb> {
        if self.horizontal_gradient.is_empty() {
            None
        } else {
            Some(gradient_at(
                &self.horizontal_gradient,
                self.horizontal_gradient[0],
                t,
            ))
        }
    }
}

pub fn gradient_at(stops: &[Rgb], fallback: Rgb, t: f32) -> Rgb {
    match stops.len() {
        0 => fallback,
        1 => stops[0],
        n => {
            let t = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (t.floor() as usize).min(n - 2);
            lerp(stops[i], stops[i + 1], t - i as f32)
        }
    }
}

pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

pub fn terminal_supports_truecolor() -> bool {
    matches!(
        std::env::var("COLORTERM").as_deref(),
        Ok("truecolor") | Ok("24bit")
    )
}

/// Nearest xterm-256 index for an RGB colour (6x6x6 cube or grey ramp).
fn quantise_256(c: Rgb) -> u8 {
    let level = |v: u8| -> u8 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            ((v as u16 - 35) / 40) as u8
        }
    };
    let (r, g, b) = (level(c.0), level(c.1), level(c.2));
    let cube_val = |l: u8| -> i32 { if l == 0 { 0 } else { 55 + 40 * l as i32 } };
    let cube_idx = 16 + 36 * r + 6 * g + b;
    let cube_dist = (cube_val(r) - c.0 as i32).pow(2)
        + (cube_val(g) - c.1 as i32).pow(2)
        + (cube_val(b) - c.2 as i32).pow(2);
    let avg = (c.0 as i32 + c.1 as i32 + c.2 as i32) / 3;
    let grey_level = if avg > 238 {
        23
    } else {
        ((avg - 3) / 10).max(0)
    };
    let grey_val = 8 + 10 * grey_level;
    let grey_dist = 3 * (grey_val - avg).pow(2)
        + (c.0 as i32 - avg).pow(2)
        + (c.1 as i32 - avg).pow(2)
        + (c.2 as i32 - avg).pow(2);
    if grey_dist < cube_dist {
        (232 + grey_level) as u8
    } else {
        cube_idx
    }
}

pub fn to_color(rgb: Rgb, truecolor: bool) -> Color {
    if truecolor {
        Color::Rgb(rgb.0, rgb.1, rgb.2)
    } else {
        Color::Indexed(quantise_256(rgb))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_gradient_and_ui_keys() {
        let t = Theme::parse(
            "x",
            "[color]\ngradient = 1\ngradient_color_1 = '#000000'\ngradient_color_2 = \"#ffffff\"\nui_accent = '#ff0000'\n",
        )
        .unwrap();
        assert_eq!(t.gradient, vec![Rgb(0, 0, 0), Rgb(255, 255, 255)]);
        assert_eq!(t.accent, Rgb(255, 0, 0));
    }

    #[test]
    fn gradient_zero_means_flat_foreground() {
        let t = Theme::parse(
            "x",
            "[color]\ngradient = 0\ngradient_color_1 = '#112233'\nforeground = cyan\n",
        )
        .unwrap();
        assert!(t.gradient.is_empty());
        assert_eq!(t.foreground, Some(Rgb(0, 255, 255)));
    }

    #[test]
    fn cava_theme_without_ui_keys_derives_accents_from_gradient() {
        let t = Theme::parse(
            "x",
            "gradient = 1\ngradient_color_1 = '#010203'\ngradient_color_2 = '#040506'\ngradient_color_3 = '#070809'\n",
        )
        .unwrap();
        assert_eq!(t.accent, Rgb(1, 2, 3));
        assert_eq!(t.highlight, Rgb(4, 5, 6));
    }

    #[test]
    fn builtin_catppuccin_has_eight_stops() {
        let t = Theme::builtin("catppuccin-mocha").unwrap();
        assert_eq!(t.gradient.len(), 8);
        assert_eq!(t.gradient[0], Rgb(0x89, 0xb4, 0xfa));
        assert_eq!(t.selection_bg, Rgb(0x31, 0x32, 0x44));
    }

    #[test]
    fn all_builtins_parse() {
        for name in [
            "catppuccin-mocha",
            "catppuccin-mocha-h",
            "solarized_dark",
            "tricolor",
            "terminal",
        ] {
            assert!(Theme::builtin(name).is_some(), "{name}");
        }
        assert!(Theme::builtin("nope").is_none());
    }

    #[test]
    fn load_prefers_user_dir_over_builtin() {
        let dir = std::env::temp_dir().join(format!("appytui-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("catppuccin-mocha"),
            "gradient = 1\ngradient_color_1 = '#123456'\n",
        )
        .unwrap();
        let t = Theme::load("catppuccin-mocha", std::slice::from_ref(&dir)).unwrap();
        assert_eq!(t.gradient, vec![Rgb(0x12, 0x34, 0x56)]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(Theme::load("does-not-exist", &[]).is_err());
    }

    #[test]
    fn gradient_at_interpolates() {
        let t = Theme::parse(
            "x",
            "gradient = 1\ngradient_color_1 = '#000000'\ngradient_color_2 = '#0000ff'\n",
        )
        .unwrap();
        assert_eq!(t.gradient_at(0.0), Rgb(0, 0, 0));
        assert_eq!(t.gradient_at(1.0), Rgb(0, 0, 255));
        assert_eq!(t.gradient_at(0.5), Rgb(0, 0, 128));
    }

    #[test]
    fn to_color_quantises_without_truecolor() {
        assert_eq!(to_color(Rgb(255, 0, 0), true), Color::Rgb(255, 0, 0));
        assert_eq!(to_color(Rgb(255, 0, 0), false), Color::Indexed(196));
        assert_eq!(to_color(Rgb(128, 128, 128), false), Color::Indexed(244));
    }
}
