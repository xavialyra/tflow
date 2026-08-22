use super::{App, InputArtifact, InvocationResult, finish};
use crate::config::Config;
use crate::diagnostics::RuntimeLog;
use crate::engine::EngineRegistry;
use crate::lifecycle::SignalGuard;
use crate::session::SessionOutcome;
use crate::terminal::Terminal;
use crate::theme::{self, ThemeLoadOptions};
use anyhow::{Context, Result, bail};
use clap::Parser;
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

pub(crate) fn run() -> Result<i32> {
    let args = Args::parse();
    let config_path = args.config.clone().unwrap_or_else(default_config_path);
    let selector = args.theme.map(theme::cli_named_theme);
    let theme_options = ThemeLoadOptions { selector };
    let loaded = Config::load_app(&config_path, &theme_options)?;
    let mut config = loaded.config;
    let image_protocol = config.image_protocol;
    let theme = loaded.theme;
    let engines = EngineRegistry::new();

    if args.check {
        if args.view.is_some() || !args.view_options.is_empty() {
            bail!("--check cannot be combined with a target View or View options");
        }
        println!("configuration is valid: {}", config_path.display());
        return Ok(0);
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
    let input = InputArtifact::capture()?;
    config.set_invocation(input.value(), state);

    let runtime_log = RuntimeLog::open(config.log_file.as_deref());
    let signal_guard =
        SignalGuard::install().context("could not install launcher signal handlers")?;
    let cancellation = signal_guard.cancellation_token();
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .context("could not open /dev/tty for launcher interaction")?;
    let mut terminal = match Terminal::enter_with_fds_and_cancellation(
        tty.as_raw_fd(),
        tty.as_raw_fd(),
        image_protocol,
        cancellation.clone(),
    ) {
        Ok(terminal) => terminal,
        Err(_error) if signal_guard.received().is_some() => {
            let signal = signal_guard.received().unwrap_or(0);
            drop(input);
            return Ok(128 + signal);
        }
        Err(error) => return Err(error).context("could not initialize the launcher terminal"),
    };
    let app_result = if explicit_view {
        App::with_view(
            &config,
            &theme,
            runtime_log,
            engines,
            &root_view,
            &cancellation,
        )
    } else {
        App::with_runtime_log_and_engines(&config, &theme, runtime_log, engines, &cancellation)
    };
    let mut app = match app_result {
        Ok(app) => app,
        Err(_error) if signal_guard.received().is_some() => {
            let _ = terminal.leave();
            drop(terminal);
            let signal = signal_guard.received().unwrap_or(0);
            drop(input);
            return Ok(if signal != 0 { 128 + signal } else { 1 });
        }
        Err(error) => return Err(error),
    };

    let outcome = app.run(&mut terminal);
    let runtime_warning = app.take_runtime_warning();
    let leave_result = terminal.leave();
    drop(terminal);
    drop(app);
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(_error) if signal_guard.received().is_some() => SessionOutcome::Exited,
        Err(error) => return Err(error),
    };
    if let Err(error) = leave_result
        && signal_guard.received().is_none()
    {
        return Err(error);
    }
    let mut result = match finish(&config, &root_view, outcome, &cancellation) {
        Ok(result) => result,
        Err(_error) if signal_guard.received().is_some() => InvocationResult {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
        },
        Err(error) => return Err(error),
    };
    if let Some(warning) = runtime_warning {
        result
            .stderr
            .extend_from_slice(format!("Error: {warning}\n").as_bytes());
    }
    drop(input);
    signal_guard.enter_final_output();
    let signal = signal_guard.received().unwrap_or(0);
    let exit_code = if signal != 0 {
        result.stdout.clear();
        result.stderr.clear();
        128 + signal
    } else {
        result.exit_code
    };

    if signal == 0 {
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
    }
    Ok(exit_code)
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
