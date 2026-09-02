//! Minimal flag parsing; no clap to keep the binary small.

use std::path::PathBuf;

pub const USAGE: &str = "appytui — Apple Music TUI

USAGE: appytui [--config PATH] [--theme NAME] [--no-viz] [--no-art] [--version] [--help]

  --config PATH   config file (default ~/.config/appytui/config.toml)
  --theme NAME    override [theme].name for this run
  --no-viz        do not capture audio or draw the visualizer
  --no-art        do not fetch album art
  --version       print the version and exit
";

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub config: Option<PathBuf>,
    pub theme: Option<String>,
    pub no_viz: bool,
    pub no_art: bool,
    pub help: bool,
    pub version: bool,
}

pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args::default();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => {
                out.config = Some(PathBuf::from(args.next().ok_or("--config needs a path")?))
            }
            "--theme" => out.theme = Some(args.next().ok_or("--theme needs a name")?),
            "--no-viz" => out.no_viz = true,
            "--no-art" => out.no_art = true,
            "--help" | "-h" => out.help = true,
            "--version" | "-V" => out.version = true,
            other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &[&str]) -> Result<Args, String> {
        parse(s.iter().map(|x| x.to_string()))
    }

    #[test]
    fn parses_flags() {
        let a = p(&["--theme", "tricolor", "--no-viz"]).unwrap();
        assert_eq!(a.theme.as_deref(), Some("tricolor"));
        assert!(a.no_viz && !a.no_art);
        assert!(p(&["--version"]).unwrap().version);
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["--theme"]).is_err());
    }
}
