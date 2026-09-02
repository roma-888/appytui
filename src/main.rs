mod app;
mod cli;
mod config;
mod music;
mod ui;
mod viz;

use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::{select, unbounded};
use ratatui::crossterm::event::{self, Event as TermEvent};

use app::App;
use app::reducer::{Action, Effect, reduce};
use config::Config;
use config::theme::Theme;
use music::Command;
use music::jxa::JxaBridge;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(250);

fn main() {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if args.help {
        print!("{}", cli::USAGE);
        return;
    }
    if let Err(e) = run(args) {
        eprintln!("appytui: {e:#}");
        std::process::exit(1);
    }
}

fn run(args: cli::Args) -> Result<()> {
    let config_path = args.config.unwrap_or_else(Config::default_path);
    let config = Config::load(&config_path)?;
    let theme_name = args.theme.unwrap_or(config.theme.name.clone());
    let theme = Theme::load(&theme_name, &Theme::user_dirs())?;
    let mut viz = config.visualizer.clone();
    if args.no_viz {
        viz.enabled = false;
    }
    let art_enabled = config.art.enabled && !args.no_art;

    let bridge = JxaBridge::new();
    bridge.ensure_running().context(
        "cannot control Music.app. Allow automation for your terminal in System Settings > Privacy & Security > Automation",
    )?;

    let (cmd_tx, cmd_rx) = unbounded::<Command>();
    let (ev_tx, ev_rx) = unbounded::<music::Event>();
    let worker = music::worker::spawn(Box::new(bridge), cmd_rx, ev_tx, POLL_INTERVAL);
    cmd_tx.send(Command::LoadLibrary)?;
    cmd_tx.send(Command::LoadPlaylists)?;

    let (key_tx, key_rx) = unbounded::<TermEvent>();
    std::thread::Builder::new()
        .name("input".into())
        .spawn(move || {
            while let Ok(ev) = event::read() {
                if key_tx.send(ev).is_err() {
                    break;
                }
            }
        })
        .context("spawning input thread")?;
    let ticker = crossbeam_channel::tick(TICK);

    let mut app = App::new(theme, viz, art_enabled);
    let mut terminal = ratatui::init();
    terminal.draw(|f| ui::draw(f, &mut app))?;

    let result = (|| -> Result<()> {
        loop {
            let action = select! {
                recv(key_rx) -> ev => match ev? {
                    TermEvent::Key(k) if k.kind != event::KeyEventKind::Release => Action::Key(k),
                    TermEvent::Resize(_, _) => Action::Resize,
                    _ => continue,
                },
                recv(ev_rx) -> ev => Action::Bridge(ev?),
                recv(ticker) -> _ => Action::Tick,
            };
            for effect in reduce(&mut app, action) {
                match effect {
                    Effect::Send(cmd) => cmd_tx.send(cmd)?,
                    Effect::Quit => return Ok(()),
                }
            }
            if app.should_quit {
                return Ok(());
            }
            terminal.draw(|f| ui::draw(f, &mut app))?;
        }
    })();

    ratatui::restore();
    let _ = cmd_tx.send(Command::Shutdown);
    let _ = worker.join();
    result
}
