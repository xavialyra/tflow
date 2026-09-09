use super::SessionOutcome;
use super::{App, InputArtifact, InvocationResult, LoadedApp, finish};
use crate::diagnostics::RuntimeLog;
use crate::engine::EngineRegistry;
use crate::lifecycle::SignalGuard;
use crate::terminal::{ImageProtocol as TerminalImageProtocol, Terminal};
use crate::ui::theme::{self, ThemeLoadOptions};
#[cfg(test)]
use crate::workflow::config::EngineConfigValidator;
use crate::workflow::config::{CompiledConfig, ImageProtocol as ConfigImageProtocol};
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::env;
use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "A generic TUI workflow host for View-based CLI workflows"
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

    /// Output the view contract (query schema, commands, engine) in JSON and exit.
    #[arg(long, value_name = "VIEW")]
    inspect: Option<String>,

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

fn effective_cli_args() -> Vec<String> {
    effective_cli_args_from(env::args().collect())
}

fn effective_cli_args_from(args: Vec<String>) -> Vec<String> {
    if args.is_empty() {
        return args;
    }
    let stem = Path::new(&args[0])
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    if stem.is_empty() || stem == "tlaunch" {
        return args;
    }
    if args.iter().any(|a| a == "--check" || a == "--inspect") {
        return args;
    }

    let mut global_prefix = Vec::new();
    let mut remainder = Vec::new();
    let mut iter = args.into_iter();
    let exe = iter.next().unwrap();

    while let Some(arg) = iter.next() {
        if arg == "--config" || arg == "-c" || arg == "--theme" {
            global_prefix.push(arg);
            if let Some(val) = iter.next() {
                global_prefix.push(val);
            }
        } else {
            remainder.push(arg);
            remainder.extend(iter);
            break;
        }
    }

    let mut new_args = vec![exe];
    new_args.extend(global_prefix);
    new_args.push(stem);
    new_args.extend(remainder);
    new_args
}

impl CompiledConfig {
    #[cfg(test)]
    pub(crate) fn load(user_path: &Path) -> Result<Self> {
        let engines = EngineRegistry::new();
        Self::load_with_engines(user_path, &engines)
    }

    pub(crate) fn load_app(user_path: &Path, options: &ThemeLoadOptions) -> Result<LoadedApp> {
        let engines = EngineRegistry::new();
        let loaded = Self::load_unvalidated(user_path)?;
        let mut theme = theme::load(user_path, loaded.theme_selector(), options)?;
        let config = loaded.compile()?;
        config.validate_with_engines(&engines)?;
        theme.register_all_workflow_defaults(config.workflows())?;
        Ok(LoadedApp {
            config: std::sync::Arc::new(config),
            theme,
        })
    }

    #[cfg(test)]
    pub(crate) fn load_with_engines<V>(user_path: &Path, engines: &V) -> Result<Self>
    where
        V: EngineConfigValidator,
    {
        let loaded = Self::load_unvalidated(user_path)?;
        let mut theme = theme::load(
            user_path,
            loaded.theme_selector(),
            &ThemeLoadOptions::default(),
        )?;
        let config = loaded.compile()?;
        theme.register_all_workflow_defaults(config.workflows())?;
        config.validate_with_engines(engines)?;
        Ok(config)
    }
}

