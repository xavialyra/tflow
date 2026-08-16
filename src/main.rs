mod app;
mod cancellation;
mod chrome;
mod command_runner;
mod config;
mod embedded_terminal;
mod engine;
mod expression;
mod input;
mod invocation;
mod router;
mod runtime_log;
mod state;
mod terminal;
mod text;
mod theme;

use anyhow::{Context, Result, bail};
use app::App;
use clap::Parser;
use config::Config;
use std::env;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "A generic TUI workflow host for View-based CLI plugins"
)]
struct Args {
    /// Path to a TOML configuration file.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Named or builtin theme; overrides the configured theme.
    #[arg(long, value_name = "NAME")]
    theme: Option<String>,

    /// Override one final theme token field for this invocation.
    #[arg(long = "theme-set", value_name = "TOKEN.FIELD=VALUE")]
    theme_set: Vec<crate::theme::ThemeOverride>,

    /// Validate the configuration and exit without opening the TUI.
    #[arg(long)]
    check: bool,

    /// View to start directly; defaults to the configured root view.
    #[arg(value_name = "VIEW")]
    view: Option<String>,

    /// Keyed values validated against the target View query.
    #[arg(
        value_name = "VIEW_OPTION",
        num_args = 0..,
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    view_options: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config_path = args.config.clone().unwrap_or_else(default_config_path);
    let selector = args.theme.map(theme::cli_named_theme);
    let theme_options = theme::ThemeLoadOptions {
        selector,
        overrides: args.theme_set,
    };
    let loaded = Config::load_app(&config_path, &theme_options)?;
    let mut config = loaded.config;
    let theme = loaded.theme;
    let engines = engine::EngineRegistry::new();

    if args.check {
        if args.view.is_some() || !args.view_options.is_empty() {
            bail!("--check cannot be combined with a target View or View options");
        }
        println!("configuration is valid: {}", config_path.display());
        return Ok(());
    }

    let explicit_view = args.view.is_some();
    let root_view = match args.view {
        Some(selector) => config.resolve_view(&selector)?,
        None => config
            .default_view
            .clone()
            .context("no default_view configured; specify a View on the command line")?,
    };
    let state = config.bind_invocation_state(&root_view, &args.view_options)?;
    let input = invocation::InputArtifact::capture()?;
    config.set_invocation(input.value(), state);

    let runtime_log =
        runtime_log::RuntimeLog::open().context("could not initialize the launcher runtime log")?;
    let stdout_is_tty = unsafe { libc::isatty(io::stdout().as_raw_fd()) } == 1;
    let tty = if input.is_tty() && stdout_is_tty {
        None
    } else {
        Some(
            OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .context("could not open /dev/tty for launcher interaction")?,
        )
    };
    let mut terminal = match &tty {
        Some(tty) => terminal::Terminal::enter_with_fds(tty.as_raw_fd(), tty.as_raw_fd()),
        None => terminal::Terminal::enter(),
    }
    .context("could not initialize the launcher terminal")?;
    let mut app = if explicit_view {
        App::with_view(&config, &theme, runtime_log, engines, &root_view)?
    } else {
        App::with_runtime_log_and_engines(&config, &theme, runtime_log, engines)?
    };
    let outcome = app.run(&mut terminal);
    let leave_result = terminal.leave();
    let outcome = outcome.and_then(|outcome| {
        leave_result?;
        Ok(outcome)
    })?;
    let result = invocation::finish(&config, &root_view, outcome)?;

    let mut stderr = io::stderr().lock();
    stderr
        .write_all(&result.stderr)
        .context("could not write invocation stderr")?;
    stderr
        .flush()
        .context("could not flush invocation stderr")?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&result.stdout)
        .context("could not write invocation output")?;
    stdout
        .flush()
        .context("could not flush invocation output")?;
    let exit_code = result.exit_code;
    drop(input);
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
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
