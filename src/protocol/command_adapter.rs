use crate::protocol::contracts::ViewInstanceId;
use crate::view::{
    CallBoundary, CallReturnHandler, ViewCommandSnapshot, ViewContext, ViewDecision, ViewLocation,
    ViewResult,
};
use anyhow::{Context, Result, bail};

/// Immutable command bindings projected into protocol-native Views.
/// Runtime preparation remains owned by `ProtocolCommandService`.
#[derive(Clone)]
pub(crate) struct ViewCommandBindings {
    pub(crate) bindings: Vec<ProtocolCommandBinding>,
    pub(crate) overflow_binding: Option<ProtocolCommandBinding>,
    pub(crate) has_unbound: bool,
    view_invocations:
        std::collections::BTreeMap<(String, String), crate::workflow::command::CommandInvocation>,
}

#[derive(Clone)]
pub(crate) struct ProtocolCommandBinding {
    pub(crate) key: crate::input::Key,
    pub(crate) label: Option<String>,
    #[allow(dead_code)]
    pub(crate) visibility: crate::workflow::config::CommandBindingVisibility,
    pub(crate) invocation: crate::workflow::command::CommandInvocation,
}

impl ViewCommandBindings {
    pub(crate) fn new(
        config: &crate::workflow::config::CompiledConfig,
        view_ref: &str,
        _cancellation: crate::lifecycle::CancellationObserver,
    ) -> anyhow::Result<Self> {
        let mut bindings = Vec::new();
        let mut overflow_binding = None;
        let mut add = |id: String,
                       binding: &crate::workflow::config::CommandBinding,
                       invocation|
         -> anyhow::Result<()> {
            let Some(key) = binding.key(&id) else {
                return Ok(());
            };
            let visibility = binding
                .visibility(&id)
                .unwrap_or(crate::workflow::config::CommandBindingVisibility::Always);
            let key =
                crate::input::Key::parse_binding(&crate::workflow::config::normalize_key(key)?)?;
            let entry = ProtocolCommandBinding {
                key,
                label: binding.label(&id).map(str::to_string),
                visibility,
                invocation,
            };
            if visibility == crate::workflow::config::CommandBindingVisibility::Overflow {
                overflow_binding = Some(entry);
            } else {
                bindings.push(entry);
            }
            Ok(())
        };
        let mut globals = config.commands.bindings.clone();
        if !globals.contains_key("commands") && config.view("selectors:commands").is_some() {
            globals.insert(
                "commands".to_string(),
                crate::workflow::config::CommandBinding::builtin_commands(),
            );
        }
        for (id, binding) in globals {
            let Some(command) = binding.as_command(&id) else {
                continue;
            };
            add(
                id.clone(),
                &binding,
                crate::workflow::command::CommandInvocation::session_command(view_ref, id, command),
            )?;
        }
        if let Some(view) = config.view(view_ref) {
            for (id, command) in &view.commands {
                add(
                    id.clone(),
                    &crate::workflow::config::CommandBinding {
                        key: command.key.clone(),
                        label: Some(command.label.clone()),
                        visibility: None,
                        action: Some(command.action.clone()),
                    },
                    crate::workflow::command::CommandInvocation::view(
                        crate::workflow::command::CommandRef {
                            view: view_ref.to_string(),
                            id: id.clone(),
                        },
                        command.clone(),
                    ),
                )?;
            }
        }
        let view_invocations = config
            .iter_views()
            .flat_map(|(owner, view)| {
                view.commands.iter().map(move |(id, command)| {
                    (
                        (owner.clone(), id.clone()),
                        crate::workflow::command::CommandInvocation::view(
                            crate::workflow::command::CommandRef {
                                view: owner.clone(),
                                id: id.clone(),
                            },
                            command.clone(),
                        ),
                    )
                })
            })
            .collect();
        let has_unbound = config
            .view(view_ref)
            .is_some_and(|view| view.commands.values().any(|command| command.key.is_none()))
            || config
                .session_commands()
                .values()
                .any(|command| command.key.is_none());
        Ok(Self {
            bindings,
            overflow_binding,
            has_unbound,
            view_invocations,
        })
    }

