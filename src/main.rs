mod app;
mod cancellation;
mod command_runner;
mod config;
mod dmenu;
mod engine;
mod expression;
mod input;
mod runtime_log;
mod terminal;
mod text;
mod vt;

use anyhow::{Context, Result, bail};
use app::App;
use clap::Parser;
use config::Config;
use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(author, version, about = "A dmenu-style TUI workflow launcher")]
struct Args {
    /// Path to a TOML configuration file.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Validate the configuration and exit without opening the TUI.
    #[arg(long)]
    check: bool,

    /// Read newline-delimited candidates from stdin and select one.
    #[arg(short = 'd', long)]
    dmenu: bool,

    /// Read NUL-delimited candidates from stdin and select one.
    #[arg(long)]
    dmenu0: bool,

    /// Prompt shown before the dmenu query.
    #[arg(long, default_value = "> ")]
    prompt: String,

    /// Limit the number of visible dmenu result rows.
    #[arg(long)]
    lines: Option<usize>,

    /// Initial dmenu query.
    #[arg(long, default_value = "")]
    initial: String,

    /// Print the selected zero-based input index instead of its text.
    #[arg(long)]
    index: bool,

    /// Change the displayed fields or format.
    #[arg(long, value_name = "N|FMT")]
    with_nth: Option<String>,

    /// Change the output fields or format.
    #[arg(long, value_name = "N|FMT")]
    accept_nth: Option<String>,

    /// Change the fields used for matching.
    #[arg(long, value_name = "N|FMT")]
    match_nth: Option<String>,

    /// Single ASCII field delimiter or whitespace mode; defaults to tab.
    #[arg(long, value_name = "CHARACTER")]
    nth_delimiter: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let invoked_as_dmenu = invoked_as_dmenu();

    if args.dmenu || args.dmenu0 || invoked_as_dmenu {
        if args.check {
            bail!("dmenu mode cannot be combined with --check");
        }
        let config_path = args.config.unwrap_or_else(default_config_path);
        let config = Config::load(&config_path)?;
        let display = config.dmenu_view()?.display;
        let outcome = dmenu::run(dmenu::Options {
            prompt: args.prompt,
            lines: args.lines,
            initial: args.initial,
            index: args.index,
            dmenu0: args.dmenu0,
            display,
            with_nth: args.with_nth,
            accept_nth: args.accept_nth,
            match_nth: args.match_nth,
            nth_delimiter: args.nth_delimiter,
        })?;
        return match outcome {
            dmenu::Outcome::Selected { value, terminator } => {
                let mut stdout = io::stdout().lock();
                stdout
                    .write_all(&value)
                    .context("could not write selected dmenu value")?;
                stdout
                    .write_all(&[terminator])
                    .context("could not terminate selected dmenu value")?;
                stdout
                    .flush()
                    .context("could not flush selected dmenu value")?;
                Ok(())
            }
            dmenu::Outcome::Cancelled => std::process::exit(1),
        };
    }

    let config_path = args.config.unwrap_or_else(default_config_path);
    let engines = engine::EngineRegistry::new();
    let config = Config::load_with_engines(&config_path, &engines)?;

    if args.check {
        println!("configuration is valid: {}", config_path.display());
        return Ok(());
    }

    let runtime_log =
        runtime_log::RuntimeLog::open().context("could not initialize the launcher runtime log")?;
    let mut terminal = terminal::Terminal::enter()
        .with_context(|| "could not initialize the launcher terminal")?;
    let mut app = App::with_runtime_log_and_engines(&config, runtime_log, engines)?;
    app.run(&mut terminal)
}

fn invoked_as_dmenu() -> bool {
    std::env::args_os().next().is_some_and(|argument| {
        Path::new(&argument)
            .file_name()
            .is_some_and(|name| name == "dmenu")
    })
}

fn default_config_path() -> PathBuf {
    if let Some(path) = env::var_os("TUI_LAUNCHER_CONFIG") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("tui-launcher/config.toml");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".config/tui-launcher/config.toml");
    }
    PathBuf::from("config.toml")
}
