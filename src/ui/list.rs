use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Row as TRow, Table, TableState};

use super::Palette;
use crate::app::App;
use crate::app::views::{Drill, Row, Tab};
use crate::music::model::fmt_duration;

pub fn draw(frame: &mut Frame, app: &mut App, p: &Palette, area: Rect) {
    let title = pane_title(app);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(p.dim));
    if app.library.is_none() {
        frame.render_widget(Paragraph::new("Loading library…").block(block), area);
        return;
    }
    if app.tab == Tab::Playlists && !app.playlists_loaded && app.view().drill == Drill::Top {
        frame.render_widget(Paragraph::new("Loading playlists…").block(block), area);
        return;
    }
    let rows = app.rows(app.tab);
    let cursor = app.view().cursor.min(rows.len().saturating_sub(1));
    let current = app.status.track_id.clone();
    let lib = app.library.as_ref().expect("checked above");

    let mixed = app.tab == Tab::Search && app.view().drill == Drill::Top;
    let (header, widths, body): (Vec<&str>, Vec<Constraint>, Vec<TRow>) = match rows.first() {
        // Search results mix kinds, so each row says what it is.
        Some(_) if mixed => (
            vec!["", "Name", "Artist / Album", "Len"],
            vec![
                Constraint::Length(7),
                Constraint::Percentage(45),
                Constraint::Percentage(40),
                Constraint::Length(6),
            ],
            rows.iter()
                .map(|r| match r {
                    Row::Track(i) => {
                        let t = &lib.tracks[*i];
                        let playing = current.as_ref() == Some(&t.id);
                        let mark = if playing { "▶ " } else { "" };
                        let row = TRow::new(vec![
                            "song".to_string(),
                            format!("{mark}{}", t.name),
                            format!("{} — {}", t.artist, t.album),
                            fmt_duration(t.duration_secs),
                        ]);
                        if playing {
                            row.style(Style::default().fg(p.highlight))
                        } else {
                            row
                        }
                    }
                    Row::Album(i) => {
                        let a = &lib.albums[*i];
                        TRow::new(vec![
                            "album".to_string(),
                            a.album.clone(),
                            a.artist.clone(),
                            format!("{:>3} ♪", a.tracks.len()),
                        ])
                    }
                    Row::Artist(i) => {
                        let a = &lib.artists[*i];
                        TRow::new(vec![
                            "artist".to_string(),
                            a.name.clone(),
                            String::new(),
                            format!("{:>3} ♪", a.tracks.len()),
                        ])
                    }
                    Row::Playlist(i) => {
                        let pl = &app.playlists[*i];
                        TRow::new(vec![
                            "list".to_string(),
                            pl.name.clone(),
                            String::new(),
                            format!("{:>3} ♪", pl.track_ids.len()),
                        ])
                    }
                })
                .collect(),
        ),
        Some(Row::Track(_)) | None => (
            vec!["Title", "Artist", "Album", "Len"],
            vec![
                Constraint::Percentage(40),
                Constraint::Percentage(28),
                Constraint::Percentage(24),
                Constraint::Length(6),
            ],
            rows.iter()
                .map(|r| {
                    let Row::Track(i) = r else {
                        return TRow::new(vec![String::new()]);
                    };
                    let t = &lib.tracks[*i];
                    let playing = current.as_ref() == Some(&t.id);
                    let mark = if playing { "▶ " } else { "" };
                    let row = TRow::new(vec![
                        format!("{mark}{}", t.name),
                        t.artist.clone(),
                        t.album.clone(),
                        fmt_duration(t.duration_secs),
                    ]);
                    if playing {
                        row.style(Style::default().fg(p.highlight))
                    } else {
                        row
                    }
                })
                .collect(),
        ),
        Some(Row::Album(_)) => (
            vec!["Album", "Artist", "Tracks"],
            vec![
                Constraint::Percentage(50),
                Constraint::Percentage(40),
                Constraint::Length(7),
            ],
            rows.iter()
                .map(|r| {
                    let Row::Album(i) = r else {
                        return TRow::new(vec![String::new()]);
                    };
                    let a = &lib.albums[*i];
                    TRow::new(vec![
                        a.album.clone(),
                        a.artist.clone(),
                        a.tracks.len().to_string(),
                    ])
                })
                .collect(),
        ),
        Some(Row::Artist(_)) => (
            vec!["Artist", "Tracks"],
            vec![Constraint::Percentage(85), Constraint::Length(7)],
            rows.iter()
                .map(|r| {
                    let Row::Artist(i) = r else {
                        return TRow::new(vec![String::new()]);
                    };
                    let a = &lib.artists[*i];
                    TRow::new(vec![a.name.clone(), a.tracks.len().to_string()])
                })
                .collect(),
        ),
        Some(Row::Playlist(_)) => (
            vec!["Playlist", "Tracks"],
            vec![Constraint::Percentage(85), Constraint::Length(7)],
            rows.iter()
                .map(|r| {
                    let Row::Playlist(i) = r else {
                        return TRow::new(vec![String::new()]);
                    };
                    let pl = &app.playlists[*i];
                    let name = if pl.smart {
                        format!("{} ⚙", pl.name)
                    } else {
                        pl.name.clone()
                    };
                    TRow::new(vec![name, pl.track_ids.len().to_string()])
                })
                .collect(),
        ),
    };

    let table = Table::new(body, widths)
        .header(TRow::new(header).style(Style::default().fg(p.dim).add_modifier(Modifier::BOLD)))
        .block(block)
        .row_highlight_style(
            Style::default()
                .bg(p.selection_bg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state =
        TableState::default().with_selected(if rows.is_empty() { None } else { Some(cursor) });
    frame.render_stateful_widget(table, area, &mut state);

    if app.editing_filter || !app.view().filter.is_empty() {
        let prompt = if app.editing_filter {
            format!("/{}▏", app.view().filter)
        } else {
            format!("/{}", app.view().filter)
        };
        let line_area = Rect::new(
            area.x + 2,
            area.y + area.height.saturating_sub(1),
            area.width.saturating_sub(4),
            1,
        );
        frame.render_widget(
            Line::from(prompt).style(Style::default().fg(p.highlight)),
            line_area,
        );
    }
}

fn pane_title(app: &App) -> String {
    let lib = app.library.as_ref();
    let name = match (app.tab, app.view().drill, lib) {
        (_, Drill::Album(a), Some(l)) => {
            format!("{} — {}", l.albums[a].artist, l.albums[a].album)
        }
        (_, Drill::Artist(a), Some(l)) => l.artists[a].name.clone(),
        (_, Drill::Playlist(pl), _) => app.playlists[pl].name.clone(),
        (Tab::Queue, _, _) if app.status.shuffle => "Queue (shuffle on: order unknown)".to_string(),
        (tab, _, _) => tab.title().to_string(),
    };
    format!(" {name} ")
}