    pub(crate) fn overflow_command(&self) -> Option<(String, String)> {
        let b = self.overflow_binding.as_ref()?;
        Some((b.key.binding_name()?, b.label.as_ref()?.clone()))
    }

    pub(crate) fn has_unbound(&self) -> bool {
        self.has_unbound
    }

    pub(crate) fn is_palette_active(
        &self,
        width: usize,
        title: Option<&str>,
        status: Option<&str>,
    ) -> bool {
        if self.overflow_binding.is_none() {
            return false;
        }
        if self.has_unbound {
            return true;
        }
        let regular_commands = self
            .bindings
            .iter()
            .filter_map(|b| Some((b.key.binding_name()?, b.label.as_ref()?.clone())))
            .collect::<Vec<_>>();
        crate::ui::chrome::is_palette_active(
            width,
            title,
            status,
            &regular_commands,
            self.has_unbound,
        )
    }

    pub(crate) fn binding(&self, key: crate::input::Key) -> Option<&ProtocolCommandBinding> {
        self.bindings
            .iter()
            .find(|binding| binding.key.binding_identity() == key.binding_identity())
            .or_else(|| {
                self.overflow_binding
                    .as_ref()
                    .filter(|binding| binding.key.binding_identity() == key.binding_identity())
            })
    }

    pub(crate) fn view_bindings(&self) -> crate::view::BindingSet {
        let mut seen = std::collections::HashSet::new();
        crate::view::BindingSet::new(self.bindings.iter().filter_map(|binding| {
            seen.insert(binding.key.binding_identity())
                .then_some(crate::view::Binding {
                    key: binding.key,
                    label: binding.label.clone(),
                })
        }))
    }

    pub(crate) fn request(
        &self,
        binding: &ProtocolCommandBinding,
        owner: Option<crate::workflow::command::CommandOwnerContext>,
    ) -> crate::view::ViewDecision {
        self.request_for_invocation(binding.invocation.clone(), owner)
    }

    pub(crate) fn request_for_invocation(
        &self,
        invocation: crate::workflow::command::CommandInvocation,
        owner: Option<crate::workflow::command::CommandOwnerContext>,
    ) -> crate::view::ViewDecision {
        crate::view::ViewDecision::RequestCommand(crate::view::CommandRequest { invocation, owner })
    }

    pub(crate) fn view_invocation(
        &self,
        owner: &str,
        id: &str,
    ) -> anyhow::Result<crate::workflow::command::CommandInvocation> {
        self.view_invocations
            .get(&(owner.to_string(), id.to_string()))
            .cloned()
            .with_context(|| format!("dynamic command {id:?} for View {owner:?} is unavailable"))
    }
}

