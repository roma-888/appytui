use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::Palette;

pub const KEYS: &[(&str, &str)] = &[
    ("1-6 / Tab", "switch tab"),
    ("j/k ↓/↑", "move cursor"),
    ("g / G", "top / bottom"),
    ("Ctrl-d / Ctrl-u", "half page"),
    ("Enter", "play selected / open"),
    ("a", "play list, album, artist or playlist from the top"),
    ("e / E", "add to queue / play next"),
    ("d", "remove from queue (Queue tab)"),
    ("Backspace", "back out of album/artist/playlist"),
    ("Space", "play / pause"),
    ("n / p", "next / previous"),
    ("← / →", "seek −5 s / +5 s"),
    ("+ / -", "volume ±5"),
    ("s / r", "toggle shuffle / cycle repeat"),
    ("/", "filter list (Esc clears)"),
    ("v / V / w", "toggle visualizer / orientation / waveform"),
    ("?", "this help"),
    ("q", "quit"),
];

pub fn draw(frame: &mut Frame, p: &Palette, area: Rect) {
    let text: Vec<String> = KEYS.iter().map(|(k, d)| format!(" {k:<16} {d}")).collect();
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text.join("\n")).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Keys ")
                .border_style(Style::default().fg(p.accent)),
        ),
        area,
    );
}
