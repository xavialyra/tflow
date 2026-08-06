use crate::config::{Config, Rule, View};
use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::Value;
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
struct DiscoveryItem {
    label: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub prefix: String,
    pub text: String,
    pub value: Option<String>,
    pub metadata: Value,
    pub source_view: String,
}

#[derive(Debug, Default)]
pub struct DiscoveryResult {
    pub items: Vec<Item>,
    pub errors: Vec<String>,
}

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DISCOVERY_STDOUT: usize = 1024 * 1024;
const MAX_DISCOVERY_STDERR: usize = 64 * 1024;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub fn discover(
    config: &Config,
    view_ref: &str,
    rule_name: &str,
    rule: &Rule,
    query: &str,
    source_prefix: Option<&str>,
    log_file: Option<&Path>,
) -> Result<DiscoveryResult> {
    let mut result = DiscoveryResult::default();

    for (source_ref, view) in config.source_views(view_ref)? {
        let display_prefix = display_prefix(&source_ref, view);
        if source_prefix.is_some_and(|prefix| prefix != display_prefix) {
            continue;
        }

        let script = if source_prefix.is_some() {
            view.query_discover.as_ref().or(view.discover.as_ref())
        } else {
            view.default_discover.as_ref().or(view.discover.as_ref())
        };
        let Some(script) = script else {
            continue;
        };

        let shell = view.discover_shell.as_deref().unwrap_or("sh");
        let mut process = Command::new(shell);
        process.args(["-c", script, "tui-launcher"]);
        process.env("LAUNCHER_RULE", rule_name);
        process.env("LAUNCHER_PLUGIN", plugin_name(&source_ref));
        process.env("LAUNCHER_VIEW", view_name(&source_ref));
        process.env("LAUNCHER_VIEW_REF", &source_ref);
        process.env("LAUNCHER_PROVIDER", plugin_name(&source_ref));
        process.env("LAUNCHER_QUERY", query);
        if let Some(log_file) = log_file {
            process.env("LAUNCHER_LOG_FILE", log_file);
        }
        if let Some(root) = config.plugin_root(&source_ref) {
            process.current_dir(root);
            process.env("LAUNCHER_PLUGIN_DIR", root);
        }

        let output = match run_discovery_command(process) {
            Ok(output) => output,
            Err(error) => {
                result.errors.push(format!("{}: {}", source_ref, error));
                continue;
            }
        };

        if !output.status.success() {
            let stderr = sanitize_text(&String::from_utf8_lossy(&output.stderr));
            let detail = if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                stderr
            };
            result.errors.push(format!("{}: {}", source_ref, detail));
        }

        parse_items(
            &mut result,
            &source_ref,
            view,
            &display_prefix,
            rule,
            query,
            &String::from_utf8_lossy(&output.stdout),
        );
    }

    Ok(result)
}

fn run_discovery_command(process: Command) -> Result<std::process::Output> {
    run_bounded_command(
        process,
        DISCOVERY_TIMEOUT,
        MAX_DISCOVERY_STDOUT,
        MAX_DISCOVERY_STDERR,
    )
}

