use crate::config::{ActionMode, Config};
use crate::discovery::{DiscoveryResult, Item, discover, sanitize_text};
use crate::pty::{self, EmbeddedOutcome};
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
const INPUT_POLL_MS: i32 = 80;

struct DiscoveryRequest {
    id: u64,
    input: String,
}

struct DiscoveryResponse {
    id: u64,
    input: String,
    active_rule: String,
    query: String,
    result: std::result::Result<DiscoveryResult, String>,
}

pub struct App<'a> {
    config: &'a Config,
    input: String,
    items: Vec<Item>,
    selected: usize,
    active_rule: String,
    query: String,
    errors: Vec<String>,
    message: String,
    decoder: InputDecoder,
    refresh_deadline: Option<Instant>,
    discovery_tx: Sender<DiscoveryRequest>,
    discovery_rx: Receiver<DiscoveryResponse>,
    next_request_id: u64,
    latest_request_id: u64,
    requested_input: String,
    results_input: String,
    discovery_pending: bool,
    execute_after_discovery: bool,
}

impl<'a> App<'a> {
    pub fn new(config: &'a Config) -> Self {
        let (discovery_tx, request_rx) = mpsc::channel();
        let (response_tx, discovery_rx) = mpsc::channel();
        let worker_config = config.clone();
        thread::spawn(move || discovery_worker(worker_config, request_rx, response_tx));

        Self {
            config,
            input: String::new(),
            items: Vec::new(),
            selected: 0,
            active_rule: config.default_rule.clone(),
            query: String::new(),
            errors: Vec::new(),
            message: String::new(),
            decoder: InputDecoder::default(),
            refresh_deadline: None,
            discovery_tx,
            discovery_rx,
            next_request_id: 0,
            latest_request_id: 0,
            requested_input: String::new(),
            results_input: String::new(),
            discovery_pending: false,
            execute_after_discovery: false,
        }
    }

    pub fn run(&mut self, terminal: &mut Terminal) -> Result<()> {
        self.request_discovery()?;

        loop {
            if self.collect_discoveries(terminal)? {
                return Ok(());
            }
            self.refresh_if_due()?;
            self.render(terminal)?;
            let timeout = self.input_timeout_ms();
            let mut keys = self.decoder.feed(&terminal.read_input(timeout)?);
            keys.extend(self.decoder.flush_due());

            let mut refresh = false;
            for key in keys {
                match self.handle_key(key, terminal)? {
                    CommandResult::Continue => {}
                    CommandResult::Refresh => refresh = true,
                    CommandResult::Exit => return Ok(()),
                }
            }
            if refresh {
                self.schedule_refresh();
            }
            self.refresh_if_due()?;
        }
    }

    fn handle_key(&mut self, key: Key, terminal: &mut Terminal) -> Result<CommandResult> {
        match key {
            Key::CtrlC | Key::CtrlD => Ok(CommandResult::Exit),
            Key::Escape => {
                if self.input.is_empty() {
                    self.refresh_deadline = None;
                    self.execute_after_discovery = false;
                    Ok(CommandResult::Exit)
                } else {
                    self.input.clear();
                    Ok(CommandResult::Refresh)
                }
            }
            Key::Enter => {
                if !self.results_current() {
                    self.refresh_now()?;
                    self.execute_after_discovery = true;
                    return Ok(CommandResult::Continue);
                }
                if let Some(item) = self.items.get(self.selected).cloned() {
                    let should_exit = self.execute(&item, terminal)?;
                    if should_exit {
                        return Ok(CommandResult::Exit);
                    }
                    self.input.clear();
                    Ok(CommandResult::Refresh)
                } else {
                    Ok(CommandResult::Continue)
                }
            }
            Key::Up => {
                if !self.items.is_empty() {
                    self.selected = self.selected.saturating_sub(1);
                }
                Ok(CommandResult::Continue)
            }
            Key::Down => {
                if !self.items.is_empty() {
                    self.selected = (self.selected + 1).min(self.items.len() - 1);
                }
                Ok(CommandResult::Continue)
            }
            Key::Backspace => {
                if self.input.pop().is_some() {
                    Ok(CommandResult::Refresh)
                } else {
                    Ok(CommandResult::Continue)
                }
            }
            Key::CtrlU => {
                if self.input.is_empty() {
                    Ok(CommandResult::Continue)
                } else {
                    self.input.clear();
                    Ok(CommandResult::Refresh)
                }
            }
            Key::CtrlW => {
                let previous_length = self.input.len();
                while self.input.chars().last().is_some_and(char::is_whitespace) {
                    self.input.pop();
                }
                while !self.input.chars().last().is_some_and(char::is_whitespace)
                    && !self.input.is_empty()
                {
                    self.input.pop();
                }
                if self.input.len() != previous_length {
                    Ok(CommandResult::Refresh)
                } else {
                    Ok(CommandResult::Continue)
                }
            }
            Key::Char(character) if !character.is_control() => {
                self.input.push(character);
                Ok(CommandResult::Refresh)
            }
            Key::Char(_) => Ok(CommandResult::Continue),
        }
    }

