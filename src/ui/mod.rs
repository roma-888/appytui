//! ratatui rendering of `App`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Color;

use crate::app::App;
use crate::config::theme::to_color;

pub mod help;
pub mod list;
pub mod now_playing;
pub mod status;
pub mod tabs;
pub mod visualizer;

/// Theme colours converted for this terminal.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub accent: Color,
    pub highlight: Color,
    pub selection_bg: Color,
    pub dim: Color,
}

impl Palette {
    pub fn from(app: &App) -> Palette {
        let t = &app.theme;
        Palette {
            accent: to_color(t.accent, app.truecolor),
            highlight: to_color(t.highlight, app.truecolor),
            selection_bg: to_color(t.selection_bg, app.truecolor),
            dim: to_color(t.dim, app.truecolor),
        }
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let palette = Palette::from(app);
    let [top, middle, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(middle);

    tabs::draw(frame, app, &palette, top);
    list::draw(frame, app, &palette, left);
    now_playing::draw(frame, app, &palette, right);
    status::draw(frame, app, &palette, bottom);
    if app.show_help {
        help::draw(frame, &palette, centered(frame.area(), 60, 20));
    }
}

/// A `w` x `h` rect centred in `area`, clamped to it.
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::reducer::{Action, reduce};
    use crate::config::VizSettings;
    use crate::config::theme::Theme;
    use crate::music::Event;
    use crate::music::fake::track;
    use crate::music::model::{PlayerState, PlayerStatus, TrackId};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &mut App) -> String {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_tabs_list_and_now_playing() {
        let mut app = App::new(
            Theme::builtin("catppuccin-mocha").unwrap(),
            VizSettings::default(),
            false,
        );
        let text = render(&mut app);
        assert!(text.contains("Songs"));
        assert!(text.contains("Loading library"));

        reduce(
            &mut app,
            Action::Bridge(Event::Library(vec![track(
                "1",
                "Alpha Song",
                "Ann",
                "Album A",
            )])),
        );
        app.tab = crate::app::views::Tab::Playlists;
        assert!(render(&mut app).contains("Loading playlists"));
        reduce(&mut app, Action::Bridge(Event::Playlists(vec![])));
        assert!(!render(&mut app).contains("Loading playlists"));
        app.tab = crate::app::views::Tab::Songs;
        reduce(
            &mut app,
            Action::Bridge(Event::Status(PlayerStatus {
                state: PlayerState::Playing,
                track_id: Some(TrackId("1".into())),
                track: None,
                position_secs: 30.0,
                volume: 80,
                shuffle: true,
                repeat: crate::music::model::RepeatMode::All,
            })),
        );
        let text = render(&mut app);
        assert!(text.contains("Alpha Song"));
        assert!(text.contains("Ann"));
        // Title is centred in the right-hand pane, not flush against its left border.
        // The list on the left also shows the title; take the rightmost occurrence.
        let col = text
            .lines()
            .filter_map(|l| l.rfind("Alpha Song"))
            .max()
            .unwrap();
        assert!(
            col > 65,
            "title at column {col}, expected centred in the right pane"
        );
        assert!(text.contains("shuffle on"));
        assert!(text.contains("repeat all"));
        assert!(text.contains("vol 80"));
        assert!(text.contains("3:20"));

        app.show_help = true;
        let text = render(&mut app);
        assert!(text.contains("play / pause"));
    }

    fn app_with_art(protocol: ratatui_image::picker::ProtocolType) -> App {
        use ratatui_image::picker::Picker;
        let mut app = App::new(
            Theme::builtin("terminal").unwrap(),
            VizSettings::default(),
            true,
        );
        #[allow(deprecated)]
        // the only constructor that lets a test pin a font size without a terminal
        let mut picker = Picker::from_fontsize((8, 16).into());
        picker.set_protocol_type(protocol);
        app.picker = picker;
        reduce(
            &mut app,
            Action::Bridge(Event::Library(vec![track(
                "1",
                "Alpha Song",
                "Ann",
                "Album A",
            )])),
        );
        reduce(
            &mut app,
            Action::Bridge(Event::Status(PlayerStatus {
                state: PlayerState::Playing,
                track_id: Some(TrackId("1".into())),
                ..PlayerStatus::default()
            })),
        );
        let key = crate::art::cache_key("Ann", "Album A");
        let mut img = image::RgbImage::new(16, 16);
        img.put_pixel(0, 0, image::Rgb([200, 10, 10]));
        reduce(
            &mut app,
            Action::Art(crate::art::ArtResult {
                key,
                image: Some(image::DynamicImage::ImageRgb8(img)),
            }),
        );
        app
    }

    #[test]
    fn kitty_protocol_emits_graphics_escape_sequences() {
        let mut app = app_with_art(ratatui_image::picker::ProtocolType::Kitty);
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let joined: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            joined.contains("\x1b_G"),
            "no Kitty APC sequence in rendered cells"
        );
    }

    #[test]
    fn halfblocks_protocol_draws_block_cells() {
        let mut app = app_with_art(ratatui_image::picker::ProtocolType::Halfblocks);
        let text = render(&mut app);
        assert!(
            text.contains('▄') || text.contains('▀'),
            "no half-block cells rendered"
        );
    }
}