pub(crate) trait CommandService {
    fn execute(
        &mut self,
        request: crate::view::CommandRequest,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<ViewDecision>;
}

pub(crate) struct ProtocolCommandService {
    config: std::sync::Arc<crate::workflow::config::CompiledConfig>,
    invocation: std::sync::Arc<crate::workflow::InvocationContext>,
    cancellation: crate::lifecycle::CancellationToken,
}

impl ProtocolCommandService {
    pub(crate) fn new(
        config: std::sync::Arc<crate::workflow::config::CompiledConfig>,
        invocation: std::sync::Arc<crate::workflow::InvocationContext>,
        cancellation: crate::lifecycle::CancellationToken,
    ) -> Self {
        Self {
            config,
            invocation,
            cancellation,
        }
    }
}

impl CommandService for ProtocolCommandService {
    fn execute(
        &mut self,
        request: crate::view::CommandRequest,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<ViewDecision> {
        let page = command_page_owner(context, snapshot);
        let owner = request.owner.unwrap_or_else(|| page.clone());
        let execution = crate::workflow::command::CommandExecution {
            invocation: request.invocation,
            context: crate::workflow::command::CommandContext {
                page,
                owner,
                current: snapshot
                    .publication
                    .as_ref()
                    .map(|publication| publication.current.clone())
                    .unwrap_or(serde_json::Value::Null),
            },
        };
        let prepared = crate::workflow::command::prepare_command_action(
            &self.config,
            &self.invocation,
            execution,
            &self.cancellation,
        )
        .map_err(crate::view::operation_failure)?;
        map_prepared_action(
            &self.config,
            &self.invocation,
            &self.cancellation,
            prepared,
            context.instance,
        )
        .map_err(crate::view::operation_failure)
    }
}

fn command_page_owner(
    context: &ViewContext,
    snapshot: &ViewCommandSnapshot,
) -> crate::workflow::command::CommandOwnerContext {
    let parameters = crate::workflow::parameter::ParameterSnapshot::from_parts(
        snapshot.parameters.clone(),
        snapshot.raw_input.clone(),
        crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(context.instance.0),
            generation: snapshot.revision,
        },
        snapshot.revision,
    );
    crate::workflow::command::CommandOwnerContext {
        view_ref: context.location.target.clone(),
        parameters,
        binding_raw: snapshot.raw_input.clone(),
    }
}

#[derive(Clone)]
struct ProtocolCallReturnHandler {
    config: std::sync::Arc<crate::workflow::config::CompiledConfig>,
    invocation: std::sync::Arc<crate::workflow::InvocationContext>,
    cancellation: crate::lifecycle::CancellationToken,
    origin: crate::workflow::command::CommandOrigin,
    context: crate::workflow::command::CommandContext,
    return_processor: Option<crate::workflow::config::ReturnProcessor>,
    invoke_selected: bool,
}

impl CallReturnHandler for ProtocolCallReturnHandler {
    fn post_commit(&self) -> bool {
        self.return_processor.is_some() || self.invoke_selected
    }