    fn schedule_refresh(&mut self) {
        self.refresh_deadline = Some(Instant::now() + SEARCH_DEBOUNCE);
        self.execute_after_discovery = false;
    }

    fn request_discovery(&mut self) -> Result<()> {
        self.refresh_deadline = None;
        if self.discovery_pending && self.requested_input == self.input {
            return Ok(());
        }

        self.next_request_id += 1;
        self.latest_request_id = self.next_request_id;
        self.requested_input = self.input.clone();
        self.discovery_pending = true;
        self.discovery_tx
            .send(DiscoveryRequest {
                id: self.latest_request_id,
                input: self.requested_input.clone(),
            })
            .context("could not queue discovery request")
    }

    fn refresh_now(&mut self) -> Result<()> {
        self.request_discovery()
    }

    fn refresh_if_due(&mut self) -> Result<()> {
        if self
            .refresh_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.refresh_now()?;
        }
        Ok(())
    }

    fn collect_discoveries(&mut self, terminal: &mut Terminal) -> Result<bool> {
        while let Ok(response) = self.discovery_rx.try_recv() {
            if response.id != self.latest_request_id {
                continue;
            }
            self.discovery_pending = false;
            if response.input != self.input {
                continue;
            }

            self.active_rule = response.active_rule;
            self.query = response.query;
            match response.result {
                Ok(result) => {
                    self.items = result.items;
                    self.errors = result.errors;
                }
                Err(error) => {
                    self.items.clear();
                    self.errors = vec![error];
                }
            }
            self.results_input = response.input;
            self.selected = self.selected.min(self.items.len().saturating_sub(1));

            if self.execute_after_discovery {
                self.execute_after_discovery = false;
                if let Some(item) = self.items.get(self.selected).cloned() {
                    let should_exit = self.execute(&item, terminal)?;
                    if should_exit {
                        return Ok(true);
                    }
                    self.input.clear();
                    self.schedule_refresh();
                }
            }
        }
        Ok(false)
    }

    fn results_current(&self) -> bool {
        !self.discovery_pending
            && self.refresh_deadline.is_none()
            && self.results_input == self.input
    }

    fn input_timeout_ms(&self) -> i32 {
        let Some(deadline) = self.refresh_deadline else {
            return INPUT_POLL_MS;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        remaining.as_millis().min(INPUT_POLL_MS as u128).max(1) as i32
    }

    fn execute(&mut self, item: &Item, terminal: &mut Terminal) -> Result<bool> {
        let provider = self
            .config
            .providers
            .get(&item.provider)
            .with_context(|| format!("provider {:?} disappeared", item.provider))?;
        let Some(run) = &provider.run else {
            self.message = format!("{} has no action", item.provider);
            return Ok(false);
        };

        let value = item.value.as_deref().unwrap_or(&item.text);
        let metadata = serde_json::to_string(&item.metadata)
            .context("could not serialize selected item metadata")?;
        let shell = provider.run_shell.as_deref().unwrap_or("sh");
        let command = vec![
            shell.to_string(),
            "-c".to_string(),
            run.clone(),
            "tui-launcher".to_string(),
        ];
        let mut process = Command::new(&command[0]);
        process.args(&command[1..]);
        process.env("LAUNCHER_ITEM", &item.text);
        process.env("LAUNCHER_VALUE", value);
        process.env("LAUNCHER_METADATA", &metadata);
        process.env("LAUNCHER_PROVIDER", &item.provider);
        process.env("LAUNCHER_RULE", &self.active_rule);
        process.env("LAUNCHER_QUERY", &self.query);
        let environment = vec![
            ("LAUNCHER_ITEM".to_string(), item.text.clone()),
            ("LAUNCHER_VALUE".to_string(), value.to_string()),
            ("LAUNCHER_METADATA".to_string(), metadata),
            ("LAUNCHER_PROVIDER".to_string(), item.provider.clone()),
            ("LAUNCHER_RULE".to_string(), self.active_rule.clone()),
            ("LAUNCHER_QUERY".to_string(), self.query.clone()),
        ];

        match provider.mode {
            ActionMode::Embedded => {
                let outcome = pty::run(&command, &environment, terminal, &item.text)?;
                self.message = embedded_status_message(outcome);
                Ok(false)
            }
            ActionMode::Takeover => {
                terminal.leave()?;
                let status = process
                    .status()
                    .with_context(|| format!("could not run {}", command[0]))?;
                self.message = status_message(&status);
                Ok(true)
            }
            ActionMode::Oneshot => {
                terminal.leave()?;
                let result = process
                    .status()
                    .with_context(|| format!("could not run {}", command[0]));
                terminal.reenter()?;
                match result {
                    Ok(status) => self.message = status_message(&status),
                    Err(error) => self.message = error.to_string(),
                }
                Ok(false)
            }
            ActionMode::Capture => {
                terminal.leave()?;
                let result = process
                    .stdin(Stdio::null())
                    .output()
                    .with_context(|| format!("could not run {}", command[0]));
                terminal.reenter()?;
                let (text, status) = match result {
                    Ok(output) => {
                        let mut text = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        if !stderr.is_empty() {
                            if !text.is_empty() && !text.ends_with('\n') {
                                text.push('\n');
                            }
                            text.push_str(&stderr);
                        }
                        (sanitize_text(&text), status_message(&output.status))
                    }
                    Err(error) => (error.to_string(), "failed".to_string()),
                };
                self.show_capture(terminal, &item.text, &text, &status)?;
                Ok(false)
            }
        }
    }

    fn show_capture(
        &mut self,
        terminal: &mut Terminal,
        title: &str,
        output: &str,
        status: &str,
    ) -> Result<()> {
        let lines: Vec<String> = if output.is_empty() {
            vec!["(no output)".to_string()]
        } else {
            output.lines().map(sanitize_text).collect()
        };

        loop {
            render_capture(terminal, title, &lines, status)?;
            let bytes = terminal.read_input(80)?;
            let mut keys = self.decoder.feed(&bytes);
            keys.extend(self.decoder.flush_due());
            if keys
                .iter()
                .any(|key| matches!(key, Key::CtrlC | Key::Escape | Key::Enter | Key::Char(_)))
            {
                return Ok(());
            }
        }
    }

    fn render(&self, terminal: &Terminal) -> Result<()> {
        let (width, height) = terminal.size();
        let width = width as usize;
        let height = height as usize;
        let mut lines = Vec::with_capacity(height);
        let prefix_width = prefix_column_width(&self.items, width);
        let content_width = width.saturating_sub(prefix_width + 4);

        lines.push(format!(" TUI Launcher  [{}]", self.active_rule));
        lines.push(format!(" > {}", self.input));

        let list_height = height.saturating_sub(4);
        let start = if self.selected >= list_height && list_height > 0 {
            self.selected + 1 - list_height
        } else {
            0
        };
        let selected_row = if self.items.is_empty() || list_height == 0 {
            None
        } else {
            Some(2 + self.selected.saturating_sub(start))
        };

        let searching = self.refresh_deadline.is_some() || self.discovery_pending;
        if self.items.is_empty() {
            lines.push(if searching {
                "   (searching...)".to_string()
            } else {
                "   (no matches)".to_string()
            });
        } else {
            for (offset, item) in self.items.iter().skip(start).take(list_height).enumerate() {
                let index = start + offset;
                let marker = if index == self.selected { "> " } else { "  " };
                lines.push(format_item_line(
                    marker,
                    &item.prefix,
                    &item.text,
                    prefix_width,
                    content_width,
                ));
            }
        }

        while lines.len() < 2 + list_height {
            lines.push(String::new());
        }

        let status = if searching {
            " * searching...".to_string()
        } else if let Some(error) = self.errors.first() {
            format!(" ! {}", error)
        } else if !self.message.is_empty() {
            format!(" * {}", self.message)
        } else {
            format!(" {} | {} result(s)", self.active_rule, self.items.len())
        };
        let hint = if self.items.is_empty() {
            " Type to search | Esc clear/quit | Ctrl-C quit"
        } else if self.input.is_empty() {
            " Enter run | Up/Down select | Esc clear/quit | Ctrl-C quit"
        } else {
            " Enter run | Up/Down select | Ctrl-U clear | Esc clear/quit"
        };
        lines.push(status);
        lines.push(hint.to_string());

        let mut stdout = io::stdout().lock();
        stdout.write_all(b"\x1b[H")?;
        for row in 0..height {
            let line = lines.get(row).map(String::as_str).unwrap_or("");
            let clipped = clip(line, width);
            if selected_row == Some(row) && clipped.starts_with("> ") {
                write!(stdout, "\x1b[7m{}\x1b[0m", clipped)?;
            } else if row == 0 {
                write!(stdout, "\x1b[1;36m{}\x1b[0m", clipped)?;
            } else {
                stdout.write_all(clipped.as_bytes())?;
            }
            stdout.write_all(b"\x1b[K")?;
            if row + 1 < height {
                stdout.write_all(b"\r\n")?;
            }
        }
        stdout.flush().context("could not draw launcher")
    }
}

