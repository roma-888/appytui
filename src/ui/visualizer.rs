//! Renders a `viz::Frame` as bars (or a waveform) with cava's layout options.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::config::theme::{BlendDirection, Rgb, Theme, lerp, to_color};
use crate::config::{Channels, Orientation, VizSettings};
use crate::viz::Frame;

const LOWER: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

pub struct Visualizer<'a> {
    pub frame: Option<&'a Frame>,
    pub settings: &'a VizSettings,
    pub theme: &'a Theme,
    pub truecolor: bool,
}

/// How many bars fit in `width` cells. Stereo always yields an even count.
pub fn bar_count(width: u16, settings: &VizSettings) -> usize {
    let bw = settings.bar_width.max(1) as usize;
    let sp = settings.bar_spacing as usize;
    let n = ((width as usize + sp) / (bw + sp)).max(1);
    match settings.channels {
        Channels::Stereo => (n / 2 * 2).max(2),
        Channels::Mono => n,
    }
}

/// Bar values left to right as displayed: stereo mirrors the channels with the
/// lowest bands meeting in the centre; `reverse` flips the whole thing.
pub fn display_values(frame: &Frame, settings: &VizSettings, total: usize) -> Vec<f32> {
    let mut v: Vec<f32> = match settings.channels {
        Channels::Stereo if !frame.right.is_empty() => {
            let half = total / 2;
            let mut out: Vec<f32> = frame.left.iter().take(half).rev().cloned().collect();
            out.extend(frame.right.iter().take(half).cloned());
            out
        }
        _ => frame.left.iter().take(total).cloned().collect(),
    };
    v.resize(total, 0.0);
    if settings.reverse {
        v.reverse();
    }
    v
}

impl Visualizer<'_> {
    fn colour(&self, row_frac: f32, bar_frac: f32) -> ratatui::style::Color {
        let vertical = self.theme.gradient_at(row_frac);
        let rgb: Rgb = match self.theme.horizontal_gradient_at(bar_frac) {
            None => vertical,
            Some(h) if self.theme.gradient.is_empty() => h,
            Some(h) => {
                let t = match self.theme.blend_direction {
                    BlendDirection::Up => row_frac,
                    BlendDirection::Down => 1.0 - row_frac,
                    BlendDirection::Right => bar_frac,
                    BlendDirection::Left => 1.0 - bar_frac,
                };
                lerp(h, vertical, t)
            }
        };
        to_color(rgb, self.truecolor)
    }

    fn put(&self, buf: &mut Buffer, x: u16, y: u16, sym: &str, row_frac: f32, bar_frac: f32) {
        buf[(x, y)].set_symbol(sym).set_style(Style::default().fg(self.colour(row_frac, bar_frac)));
    }

    /// Draw one bar column `x` of value `v` (0..1) over `rows` rows starting at
    /// `y0`, growing away from the base. `upward` selects lower-block partials.
    #[allow(clippy::too_many_arguments)]
    fn column(&self, buf: &mut Buffer, x: u16, y0: u16, rows: u16, v: f32, upward: bool, bar_frac: f32, idle: bool) {
        if rows == 0 {
            return;
        }
        let eighths_total = rows as u32 * 8;
        let filled = (v.clamp(0.0, 1.0) * eighths_total as f32).round() as u32;
        if filled == 0 {
            if idle {
                let y = if upward { y0 + rows - 1 } else { y0 };
                let sym = if upward { "▁" } else { "▔" };
                self.put(buf, x, y, sym, 0.0, bar_frac);
            }
            return;
        }
        for r in 0..rows {
            let cell_e = filled.saturating_sub(r as u32 * 8).min(8);
            if cell_e == 0 {
                break;
            }
            let row_frac = if rows > 1 { r as f32 / (rows - 1) as f32 } else { 0.0 };
            let y = if upward { y0 + rows - 1 - r } else { y0 + r };
            let sym = if upward {
                LOWER[cell_e as usize]
            } else {
                match cell_e {
                    8 => "█",
                    3..=7 => "▀",
                    _ => "▔",
                }
            };
            self.put(buf, x, y, sym, row_frac, bar_frac);
        }
    }

    fn draw_bars(&self, area: Rect, buf: &mut Buffer, values: &[f32], values_down: Option<&[f32]>) {
        let bw = self.settings.bar_width.max(1);
        let sp = self.settings.bar_spacing;
        let n = values.len();
        let idle = self.settings.show_idle_bar_heads;
        for (i, &v) in values.iter().enumerate() {
            let x0 = area.x + i as u16 * (bw + sp);
            let bar_frac = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
            for dx in 0..bw {
                let x = x0 + dx;
                if x >= area.x + area.width {
                    return;
                }
                match self.settings.orientation {
                    Orientation::Bottom => self.column(buf, x, area.y, area.height, v, true, bar_frac, idle),
                    Orientation::Top => self.column(buf, x, area.y, area.height, v, false, bar_frac, idle),
                    Orientation::Horizontal => {
                        let up_rows = area.height / 2;
                        let down_rows = area.height - up_rows;
                        let down_v = values_down.map(|d| d[i]).unwrap_or(v);
                        self.column(buf, x, area.y, up_rows, v, true, bar_frac, idle);
                        self.column(buf, x, area.y + up_rows, down_rows, down_v, false, bar_frac, false);
                    }
                }
            }
        }
    }

    fn draw_waveform(&self, area: Rect, buf: &mut Buffer, wave: &[f32]) {
        let w = area.width as usize;
        if w == 0 || area.height == 0 {
            return;
        }
        let centre = area.height / 2;
        for x in 0..w {
            let idx = if wave.is_empty() { 0 } else { x * wave.len() / w };
            let v = wave.get(idx).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
            let bar_frac = if w > 1 { x as f32 / (w - 1) as f32 } else { 0.0 };
            let xx = area.x + x as u16;
            if v.abs() < 0.02 {
                if self.settings.show_idle_bar_heads {
                    self.put(buf, xx, area.y + centre, "▁", 0.0, bar_frac);
                }
                continue;
            }
            if v > 0.0 {
                let rows = ((v * centre as f32).round() as u16).clamp(1, centre.max(1));
                for r in 0..rows {
                    if r + 1 > centre {
                        break;
                    }
                    let y = area.y + centre - 1 - r;
                    self.put(buf, xx, y, "█", r as f32 / centre.max(1) as f32, bar_frac);
                }
            } else {
                let down = area.height - centre;
                let rows = ((-v * down as f32).round() as u16).clamp(1, down.max(1));
                for r in 0..rows {
                    let y = area.y + centre + r;
                    if y >= area.y + area.height {
                        break;
                    }
                    self.put(buf, xx, y, "█", r as f32 / down.max(1) as f32, bar_frac);
                }
            }
        }
    }
}

