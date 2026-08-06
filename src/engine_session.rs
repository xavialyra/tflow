use crate::config::Config;
use crate::discovery::{DiscoveryResult, discover};
use crate::render;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::{Key, LauncherFocus};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

pub(crate) struct LauncherFrame {
    pub(crate) view: String,
    pub(crate) input: String,
    pub(crate) items: Vec<crate::discovery::Item>,
    pub(crate) selected: usize,
    pub(crate) active_rule: String,
    pub(crate) query: String,
    pub(crate) refresh_deadline: Option<Instant>,
    pub(crate) requested_input: String,
    pub(crate) results_input: String,
    pub(crate) discovery_pending: bool,
    pub(crate) pending_command: Option<Key>,
    pub(crate) command_owner: Option<String>,
}

impl LauncherFrame {
    pub(crate) fn new(view: &str, default_rule: &str) -> Self {
        Self {
            view: view.to_string(),
            input: String::new(),
            items: Vec::new(),
            selected: 0,
            active_rule: default_rule.to_string(),
            query: String::new(),
            refresh_deadline: None,
            requested_input: String::new(),
            results_input: String::new(),
            discovery_pending: false,
            pending_command: None,
            command_owner: None,
        }
    }
}

struct DiscoveryRequest {
    id: u64,
    view: String,
    input: String,
    log_file: Option<PathBuf>,
}

struct DiscoveryResponse {
    id: u64,
    view: String,
    input: String,
    active_rule: String,
    query: String,
    result: std::result::Result<DiscoveryResult, String>,
}

pub(crate) struct DiscoveryEvent {
    pub(crate) current: bool,
    pub(crate) view: String,
    pub(crate) errors: Vec<String>,
    pub(crate) failure: Option<String>,
    pub(crate) pending_command: Option<Key>,
}

pub(crate) struct LauncherSession {
    frames: Vec<LauncherFrame>,
    discovery_tx: Sender<DiscoveryRequest>,
    discovery_rx: Receiver<DiscoveryResponse>,
    next_request_id: u64,
    latest_request_id: u64,
    requested_view: String,
    log_file: Option<PathBuf>,
    focus: LauncherFocus,
}

impl LauncherSession {
    pub(crate) fn new(
        view: &str,
        default_rule: &str,
        config: Config,
        log_file: Option<PathBuf>,
    ) -> Self {
        let (discovery_tx, request_rx) = mpsc::channel();
        let (response_tx, discovery_rx) = mpsc::channel();
        thread::spawn(move || discovery_worker(config, request_rx, response_tx));
        Self {
            frames: vec![LauncherFrame::new(view, default_rule)],
            discovery_tx,
            discovery_rx,
            next_request_id: 0,
            latest_request_id: 0,
            requested_view: String::new(),
            log_file,
            focus: LauncherFocus::Launcher,
        }
    }

    pub(crate) fn focus(&self) -> LauncherFocus {
        self.focus
    }

    pub(crate) fn take_focus(&mut self, focus: LauncherFocus) -> Result<()> {
        if focus == LauncherFocus::Launcher {
            anyhow::bail!("launcher focus must be restored with restore_focus")
        }
        if self.focus != LauncherFocus::Launcher {
            anyhow::bail!("launcher session focus is already {:?}", self.focus)
        }
        self.focus = focus;
        Ok(())
    }

    pub(crate) fn restore_focus(&mut self) {
        self.focus = LauncherFocus::Launcher;
    }

    pub(crate) fn current(&self) -> &LauncherFrame {
        self.frames.last().expect("launcher always has a root view")
    }

    pub(crate) fn current_mut(&mut self) -> &mut LauncherFrame {
        self.frames
            .last_mut()
            .expect("launcher always has a root view")
    }

    pub(crate) fn push(&mut self, frame: LauncherFrame) {
        self.frames.push(frame);
    }

    pub(crate) fn pop(&mut self) -> Option<LauncherFrame> {
        (self.frames.len() > 1).then(|| self.frames.pop().expect("frame exists"))
    }

    pub(crate) fn parent(&self) -> Option<&LauncherFrame> {
        self.frames.get(self.frames.len().saturating_sub(2))
    }

    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn schedule_refresh(&mut self) {
        let frame = self.current_mut();
        frame.refresh_deadline = Some(Instant::now() + SEARCH_DEBOUNCE);
        frame.pending_command = None;
    }