fn discovery_worker(
    config: Config,
    requests: Receiver<DiscoveryRequest>,
    responses: Sender<DiscoveryResponse>,
) {
    while let Ok(mut request) = requests.recv() {
        while let Ok(next_request) = requests.try_recv() {
            request = next_request;
        }

        let (rule_name, rule, rule_query) = config.resolve_rule(&request.input);
        let (provider_prefix, query) = config.resolve_provider_prefix(&rule_query);
        let result = discover(&config, rule_name, rule, &query, provider_prefix.as_deref())
            .map_err(|error| error.to_string());
        let response = DiscoveryResponse {
            id: request.id,
            input: request.input,
            active_rule: rule_name.to_string(),
            query,
            result,
        };
        if responses.send(response).is_err() {
            break;
        }
    }
}

fn prefix_column_width(items: &[Item], width: usize) -> usize {
    let longest = items
        .iter()
        .map(|item| UnicodeWidthStr::width(item.prefix.as_str()))
        .max()
        .unwrap_or(6)
        .clamp(6, 20);
    longest.min(width.saturating_sub(8).max(1))
}

fn format_item_line(
    marker: &str,
    prefix: &str,
    content: &str,
    prefix_width: usize,
    content_width: usize,
) -> String {
    let prefix = pad_right(&clip(prefix, prefix_width), prefix_width);
    format!("{}{}  {}", marker, prefix, clip(content, content_width))
}

