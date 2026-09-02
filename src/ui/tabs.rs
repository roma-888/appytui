use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::Palette;
use crate::app::App;
use crate::app::views::Tab;

pub fn draw(frame: &mut Frame, app: &App, p: &Palette, area: Rect) {
    let mut spans = vec![Span::styled(" appytui ", Style::default().fg(p.highlight).add_modifier(Modifier::BOLD))];
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let label = format!(" {} {} ", i + 1, tab.title());
        let style = if *tab == app.tab {
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(p.dim)
        };
        spans.push(Span::styled(label, style));
    }
    frame.render_widget(Line::from(spans), area);
}