impl Widget for Visualizer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if self.settings.waveform {
            let empty: Vec<f32> = Vec::new();
            let wave = self.frame.map(|f| f.waveform.as_slice()).unwrap_or(&empty);
            self.draw_waveform(area, buf, wave);
            return;
        }
        let total = bar_count(area.width, self.settings);
        let empty = Frame::default();
        let frame = self.frame.unwrap_or(&empty);
        let values = display_values(frame, self.settings, total);
        if self.settings.orientation == Orientation::Horizontal
            && self.settings.horizontal_stereo
            && self.settings.channels == Channels::Stereo
            && !frame.right.is_empty()
        {
            // Left channel above the line, right channel below, both low → high.
            let mut up: Vec<f32> = frame.left.iter().take(total).cloned().collect();
            let mut down: Vec<f32> = frame.right.iter().take(total).cloned().collect();
            up.resize(total, 0.0);
            down.resize(total, 0.0);
            if self.settings.reverse {
                up.reverse();
                down.reverse();
            }
            self.draw_bars(area, buf, &up, Some(&down));
        } else {
            self.draw_bars(area, buf, &values, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(orientation: Orientation, channels: Channels) -> VizSettings {
        VizSettings { orientation, channels, bar_width: 1, bar_spacing: 1, ..VizSettings::default() }
    }

    fn render(frame: &Frame, s: &VizSettings, w: u16, h: u16) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        let theme = Theme::builtin("terminal").unwrap();
        Visualizer { frame: Some(frame), settings: s, theme: &theme, truecolor: true }.render(area, &mut buf);
        buf
    }

    fn sym(buf: &Buffer, x: u16, y: u16) -> &str {
        buf[(x, y)].symbol()
    }

    #[test]
    fn bar_count_accounts_for_width_and_spacing() {
        let s = settings(Orientation::Bottom, Channels::Mono);
        assert_eq!(bar_count(7, &s), 4);
        let wide = VizSettings { bar_width: 2, bar_spacing: 1, ..s };
        assert_eq!(bar_count(8, &wide), 3);
        let stereo = VizSettings { channels: Channels::Stereo, ..wide };
        assert_eq!(bar_count(11, &stereo) % 2, 0);
    }

    #[test]
    fn stereo_mirrors_with_bass_in_centre() {
        let f = Frame { left: vec![0.1, 0.2, 0.3], right: vec![0.4, 0.5, 0.6], waveform: vec![] };
        let s = settings(Orientation::Bottom, Channels::Stereo);
        assert_eq!(display_values(&f, &s, 6), vec![0.3, 0.2, 0.1, 0.4, 0.5, 0.6]);
        let rev = VizSettings { reverse: true, ..s };
        assert_eq!(display_values(&f, &rev, 6), vec![0.6, 0.5, 0.4, 0.1, 0.2, 0.3]);
    }

    #[test]
    fn bottom_bars_grow_upwards_with_partial_top() {
        let f = Frame { left: vec![1.0, 0.5, 0.0, 0.125], right: vec![], waveform: vec![] };
        let s = settings(Orientation::Bottom, Channels::Mono);
        let buf = render(&f, &s, 7, 4);
        assert_eq!((0..4).map(|y| sym(&buf, 0, y)).collect::<Vec<_>>(), vec!["█", "█", "█", "█"]);
        assert_eq!(sym(&buf, 2, 3), "█");
        assert_eq!(sym(&buf, 2, 2), "█");
        assert_eq!(sym(&buf, 2, 1), " ");
        assert_eq!(sym(&buf, 4, 3), "▁");
        assert_eq!(sym(&buf, 6, 3), "▄");
        assert_eq!(sym(&buf, 1, 3), " ");
    }

    #[test]
    fn top_bars_hang_from_the_top() {
        let f = Frame { left: vec![0.5], right: vec![], waveform: vec![] };
        let s = settings(Orientation::Top, Channels::Mono);
        let buf = render(&f, &s, 1, 4);
        assert_eq!(sym(&buf, 0, 0), "█");
        assert_eq!(sym(&buf, 0, 1), "█");
        assert_eq!(sym(&buf, 0, 2), " ");
    }

    #[test]
    fn horizontal_bars_are_symmetric_about_centre() {
        let f = Frame { left: vec![1.0], right: vec![], waveform: vec![] };
        let s = settings(Orientation::Horizontal, Channels::Mono);
        let buf = render(&f, &s, 1, 6);
        assert!((0..6).all(|y| sym(&buf, 0, y) == "█"));
        let half = Frame { left: vec![0.5], right: vec![], waveform: vec![] };
        let buf = render(&half, &s, 1, 6);
        assert_eq!(sym(&buf, 0, 0), " ");
        assert_eq!(sym(&buf, 0, 2), "█");
        assert_eq!(sym(&buf, 0, 3), "█");
        assert_eq!(sym(&buf, 0, 5), " ");
    }

    #[test]
    fn gradient_colours_rows_bottom_to_top() {
        let f = Frame { left: vec![1.0], right: vec![], waveform: vec![] };
        let s = settings(Orientation::Bottom, Channels::Mono);
        let area = Rect::new(0, 0, 1, 2);
        let mut buf = Buffer::empty(area);
        let theme =
            Theme::parse("t", "gradient = 1\ngradient_color_1 = '#000000'\ngradient_color_2 = '#ffffff'\n").unwrap();
        Visualizer { frame: Some(&f), settings: &s, theme: &theme, truecolor: true }.render(area, &mut buf);
        assert_eq!(buf[(0, 1)].fg, ratatui::style::Color::Rgb(0, 0, 0));
        assert_eq!(buf[(0, 0)].fg, ratatui::style::Color::Rgb(255, 255, 255));
    }

    #[test]
    fn waveform_draws_from_centre() {
        let f = Frame { left: vec![], right: vec![], waveform: vec![0.0, 1.0, -1.0, 0.0] };
        let s = VizSettings { waveform: true, ..settings(Orientation::Bottom, Channels::Mono) };
        let buf = render(&f, &s, 4, 5);
        assert_eq!(sym(&buf, 1, 0), "█");
        assert_eq!(sym(&buf, 2, 4), "█");
        assert_eq!(sym(&buf, 0, 2), "▁");
    }

    #[test]
    fn no_frame_draws_idle_heads_only() {
        let s = settings(Orientation::Bottom, Channels::Mono);
        let area = Rect::new(0, 0, 3, 2);
        let mut buf = Buffer::empty(area);
        let theme = Theme::builtin("terminal").unwrap();
        Visualizer { frame: None, settings: &s, theme: &theme, truecolor: true }.render(area, &mut buf);
        assert_eq!(sym(&buf, 0, 1), "▁");
        assert_eq!(sym(&buf, 2, 1), "▁");
        assert_eq!(sym(&buf, 0, 0), " ");
    }
}