fn pad_right(text: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(text);
    format!("{}{}", text, " ".repeat(width.saturating_sub(used)))
}

fn render_capture(terminal: &Terminal, title: &str, lines: &[String], status: &str) -> Result<()> {
    let (width, height) = terminal.size();
    let width = width as usize;
    let height = height as usize;
    let inner_height = height.saturating_sub(4).max(1);
    let start = lines.len().saturating_sub(inner_height);

    let mut stdout = io::stdout().lock();
    stdout.write_all(b"\x1b[?25l")?;
    write_capture_line(
        &mut stdout,
        1,
        &format!(" TUI Launcher  [capture: {}]", title),
        width,
        true,
    )?;
    write_capture_line(&mut stdout, 2, &format!(" > {}", title), width, false)?;
    for row in 0..inner_height {
        let content = lines.get(start + row).map(String::as_str).unwrap_or("");
        write_capture_line(&mut stdout, 3 + row, content, width, false)?;
    }
    write_capture_line(
        &mut stdout,
        height.saturating_sub(1).max(1),
        &format!(" status: {}", status),
        width,
        false,
    )?;
    write_capture_line(
        &mut stdout,
        height.max(1),
        " press any key to return | Esc",
        width,
        false,
    )?;
    stdout.flush().context("could not draw command output")
}