pub(crate) fn run() -> Result<i32> {
    let cli_args = effective_cli_args();
    let args = Args::parse_from(cli_args);
    let config_path = args.config.clone().unwrap_or_else(default_config_path);
    let selector = args.theme.map(theme::cli_named_theme);
    let theme_options = ThemeLoadOptions { selector };
    let loaded = CompiledConfig::load_app(&config_path, &theme_options)?;
    let config = loaded.config;
    let image_protocol = match config.image_protocol {
        ConfigImageProtocol::Halfblocks => TerminalImageProtocol::Halfblocks,
        ConfigImageProtocol::Kitty => TerminalImageProtocol::Kitty,
        ConfigImageProtocol::Sixel => TerminalImageProtocol::Sixel,
        ConfigImageProtocol::Iterm2 => TerminalImageProtocol::Iterm2,
    };
    let theme = loaded.theme;
    let engines = EngineRegistry::new();

    if args.check {
        if args.view.is_some() || !args.view_options.is_empty() {
            bail!("--check cannot be combined with a target View or View options");
        }
        println!("configuration is valid: {}", config_path.display());
        return Ok(0);
    }

    let inspect_target = if let Some(target) = &args.inspect {
        Some(target.as_str())
    } else if args.view.as_deref() == Some("inspect") {
        let target = args
            .view_options
            .first()
            .context("inspect requires a view argument, e.g. `tlaunch inspect <view>`")?;
        Some(target.as_str())
    } else {
        None
    };

    if let Some(target) = inspect_target {
        let view_ref = config.resolve_view(target)?;
        let view = config.view(&view_ref).context("view disappeared")?;
        let output = serde_json::json!({
            "view": view_ref,
            "alias": view.alias,
            "engine": view.selected_engine_type(),
            "query": view.query,
            "commands": view.commands.iter().map(|(id, cmd)| {
                serde_json::json!({
                    "id": id,
                    "key": cmd.key,
                    "label": cmd.label,
                    "scope": cmd.scope,
                    "requires": cmd.requires,
                })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
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
    let mut parameters = config.bind_invocation_parameters(&root_view, &args.view_options)?;
    config.sanitize_initial_parameter_values(&mut parameters)?;
    let input = InputArtifact::capture()?;
    let invocation = std::sync::Arc::new(crate::workflow::InvocationContext::new(
        root_view.clone(),
        input.value(),
        parameters,
    )?);

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
            std::sync::Arc::clone(&config),
            std::sync::Arc::clone(&invocation),
            &theme,
            runtime_log,
            engines,
            &root_view,
            &cancellation,
        )
    } else {
        App::with_runtime_log_and_engines(
            std::sync::Arc::clone(&config),
            std::sync::Arc::clone(&invocation),
            &theme,
            runtime_log,
            engines,
            &cancellation,
        )
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
    let mut result = match finish(&config, &invocation, outcome, &cancellation) {
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
    if let Some(signal) = signal_guard.received() {
        return Ok(128 + signal);
    }

    let stderr = io::stderr().lock();
    if !write_final_output(stderr.as_raw_fd(), &result.stderr, &signal_guard)
        .context("could not write invocation stderr")?
    {
        return Ok(128 + signal_guard.received().unwrap_or(libc::SIGTERM));
    }
    if let Some(signal) = signal_guard.received() {
        return Ok(128 + signal);
    }
    drop(stderr);

    let stdout = io::stdout().lock();
    if !write_final_output(stdout.as_raw_fd(), &result.stdout, &signal_guard)
        .context("could not write invocation output")?
    {
        return Ok(128 + signal_guard.received().unwrap_or(libc::SIGTERM));
    }
    if let Some(signal) = signal_guard.received() {
        return Ok(128 + signal);
    }
    Ok(result.exit_code)
}

fn write_final_output(
    fd: libc::c_int,
    bytes: &[u8],
    signal_guard: &SignalGuard,
) -> io::Result<bool> {
    if bytes.is_empty() {
        return Ok(true);
    }
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut offset = 0;
    let write_result = loop {
        if signal_guard.received().is_some() {
            break Ok(false);
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let poll_result = unsafe { libc::poll(&mut poll, 1, 50) };
        if poll_result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break Err(error);
        }
        if poll_result == 0 {
            continue;
        }
        if poll.revents & libc::POLLNVAL != 0 {
            break Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        if poll.revents & (libc::POLLOUT | libc::POLLERR | libc::POLLHUP) == 0 {
            continue;
        }

        let count =
            unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        if count > 0 {
            offset += count as usize;
            if offset == bytes.len() {
                break Ok(signal_guard.received().is_none());
            }
            continue;
        }
        if count == 0 {
            break Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "final output write returned zero bytes",
            ));
        }
        let error = io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(code)
                if code == libc::EINTR
                    || code == libc::EAGAIN
                    || code == libc::EWOULDBLOCK
        ) {
            continue;
        }
        break Err(error);
    };

    let restore_result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    if restore_result < 0 {
        return Err(io::Error::last_os_error());
    }
    write_result
}

fn default_config_path() -> PathBuf {
    if let Some(path) = env::var_os("TLAUNCH_CONFIG") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("tlaunch/config.toml");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".config/tlaunch/config.toml");
    }
    PathBuf::from("config.toml")
}

#[cfg(test)]
mod tests {
    use super::effective_cli_args_from;

    #[test]
    fn injects_the_entrypoint_stem_for_all_noncanonical_entrypoints() {
        for stem in ["apps", "tlaunch-app", "launcher-app"] {
            assert_eq!(
                effective_cli_args_from(vec![format!("/tmp/{stem}"), "--mode=full".into()]),
                vec![
                    format!("/tmp/{stem}"),
                    stem.to_string(),
                    "--mode=full".into()
                ]
            );
        }
        assert_eq!(
            effective_cli_args_from(vec!["/tmp/tlaunch".into(), "apps:main".into()]),
            vec!["/tmp/tlaunch", "apps:main"]
        );
    }
}