pub(crate) fn run_bounded_command(
    process: Command,
    timeout: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<std::process::Output> {
    run_bounded_command_with_stdin(process, None, timeout, stdout_limit, stderr_limit)
}

pub(crate) fn run_bounded_command_with_stdin(
    mut process: Command,
    stdin: Option<&[u8]>,
    timeout: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<std::process::Output> {
    unsafe {
        process.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    process
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = process
        .spawn()
        .context("could not spawn discovery command")?;
    let stdout_reader = child
        .stdout
        .take()
        .context("discovery command has no stdout pipe")?;
    let stderr_reader = child
        .stderr
        .take()
        .context("discovery command has no stderr pipe")?;
    if let Some(input) = stdin {
        let mut child_stdin = child
            .stdin
            .take()
            .context("discovery command has no stdin pipe")?;
        child_stdin
            .write_all(input)
            .context("could not write discovery command input")?;
    }
    let stdout_exceeded = Arc::new(AtomicBool::new(false));
    let stderr_exceeded = Arc::new(AtomicBool::new(false));
    let stdout_thread = spawn_limited_reader(stdout_reader, stdout_limit, &stdout_exceeded);
    let stderr_thread = spawn_limited_reader(stderr_reader, stderr_limit, &stderr_exceeded);
    let deadline = Instant::now() + timeout;

    let process_result: Result<std::process::ExitStatus> = loop {
        if stdout_exceeded.load(Ordering::Relaxed) || stderr_exceeded.load(Ordering::Relaxed) {
            terminate_child(&mut child);
            break Err(anyhow!("discovery output exceeded configured limits"));
        }

        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => {
                terminate_child(&mut child);
                break Err(anyhow!(error).context("could not inspect discovery command"));
            }
        }

        if Instant::now() >= deadline {
            terminate_child(&mut child);
            break Err(anyhow!("discovery timed out after {:?}", timeout));
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    };

    let stdout = join_reader(stdout_thread, "stdout")?;
    let stderr = join_reader(stderr_thread, "stderr")?;
    if stdout_exceeded.load(Ordering::Relaxed) || stderr_exceeded.load(Ordering::Relaxed) {
        return Err(anyhow!("discovery output exceeded configured limits"));
    }
    let status = process_result?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn spawn_limited_reader<R>(
    reader: R,
    limit: usize,
    exceeded: &Arc<AtomicBool>,
) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    let exceeded = Arc::clone(exceeded);
    thread::spawn(move || read_limited(reader, limit, &exceeded))
}

fn read_limited(mut reader: impl Read, limit: usize, exceeded: &AtomicBool) -> io::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        if count > remaining {
            exceeded.store(true, Ordering::Relaxed);
        }
    }
    Ok(output)
}

fn join_reader(reader: thread::JoinHandle<io::Result<Vec<u8>>>, stream: &str) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| anyhow!("discovery {} reader panicked", stream))?
        .with_context(|| format!("could not read discovery {}", stream))
}

fn terminate_child(child: &mut Child) {
    let process_group = -(child.id() as libc::pid_t);
    if unsafe { libc::kill(process_group, libc::SIGKILL) } != 0 {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn parse_items(
    result: &mut DiscoveryResult,
    source_ref: &str,
    view: &View,
    display_prefix: &str,
    rule: &Rule,
    query: &str,
    stdout: &str,
) {
    for (line_number, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed = match serde_json::from_str::<DiscoveryItem>(line) {
            Ok(item) => item,
            Err(error) => {
                result.errors.push(format!(
                    "{}: invalid discovery JSON on line {}: {}",
                    source_ref,
                    line_number + 1,
                    error
                ));
                continue;
            }
        };
        let text = sanitize_text(&parsed.label);
        if text.is_empty() {
            continue;
        }
        if rule.filter && view.filter && !matches_query(&text, query) {
            continue;
        }
        result.items.push(Item {
            prefix: display_prefix.to_string(),
            text,
            value: parsed.value,
            metadata: parsed.metadata,
            source_view: source_ref.to_string(),
        });
    }
}

fn display_prefix(source_ref: &str, view: &View) -> String {
    view.display_prefix.clone().unwrap_or_else(|| {
        source_ref
            .split_once(':')
            .map(|(_, view_name)| view_name.to_string())
            .unwrap_or_else(|| source_ref.to_string())
    })
}

fn plugin_name(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(plugin, _)| plugin)
        .unwrap_or(view_ref)
}

fn view_name(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(_, view)| view)
        .unwrap_or(view_ref)
}

pub fn matches_query(text: &str, query: &str) -> bool {
    let folded = text.to_lowercase();
    query
        .split_whitespace()
        .all(|token| folded.contains(&token.to_lowercase()))
}