fn write_capture_line(
    stdout: &mut impl Write,
    row: usize,
    text: &str,
    width: usize,
    heading: bool,
) -> Result<()> {
    write!(stdout, "\x1b[{};1H\x1b[K", row)?;
    let text = clip(text, width);
    if heading {
        write!(stdout, "\x1b[1;36m{}\x1b[0m", text)?;
    } else {
        stdout.write_all(text.as_bytes())?;
    }
    Ok(())
}

fn embedded_status_message(outcome: EmbeddedOutcome) -> String {
    match outcome {
        EmbeddedOutcome::ReturnedToLauncher => "embedded command stopped".to_string(),
        EmbeddedOutcome::Exited(0) => "embedded command finished successfully".to_string(),
        EmbeddedOutcome::Exited(code) => format!("embedded command exited with code {}", code),
        EmbeddedOutcome::Signaled(signal) => {
            format!("embedded command terminated by signal {}", signal)
        }
    }
}

fn status_message(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "finished successfully".to_string(),
        Some(code) => format!("finished with exit code {}", code),
        None => "terminated by signal".to_string(),
    }
}

fn clip(text: &str, width: usize) -> String {
    if width == 0 || UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }

    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width - 3 {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push_str("...");
    result
}

#[derive(Debug, Clone, Copy)]
enum CommandResult {
    Continue,
    Refresh,
    Exit,
}

#[derive(Debug, Clone, Copy)]
enum Key {
    Char(char),
    Enter,
    Backspace,
    Up,
    Down,
    Escape,
    CtrlC,
    CtrlD,
    CtrlU,
    CtrlW,
}

#[derive(Default)]
struct InputDecoder {
    pending: Vec<u8>,
    escape_since: Option<Instant>,
}

impl InputDecoder {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Key> {
        self.pending.extend_from_slice(bytes);
        self.parse()
    }

    fn flush_due(&mut self) -> Vec<Key> {
        if self
            .escape_since
            .is_some_and(|started| started.elapsed() >= Duration::from_millis(35))
        {
            self.escape_since = None;
            if self.pending.first() == Some(&0x1b) {
                self.pending.remove(0);
                return vec![Key::Escape];
            }
        }
        Vec::new()
    }

    fn parse(&mut self) -> Vec<Key> {
        let mut keys = Vec::new();
        loop {
            let Some(&first) = self.pending.first() else {
                self.escape_since = None;
                break;
            };

            if first == 0x1b {
                if self.pending.len() == 1 {
                    self.escape_since.get_or_insert_with(Instant::now);
                    break;
                }
                if self.pending[1] != b'[' {
                    self.pending.remove(0);
                    keys.push(Key::Escape);
                    self.escape_since = None;
                    continue;
                }
                if self.pending.len() < 3 {
                    self.escape_since.get_or_insert_with(Instant::now);
                    break;
                }
                let code = self.pending[2];
                let key = match code {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    _ => None,
                };
                if let Some(key) = key {
                    self.pending.drain(..3);
                    self.escape_since = None;
                    keys.push(key);
                    continue;
                }
                if self.pending.last() == Some(&b'~') {
                    self.pending.clear();
                    self.escape_since = None;
                    continue;
                }
                if self.pending.len() > 8 {
                    self.pending.remove(0);
                    self.escape_since = None;
                    keys.push(Key::Escape);
                    continue;
                }
                self.escape_since.get_or_insert_with(Instant::now);
                break;
            }

            if let Some(key) = control_key(first) {
                self.pending.remove(0);
                keys.push(key);
                continue;
            }

            if first < 0x80 {
                self.pending.remove(0);
                keys.push(Key::Char(first as char));
                continue;
            }

            let width = utf8_width(first);
            if self.pending.len() < width {
                break;
            }
            match std::str::from_utf8(&self.pending[..width]) {
                Ok(text) => {
                    if let Some(character) = text.chars().next() {
                        keys.push(Key::Char(character));
                    }
                    self.pending.drain(..width);
                }
                Err(_) => {
                    self.pending.remove(0);
                }
            }
        }
        keys
    }
}

fn control_key(byte: u8) -> Option<Key> {
    match byte {
        b'\r' | b'\n' => Some(Key::Enter),
        0x7f | 0x08 => Some(Key::Backspace),
        0x03 => Some(Key::CtrlC),
        0x04 => Some(Key::CtrlD),
        0x15 => Some(Key::CtrlU),
        0x17 => Some(Key::CtrlW),
        _ => None,
    }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}