    pub(crate) fn refresh_due(&self) -> bool {
        self.current()
            .refresh_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub(crate) fn results_current(&self) -> bool {
        let frame = self.current();
        !frame.discovery_pending
            && frame.refresh_deadline.is_none()
            && frame.results_input == frame.input
    }

    pub(crate) fn command_owner(&self) -> Option<&str> {
        let frame = self.current();
        frame.command_owner.as_deref().or_else(|| {
            frame
                .items
                .get(frame.selected)
                .map(|item| item.source_view.as_str())
        })
    }

    pub(crate) fn request_discovery(&mut self, view: &str, input: &str) -> Result<()> {
        if self.current().discovery_pending
            && self.requested_view == view
            && self.current().requested_input == input
        {
            return Ok(());
        }
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.latest_request_id = self.next_request_id;
        self.requested_view = view.to_string();
        let frame = self.current_mut();
        frame.requested_input = input.to_string();
        frame.discovery_pending = true;
        frame.refresh_deadline = None;
        self.discovery_tx
            .send(DiscoveryRequest {
                id: self.latest_request_id,
                view: view.to_string(),
                input: input.to_string(),
                log_file: self.log_file.clone(),
            })
            .context("could not queue discovery request")
    }

    pub(crate) fn collect_discoveries(&mut self) -> Vec<DiscoveryEvent> {
        let mut events = Vec::new();
        while let Ok(response) = self.discovery_rx.try_recv() {
            if !self.response_matches(&response) {
                let (errors, failure) = discovery_result_error(&response.result);
                events.push(DiscoveryEvent {
                    current: false,
                    view: response.view,
                    errors,
                    failure,
                    pending_command: None,
                });
                continue;
            }

            let view = response.view;
            let pending_command;
            let (errors, failure) = match response.result {
                Ok(result) => {
                    let errors = result.errors;
                    let frame = self.current_mut();
                    frame.discovery_pending = false;
                    frame.active_rule = response.active_rule;
                    frame.query = response.query;
                    frame.items = result.items;
                    frame.results_input = response.input;
                    frame.selected = frame.selected.min(frame.items.len().saturating_sub(1));
                    pending_command = frame.pending_command.take();
                    (errors, None)
                }
                Err(error) => {
                    let frame = self.current_mut();
                    frame.discovery_pending = false;
                    frame.active_rule = response.active_rule;
                    frame.query = response.query;
                    frame.items.clear();
                    frame.results_input = response.input;
                    frame.selected = 0;
                    pending_command = frame.pending_command.take();
                    (Vec::new(), Some(error))
                }
            };
            events.push(DiscoveryEvent {
                current: true,
                view,
                errors,
                failure,
                pending_command,
            });
        }
        events
    }

    fn response_matches(&self, response: &DiscoveryResponse) -> bool {
        response.id == self.latest_request_id
            && response.view == self.current().view
            && response.input == self.current().input
    }
}

impl super::EngineSession for LauncherSession {
    fn handle_key(&mut self, key: Key, command_view: &str) -> super::EngineKeyAction {
        super::LauncherEngine::handle_key(self, key, command_view)
    }

    fn collect_events(&mut self) -> Vec<super::DiscoveryEvent> {
        self.collect_discoveries()
    }

    fn render(&self, config: &Config, terminal: &Terminal, status: Option<&str>) -> Result<()> {
        let state = super::LauncherEngine::render_state(config, self);
        let left = status.unwrap_or(state.view.as_str());
        render::render_launcher(terminal, &state, left)
    }
}

fn discovery_result_error(
    result: &std::result::Result<DiscoveryResult, String>,
) -> (Vec<String>, Option<String>) {
    match result {
        Ok(result) => (result.errors.clone(), None),
        Err(error) => (Vec::new(), Some(error.clone())),
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

        let (source_prefix, query) = config.resolve_view_prefix(&request.view, &request.input);
        let (rule_name, rule, rule_query) = config.resolve_rule(&query);
        let result = discover(
            &config,
            &request.view,
            rule_name,
            rule,
            &rule_query,
            source_prefix.as_deref(),
            request.log_file.as_deref(),
        )
        .map_err(|error| error.to_string());
        let response = DiscoveryResponse {
            id: request.id,
            view: request.view,
            input: request.input,
            active_rule: rule_name.to_string(),
            query: rule_query,
            result,
        };
        if responses.send(response).is_err() {
            break;
        }
    }
}