    fn resume(
        &self,
        _source: &ViewLocation,
        caller: &ViewContext,
        _snapshot: &ViewCommandSnapshot,
        result: &ViewResult,
    ) -> Result<ViewDecision> {
        let context = self.context.clone();
        if let Some(processor) = &self.return_processor {
            let action = crate::workflow::command::prepare_return_processor(
                &self.config,
                &self.invocation,
                processor,
                self.origin.clone(),
                context,
                caller,
                result,
                &self.cancellation,
            )?;
            return map_prepared_action(
                &self.config,
                &self.invocation,
                &self.cancellation,
                action,
                caller.instance,
            );
        }
        if self.invoke_selected {
            let output: crate::workflow::command::ViewOutput =
                serde_json::from_value(result.value.clone())
                    .context("command selector returned an invalid command output")?;
            let crate::workflow::command::ViewOutput::Value { value } = output else {
                bail!("command selector must return a command reference value");
            };
            let reference: crate::workflow::command::CommandRef =
                serde_json::from_value(value).context("command selector must return {view, id}")?;
            let invocation = crate::workflow::command::resolve_visible_command(
                &self.config,
                &context,
                &reference,
            )?;
            return map_prepared_action(
                &self.config,
                &self.invocation,
                &self.cancellation,
                crate::workflow::command::PreparedAction::Invoke(
                    crate::workflow::command::CommandExecution {
                        invocation,
                        context,
                    },
                ),
                caller.instance,
            );
        }
        Ok(ViewDecision::Stay)
    }
}

pub(crate) fn map_prepared_action(
    config: &std::sync::Arc<crate::workflow::config::CompiledConfig>,
    invocation: &std::sync::Arc<crate::workflow::InvocationContext>,
    cancellation: &crate::lifecycle::CancellationToken,
    action: crate::workflow::command::PreparedAction,
    caller: ViewInstanceId,
) -> anyhow::Result<crate::view::ViewDecision> {
    use crate::view::{TransitionRequest, ViewDecision, ViewResult};
    use crate::workflow::command::PreparedAction;
    match action {
        PreparedAction::Navigate { request, mode } => {
            let request = protocol_navigation_request(config, request)?;
            Ok(ViewDecision::Transition(match mode {
                crate::workflow::command::NavigationMode::Push => TransitionRequest::Push(request),
                crate::workflow::command::NavigationMode::Replace => {
                    TransitionRequest::Replace(request)
                }
            }))
        }
        PreparedAction::Call(call) => {
            let request = protocol_navigation_request(config, call.request)?;
            let boundary = CallBoundary {
                caller,
                handler: std::sync::Arc::new(ProtocolCallReturnHandler {
                    config: std::sync::Arc::clone(config),
                    invocation: std::sync::Arc::clone(invocation),
                    cancellation: cancellation.clone(),
                    origin: call.origin,
                    context: call.context,
                    return_processor: call.return_processor,
                    invoke_selected: call.invoke_selected,
                }),
            };
            Ok(ViewDecision::Transition(TransitionRequest::Call {
                request,
                continuation: crate::view::Continuation::Call(boundary),
            }))
        }
        PreparedAction::Return(returned) => Ok(ViewDecision::Return(ViewResult {
            value: serde_json::to_value(returned.output)?,
        })),
        PreparedAction::EditInput { value, cursor } => Ok(ViewDecision::Command(
            crate::view::CommandResult::EditInput { value, cursor },
        )),
        PreparedAction::Invoke(execution) => map_prepared_action(
            config,
            invocation,
            cancellation,
            crate::workflow::command::prepare_command_action(
                config,
                invocation,
                execution,
                cancellation,
            )?,
            caller,
        ),
        PreparedAction::Execute { prepared, exit, .. } => {
            let effect = ViewDecision::Effect(crate::view::EffectRequest::RunPrepared(prepared));
            Ok(if exit {
                ViewDecision::Batch(vec![effect, ViewDecision::Exit])
            } else {
                effect
            })
        }
    }
}

fn protocol_navigation_request(
    config: &crate::workflow::config::CompiledConfig,
    request: crate::workflow::command::NavigationRequest,
) -> anyhow::Result<crate::view::NavigationRequest> {
    let mut state = config.instantiate_parameters(&request.view_ref)?;
    config.sanitize_initial_parameter_values(&mut state)?;

    let editable = if let Some(parameters) = request.parameters.as_ref() {
        if !parameters.is_null() {
            config.update_sanitized_initial_parameter_values(&mut state, parameters)?;
        }
        None
    } else if let Some(input) = request.input.as_ref() {
        let text = crate::terminal::sanitize_terminal_text(&input.params);
        config.update_initial_parameter_input(&mut state, &text)?;
        let cursor = if text == input.params {
            input.cursor
        } else {
            crate::terminal::sanitize_terminal_text(&input.params[..input.cursor]).len()
        };
        Some((text, cursor))
    } else {
        let text = crate::terminal::sanitize_terminal_text(&config.render_parameter_input(&state)?);
        Some((text.clone(), text.len()))
    };

    let values = config.parameter_values(&state)?;
    let query = crate::view::ParsedQuery::new(&request.view_ref, "query", values);
    let mut protocol_request = crate::view::NavigationRequest::new(&request.view_ref, query);
    if let Some((text, cursor)) = editable {
        protocol_request = protocol_request.with_input(text, cursor)?;
    }
    protocol_request.presentation = request.presentation;
    Ok(protocol_request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::PreparedProcess;
    use crate::view::{EffectRequest, ViewDecision};
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn prepared_process_reaches_effect_request_unchanged() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let invocation = Arc::new(
            crate::workflow::InvocationContext::new(
                "core:default".to_string(),
                serde_json::Value::Null,
                config.instantiate_parameters("core:default").unwrap(),
            )
            .unwrap(),
        );
        let prepared = PreparedProcess {
            argv: vec!["command".to_string(), "argument".to_string()],
            environment: vec![("KEY".to_string(), "value".to_string())],
            current_dir: Some(PathBuf::from("/tmp/prepared-process")),
        };

        let decision = map_prepared_action(
            &config,
            &invocation,
            &crate::lifecycle::CancellationToken::new(),
            crate::workflow::command::PreparedAction::Execute {
                prepared: prepared.clone(),
                exit: false,
            },
            ViewInstanceId(1),
        )
        .unwrap();

        assert_eq!(
            decision,
            ViewDecision::Effect(EffectRequest::RunPrepared(prepared))
        );
    }
}