pub fn sanitize_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut escape = false;
    let mut csi = false;
    let mut osc = false;
    let mut osc_escape = false;

    for character in text.chars() {
        if osc {
            if osc_escape {
                osc_escape = false;
                if character == '\\' {
                    osc = false;
                }
            } else if character == '\u{7}' {
                osc = false;
            } else if character == '\u{1b}' {
                osc_escape = true;
            }
            continue;
        }
        if csi {
            if ('@'..='~').contains(&character) {
                csi = false;
            }
            continue;
        }
        if escape {
            escape = false;
            match character {
                '[' => csi = true,
                ']' => osc = true,
                _ => {}
            }
            continue;
        }
        if character == '\u{1b}' {
            escape = true;
            continue;
        }
        if character.is_control() {
            if character == '\t' {
                output.push(' ');
            }
            continue;
        }
        output.push(character);
    }

    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Command, DisplayType, ENGINE_LAUNCHER, EngineDefinition, ViewTypeDefinition,
    };
    use std::collections::BTreeMap;
    use std::io::Cursor;

    fn test_config() -> Config {
        let mut views = BTreeMap::new();
        views.insert(
            "core:default".to_string(),
            View {
                view_type: ENGINE_LAUNCHER.to_string(),
                display: DisplayType::Text,
                sources: vec!["apps:main".to_string()],
                display_prefix: None,
                discover: None,
                default_discover: None,
                query_discover: None,
                discover_shell: None,
                run_shell: None,
                filter: true,
                commands: BTreeMap::new(),
            },
        );
        views.insert(
            "apps:main".to_string(),
            View {
                view_type: ENGINE_LAUNCHER.to_string(),
                display: DisplayType::Text,
                sources: Vec::new(),
                display_prefix: Some("app".to_string()),
                discover: Some("printf '%s\\n' '{\"label\":\"Termius\"}'".to_string()),
                default_discover: None,
                query_discover: None,
                discover_shell: None,
                run_shell: None,
                filter: true,
                commands: BTreeMap::from([(
                    "open".to_string(),
                    Command {
                        key: "enter".to_string(),
                        label: "Open".to_string(),
                        run: None,
                        shell: None,
                        view: None,
                        exit: false,
                    },
                )]),
            },
        );
        Config {
            default_view: "core:default".to_string(),
            dmenu_view: "core:dmenu".to_string(),
            command_view: "core:command".to_string(),
            default_rule: "default".to_string(),
            rules: BTreeMap::from([("default".to_string(), crate::config::Rule { filter: true })]),
            views,
            viewtypes: BTreeMap::from([(
                ENGINE_LAUNCHER.to_string(),
                ViewTypeDefinition {
                    engine: EngineDefinition {
                        engine_type: ENGINE_LAUNCHER.to_string(),
                        config: toml::Table::new(),
                    },
                },
            )]),
            plugin_roots: BTreeMap::new(),
            config_value: Value::Object(serde_json::Map::new()),
        }
    }

    #[test]
    fn matches_all_query_tokens() {
        assert!(matches_query("Restart API service", "api start"));
        assert!(!matches_query("Restart API service", "api database"));
    }

    #[test]
    fn parses_structured_discovery_items() {
        let item: DiscoveryItem = serde_json::from_str(
            r#"{"label":"Termius","value":"termius.desktop","metadata":{"kind":"app"}}"#,
        )
        .unwrap();
        assert_eq!(item.label, "Termius");
        assert_eq!(item.value.as_deref(), Some("termius.desktop"));
        assert_eq!(item.metadata["kind"], "app");
    }

    #[test]
    fn discovered_items_keep_their_source_view() {
        let result = discover(
            &test_config(),
            "core:default",
            "default",
            &crate::config::Rule { filter: true },
            "",
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.items[0].source_view, "apps:main");
        assert_eq!(result.items[0].prefix, "app");
    }

    #[test]
    fn strips_terminal_controls_from_items() {
        assert_eq!(sanitize_text("\u{1b}[31mred\u{1b}[0m\n"), "red");
    }

    #[test]
    fn bounded_reader_keeps_only_the_configured_prefix() {
        let exceeded = AtomicBool::new(false);
        let output = read_limited(Cursor::new(b"abcdef"), 3, &exceeded).unwrap();
        assert_eq!(output, b"abc");
        assert!(exceeded.load(Ordering::Relaxed));
    }

    #[test]
    fn discovery_timeout_terminates_the_process_group() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "sleep 1"]);
        let error = run_bounded_command(command, Duration::from_millis(50), 1024, 1024)
            .expect_err("the discovery command should time out");
        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn discovery_output_limit_returns_an_error() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "printf 123456"]);
        let error = run_bounded_command(command, Duration::from_secs(1), 3, 1024)
            .expect_err("the discovery command should exceed its output limit");
        assert!(error.to_string().contains("output exceeded"));
    }
}
