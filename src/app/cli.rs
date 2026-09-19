use super::SessionOutcome;
use super::{App, InputArtifact, InvocationResult, LoadedApp, finish};
use crate::diagnostics::RuntimeLog;
use crate::engine::EngineRegistry;
use crate::lifecycle::SignalGuard;
use crate::terminal::{ImageProtocol as TerminalImageProtocol, Terminal};
use crate::ui::theme::{self, ThemeLoadOptions};
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
    /// Path to a passive host settings file.
    #[arg(short = 'c', long = "settings", alias = "config")]
    settings: Option<PathBuf>,

    /// Path to a workflow file or directory to run (or - for stdin).
    #[arg(short = 'w', long = "workflow", conflicts_with = "suite")]
    workflow: Option<PathBuf>,

    /// Path to a suite manifest to mount and run.
    #[arg(short = 's', long = "suite", conflicts_with = "workflow")]
    suite: Option<PathBuf>,

    /// Named or builtin theme; overrides the configured theme.
    #[arg(long, value_name = "NAME")]
    theme: Option<String>,

    /// Validate the configuration and exit without opening the TUI.
    #[arg(long)]
    check: bool,

    /// Output the view contract (query schema, commands, engine) in JSON and exit.
    #[arg(long, value_name = "VIEW")]
    inspect: Option<String>,

    /// Inspect every configured View; use as `tlaunch inspect --all`.
    #[arg(long)]
    all: bool,

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
        if arg == "--settings"
            || arg == "--config"
            || arg == "-c"
            || arg == "--workflow"
            || arg == "-w"
            || arg == "--suite"
            || arg == "-s"
            || arg == "--theme"
        {
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
    pub(crate) fn load_suite_app(
        suite_path: &Path,
        settings_path: Option<&Path>,
        options: &ThemeLoadOptions,
    ) -> Result<LoadedApp> {
        let engines = EngineRegistry::new();
        let loaded = Self::load_suite_unvalidated(suite_path, settings_path)?;
        let theme_base = loaded.settings_file.as_deref().unwrap_or(suite_path);
        let mut theme = theme::load(theme_base, loaded.theme_selector(), options)?;
        let suite_styles = loaded.suite_styles().clone();
        let settings_styles = loaded.settings_styles().clone();
        let config = loaded.compile()?;
        config.validate_with_engines(&engines)?;
        theme.register_all_workflow_defaults_with_overrides(
            config.workflows(),
            &suite_styles,
            &settings_styles,
        )?;
        Ok(LoadedApp {
            config: std::sync::Arc::new(config),
            theme,
        })
    }

    pub(crate) fn load_workflow_app(
        workflow_path: &Path,
        settings_path: Option<&Path>,
        options: &ThemeLoadOptions,
    ) -> Result<LoadedApp> {
        let engines = EngineRegistry::new();
        let loaded = Self::load_workflow_unvalidated(workflow_path, settings_path)?;
        let theme_base = loaded.settings_file.as_deref().unwrap_or(workflow_path);
        let mut theme = theme::load(theme_base, loaded.theme_selector(), options)?;
        let suite_styles = loaded.suite_styles().clone();
        let settings_styles = loaded.settings_styles().clone();
        let config = loaded.compile()?;
        config.validate_with_engines(&engines)?;
        theme.register_all_workflow_defaults_with_overrides(
            config.workflows(),
            &suite_styles,
            &settings_styles,
        )?;
        Ok(LoadedApp {
            config: std::sync::Arc::new(config),
            theme,
        })
    }

    pub(crate) fn load_workflow_from_str(
        source: &str,
        workflow_id: &str,
        settings_path: Option<&Path>,
        options: &ThemeLoadOptions,
    ) -> Result<LoadedApp> {
        let engines = EngineRegistry::new();
        let loaded = Self::load_workflow_from_str_unvalidated(source, workflow_id, settings_path)?;
        let fallback_path = loaded
            .settings_file
            .as_deref()
            .unwrap_or_else(|| Path::new("."));
        let mut theme = theme::load(fallback_path, loaded.theme_selector(), options)?;
        let suite_styles = loaded.suite_styles().clone();
        let settings_styles = loaded.settings_styles().clone();
        let config = loaded.compile()?;
        config.validate_with_engines(&engines)?;
        theme.register_all_workflow_defaults_with_overrides(
            config.workflows(),
            &suite_styles,
            &settings_styles,
        )?;
        Ok(LoadedApp {
            config: std::sync::Arc::new(config),
            theme,
        })
    }
}

