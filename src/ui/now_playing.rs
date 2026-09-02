use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, LineGauge, Paragraph};

use super::Palette;
use crate::app::App;
use crate::music::model::{PlayerState, fmt_duration};

/// Sub-areas of the now-playing pane that later tasks draw into.
pub struct Areas {
    #[allow(dead_code)] // drawn by Task 12
    pub art: Rect,
    pub info: Rect,
    pub progress: Rect,
    #[allow(dead_code)] // drawn by Task 11
    pub viz: Rect,
    pub flags: Rect,
}

pub fn layout(inner: Rect) -> Areas {
    // Art is square-ish: two pixels per cell vertically, so width ≈ 2 × height.
    let art_h = (inner.width / 2).min(inner.height / 2).min(12);
    let [art, info, progress, viz, flags] = Layout::vertical([
        Constraint::Length(art_h),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(inner);
    Areas { art, info, progress, viz, flags }
}

pub fn draw(frame: &mut Frame, app: &App, p: &Palette, area: Rect) {
    let block =
        Block::default().borders(Borders::ALL).title(" Now Playing ").border_style(Style::default().fg(p.dim));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let areas = layout(inner);

    let Some(track) = app.current_track() else {
        frame.render_widget(Paragraph::new("Nothing playing").style(Style::default().fg(p.dim)), areas.info);
        return;
    };

    let icon = match app.status.state {
        PlayerState::Playing => "▶",
        PlayerState::Paused => "⏸",
        PlayerState::Stopped => "■",
    };
    let info = vec![
        Line::from(Span::styled(track.name.clone(), Style::default().fg(p.highlight).add_modifier(Modifier::BOLD))),
        Line::from(track.artist.clone()),
        Line::from(Span::styled(track.album.clone(), Style::default().fg(p.dim))),
    ];
    frame.render_widget(Paragraph::new(info), areas.info);

    let pos = app.position_now();
    let ratio = if track.duration_secs > 0.0 { (pos / track.duration_secs).clamp(0.0, 1.0) } else { 0.0 };
    let gauge = LineGauge::default()
        .ratio(ratio)
        .filled_style(Style::default().fg(p.accent))
        .unfilled_style(Style::default().fg(p.dim))
        .label(format!("{icon} {} / {}", fmt_duration(pos), fmt_duration(track.duration_secs)));
    frame.render_widget(gauge, areas.progress);

    let flags = format!(
        "shuffle {} · repeat {} · vol {}",
        if app.status.shuffle { "on" } else { "off" },
        app.status.repeat.as_str(),
        app.status.volume
    );
    frame.render_widget(Line::from(flags).style(Style::default().fg(p.dim)), areas.flags);
}
