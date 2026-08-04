mod app;
mod config;
mod discovery;
mod pty;
mod terminal;
mod vt;

use anyhow::{Context, Result};
use app::App;
use clap::Parser;
use config::Config;
use std::env;
use std::path::PathBuf;

const BUILTIN_CONFIG: &str = include_str!("../config.default.toml");

#[derive(Debug, Parser)]
#[command(author, version, about = "A dmenu-style TUI workflow launcher")]
struct Args {
    /// Path to a TOML configuration file.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Validate the configuration and exit without opening the TUI.
    #[arg(long)]
    check: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config_path = args.config.unwrap_or_else(default_config_path);
    let config = Config::load_with_defaults(BUILTIN_CONFIG, &config_path)?;

    if args.check {
        let source = if config_path.exists() {
            format!("built-in defaults + {}", config_path.display())
        } else {
            "built-in defaults".to_string()
        };
        println!("configuration is valid: {}", source);
        return Ok(());
    }

    let mut terminal = terminal::Terminal::enter()
        .with_context(|| "could not initialize the launcher terminal")?;
    let mut app = App::new(&config);
    app.run(&mut terminal)
}

fn default_config_path() -> PathBuf {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("tui-launcher/config.toml");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".config/tui-launcher/config.toml");
    }
    PathBuf::from("config.toml")
}