pub(crate) fn run() -> Result<i32> {
    let cli_args = effective_cli_args();
    let args = Args::parse_from(cli_args);
    let settings_path = args.settings.as_deref();
    let selector = args.theme.map(theme::cli_named_theme);
    let theme_options = ThemeLoadOptions { selector };

    let is_stdin_workflow = args.workflow.as_ref().is_some_and(|p| p == Path::new("-"));
    let mut streamed_stdin_source = None;
    if is_stdin_workflow {
        use std::io::Read;
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .context("could not read workflow from stdin")?;
        streamed_stdin_source = Some(source);
    }

    let (loaded, target_display) = if let Some(source) = streamed_stdin_source {
        let app =
            CompiledConfig::load_workflow_from_str(&source, "main", settings_path, &theme_options)?;
        (app, "<stdin>".to_string())
    } else if let Some(workflow_path) = &args.workflow {
        let app = CompiledConfig::load_workflow_app(workflow_path, settings_path, &theme_options)?;
        let display = workflow_path.display().to_string();
        (app, display)
    } else if let Some(suite_path) = &args.suite {
        let app = CompiledConfig::load_suite_app(suite_path, settings_path, &theme_options)?;
        let display = suite_path.display().to_string();
        (app, display)
    } else {
        let default_suite = default_suite_path()?;
        let app = CompiledConfig::load_suite_app(&default_suite, settings_path, &theme_options)?;
        let display = default_suite.display().to_string();
        (app, display)
    };
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
        if args.inspect.is_some() || args.all {
            bail!("--check cannot be combined with inspection options");
        }
        println!("configuration is valid: {target_display}");
        return Ok(0);
    }

    let inspect_target = if let Some(target) = &args.inspect {
        if args.all {
            bail!("--all cannot be combined with --inspect <VIEW>");
        }
        Some(target.as_str())
    } else if args.view.as_deref() == Some("inspect") {
        if args.all {
            if !args.view_options.is_empty() {
                bail!("inspect --all cannot be combined with a View argument");
            }
            let views = config
                .iter_public_views()
                .map(|(view_ref, view)| view_contract(&config, view_ref, view))
                .collect::<Vec<_>>();
            let output = serde_json::json!({ "views": views });
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(0);
        }
        let target = args
            .view_options
            .first()
            .context("inspect requires a view argument, e.g. `tlaunch inspect <view>`")?;
        Some(target.as_str())
    } else {
        if args.all {
            bail!("--all requires `inspect`; use `tlaunch inspect --all`");
        }
        None
    };

    if let Some(target) = inspect_target {
        let view_ref = config.resolve_view(target)?;
        let view = config.view(&view_ref).context("view disappeared")?;
        println!(
            "{}",
            serde_json::to_string_pretty(&view_contract(&config, &view_ref, view))?
        );
        return Ok(0);
    }

    let explicit_view = args.view.is_some();
    let root_view = if let Some(target) = args.view.as_deref() {
        config.resolve_view(target)?
    } else {
        config.entrypoint.clone()
    };
    let mut parameters = config.bind_invocation_parameters(&root_view, &args.view_options)?;
    config.sanitize_initial_parameter_values(&mut parameters)?;
    let input = if is_stdin_workflow {
        InputArtifact::empty()
    } else {
        InputArtifact::capture()?
    };
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

fn view_contract(
    config: &CompiledConfig,
    view_ref: &str,
    view: &crate::workflow::config::View,
) -> serde_json::Value {
    serde_json::json!({
        "view": view_ref,
        "alias": config.alias_for_view(view_ref),
        "engine": view.selected_engine_type(),
        "query": view.query,
        "commands": view.commands.iter().map(|(id, cmd)| {
            serde_json::json!({
                "id": id,
                "key": cmd.key,
                "label": cmd.label,
                "scope": "view",
            })
        }).collect::<Vec<_>>(),
    })
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

fn default_suite_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("TLAUNCH_SUITE") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("tlaunch/default.toml"));
    }
    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".config/tlaunch/default.toml"));
    }
    bail!("cannot locate default suite: set XDG_CONFIG_HOME or use -s <PATH>");
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
