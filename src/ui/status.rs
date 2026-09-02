use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::Palette;
use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App, p: &Palette, area: Rect) {
    let line = match &app.message {
        Some((msg, _)) => Line::from(Span::styled(format!(" {msg}"), Style::default().fg(p.highlight))),
        None => Line::from(Span::styled(
            " j/k move  Enter play  Space pause  n/p skip  / filter  v viz  ? help  q quit",
            Style::default().fg(p.dim),
        )),
    };
    frame.render_widget(line, area);
}
