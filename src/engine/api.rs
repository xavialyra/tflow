use super::host::EngineHost;
use super::picker::TaskScheduler;
use crate::command::{
    CommandContext, CommandExecution, CommandInvocation, InputActionBinding, InputFocus,
    InputRefreshPolicy, LauncherOutcome, NavigationRequest, SelectionBindingState, ViewAction,
    ViewEffect, ViewInputMode, ViewOutput,
};
use crate::config::{Config, Defaults, EvaluationSnapshot, View};
use crate::input::InputBuffer;
use crate::input::{DecodedInput, Key};
use crate::lifecycle::CancellationToken;
use crate::state::StateInstance;
use crate::terminal::Terminal;
use anyhow::{Result, bail};
use ratatui::{Frame, layout::Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddedResultFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EmbeddedResultConfig {
    pub(crate) format: EmbeddedResultFormat,
    pub(crate) required: bool,
    pub(crate) max_bytes: usize,
}

pub(crate) trait ViewInstance {
    fn activate(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn deactivate(&mut self) -> Result<()> {
        Ok(())
    }

    fn restore_input(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn input_committed(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn input_refresh_policy(&self) -> InputRefreshPolicy {
        InputRefreshPolicy::None
    }

    fn input_ready(&mut self, _host: &mut EngineHost<'_>) -> Result<ViewEffect> {
        Ok(ViewEffect::Continue)
    }

    fn input_rejected(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn step(&mut self, host: &mut EngineHost<'_>, terminal: &mut Terminal) -> Result<ViewEffect>;

    /// Poll a suspended View whose runtime must remain alive while another
    /// command mode or overlay is active. Ordinary Views do nothing here.
    fn background_step(
        &mut self,
        _host: &mut EngineHost<'_>,
        _terminal: &mut Terminal,
    ) -> Result<ViewEffect> {
        Ok(ViewEffect::Continue)
    }

    fn launcher_input_timeout(&self, _host: &EngineHost<'_>) -> Option<i32> {
        None
    }

    fn captures_editor_input(&self) -> bool {
        false
    }

    fn input_mode(&self) -> ViewInputMode {
        ViewInputMode::Keymap
    }

    fn input_action_bindings(&self, _host: &EngineHost<'_>) -> Vec<InputActionBinding> {
        Vec::new()
    }

    fn selection_binding_state(&self, _host: &EngineHost<'_>) -> SelectionBindingState {
        SelectionBindingState::None
    }

    fn handle_unbound_input(
        &mut self,
        _host: &mut EngineHost<'_>,
        _bytes: &[u8],
    ) -> Result<LauncherOutcome> {
        Ok(LauncherOutcome::Continue)
    }

    fn handle_terminal_eof(&mut self, _host: &mut EngineHost<'_>) -> Result<LauncherOutcome> {
        Ok(LauncherOutcome::Effect(Box::new(ViewEffect::Exit)))
    }

    fn resolve_view_command(&self, host: &EngineHost<'_>, key: Key) -> Option<CommandInvocation> {
        crate::command::find_command_for_key(host.config, host.state.view_ref(), key)
    }

    fn view_command_output(&self) -> Option<ViewOutput> {
        None
    }

    fn view_command_context(&self, host: &mut EngineHost<'_>) -> Result<CommandContext> {
        Ok(CommandContext {
            page: crate::command::CommandOwnerContext {
                view_ref: host.state.view_ref().to_string(),
                state: host.state.clone(),
                binding_raw: host.input.params.clone(),
            },
            selection: None,
            runtime: host.runtime.snapshot().clone(),
            output: self.view_command_output(),
        })
    }

    fn prepare_view_command(
        &self,
        host: &mut EngineHost<'_>,
        invocation: CommandInvocation,
    ) -> Result<CommandExecution> {
        let context = self.view_command_context(host)?;
        crate::command::resolve_visible_command(
            host.config,
            &context,
            invocation
                .view_reference()
                .expect("View command invocation has a footer origin"),
        )?;
        Ok(CommandExecution {
            invocation,
            context,
        })
    }

    fn handle_view_command(
        &mut self,
        host: &mut EngineHost<'_>,
        invocation: CommandInvocation,
        _input: DecodedInput,
    ) -> Result<LauncherOutcome> {
        let execution = self.prepare_view_command(host, invocation)?;
        Ok(LauncherOutcome::Effect(Box::new(
            ViewEffect::DispatchCommand(execution),
        )))
    }

    fn handle_view_binding(
        &mut self,
        host: &mut EngineHost<'_>,
        key: Key,
        input: DecodedInput,
    ) -> Result<LauncherOutcome> {
        let Some(invocation) = self.resolve_view_command(host, key) else {
            return Ok(LauncherOutcome::Continue);
        };
        self.handle_view_command(host, invocation, input)
    }

    fn handle_pending_view_binding(
        &mut self,
        host: &mut EngineHost<'_>,
        key: Key,
        input: DecodedInput,
    ) -> Result<LauncherOutcome> {
        self.handle_view_binding(host, key, input)
    }

    fn handle_view_action(
        &mut self,
        _host: &mut EngineHost<'_>,
        _action: ViewAction,
        _input: DecodedInput,
    ) -> Result<LauncherOutcome> {
        Ok(LauncherOutcome::Continue)
    }

    fn chrome(&self, _host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        crate::chrome::EngineChrome::default()
    }

    fn render(&mut self, host: &EngineHost<'_>, frame: &mut Frame, area: Rect);

    fn input_focus(&self) -> InputFocus {
        InputFocus::Focused
    }
}

pub(crate) struct ViewContext<'a> {
    pub(crate) config: &'a Config,
    pub(crate) request: &'a NavigationRequest,
    pub(crate) input: &'a InputBuffer,
    pub(crate) state: &'a StateInstance,
    pub(crate) evaluation: EvaluationSnapshot<'a>,
    pub(crate) tasks: TaskScheduler,
    pub(crate) cancellation: CancellationToken,
}

pub(crate) trait Engine {
    fn engine_type(&self) -> &'static str;

    fn validate_config(&self, name: &str, view: &View) -> Result<()>;

    fn validate_defaults(&self, _defaults: &Defaults) -> Result<()> {
        Ok(())
    }

    fn validate_keymap(&self, name: &str, view: &View) -> Result<()> {
        if view.keymap.is_some() {
            bail!(
                "view {:?} using engine {:?} cannot define a keymap",
                name,
                self.engine_type()
            );
        }
        Ok(())
    }

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>>;
}
