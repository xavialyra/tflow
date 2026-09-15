use crate::protocol::contracts::ViewInstanceId;
use crate::view::{
    CallBoundary, CallReturnHandler, ViewCommandSnapshot, ViewContext, ViewDecision, ViewLocation,
    ViewResult,
};
use anyhow::Result;

pub(crate) trait CommandService {
    fn build_host_commands(
        &self,
        shared_snapshot: std::sync::Arc<std::sync::RwLock<crate::command::ChromeSnapshot>>,
        registry: std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>>,
    ) -> Result<Vec<crate::command::CommandEntry>>;

    fn build_view_commands(
        &self,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>>;

    fn build_engine_commands(
        &self,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>>;
}

#[derive(Clone)]
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

    fn build_commands_for_scope(
        &self,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
        scope: crate::command::CommandScope,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        let mut entries = Vec::new();
        let target = &context.location.target;
        let mut views_to_collect = vec![(target.clone(), true)];
        let has_item = snapshot
            .publication
            .as_ref()
            .is_some_and(|p| p.ready && !p.current.is_null());

        if has_item {
            if let Some(owner) = &snapshot.owner_view {
                if owner != target {
                    views_to_collect.push((owner.clone(), false));
                }
            }
        }

        for (view_ref, is_page) in views_to_collect {
            let Some(view) = self.config.view(&view_ref) else {
                continue;
            };
            for (id, cmd) in &view.commands {
                if scope != crate::command::CommandScope::View {
                    continue;
                }
                let key = match &cmd.key {
                    Some(k) => Some(crate::input::Key::parse_binding(
                        &crate::workflow::config::normalize_key(k)?,
                    )?),
                    None => None,
                };
                let page = command_page_owner(context, snapshot);
                let owner = if is_page {
                    page.clone()
                } else {
                    command_owner(&self.config, &view_ref, context, snapshot)
                };
                let execution = crate::workflow::command::CommandExecution {
                    invocation: crate::workflow::command::CommandInvocation::view(
                        crate::workflow::command::CommandRef {
                            view: view_ref.clone(),
                            id: id.clone(),
                        },
                        cmd.clone(),
                    ),
                    context: crate::workflow::command::CommandContext {
                        page,
                        owner,
                        current: {
                            let mut current = snapshot
                                .publication
                                .as_ref()
                                .map(|p| p.current.clone())
                                .unwrap_or_else(|| {
                                    if snapshot.engine_type
                                        == crate::workflow::config::ENGINE_PICKER
                                    {
                                        serde_json::json!({ "input": snapshot.raw_input })
                                    } else {
                                        serde_json::Value::Null
                                    }
                                });
                            if let serde_json::Value::Object(ref mut map) = current {
                                if snapshot.engine_type == crate::workflow::config::ENGINE_PICKER {
                                    map.insert(
                                        "input".to_string(),
                                        serde_json::Value::String(snapshot.raw_input.clone()),
                                    );
                                }
                            }
                            current
                        },
                        engine_type: snapshot.engine_type.clone(),
                    },
                };
                let action = create_command_action(
                    std::sync::Arc::clone(&self.config),
                    std::sync::Arc::clone(&self.invocation),
                    self.cancellation.clone(),
                    context.instance,
                    execution,
                );
                entries.push(crate::command::CommandEntry::new(
                    id.clone(),
                    Some(cmd.label.clone()),
                    key,
                    scope,
                    action,
                ));
            }
        }
        Ok(entries)
    }
}

fn create_command_action(
    config: std::sync::Arc<crate::workflow::config::CompiledConfig>,
    invocation: std::sync::Arc<crate::workflow::InvocationContext>,
    cancellation: crate::lifecycle::CancellationToken,
    caller: ViewInstanceId,
    execution: crate::workflow::command::CommandExecution,
) -> std::sync::Arc<dyn crate::command::CommandAction> {
    std::sync::Arc::new(move || {
        let prepared = crate::workflow::command::prepare_command_action(
            &config,
            &invocation,
            execution.clone(),
            &cancellation,
        )
        .map_err(crate::view::operation_failure)?;
        map_prepared_action(&config, &invocation, &cancellation, prepared, caller)
            .map_err(crate::view::operation_failure)
    })
}

impl CommandService for ProtocolCommandService {
    fn build_host_commands(
        &self,
        shared_snapshot: std::sync::Arc<std::sync::RwLock<crate::command::ChromeSnapshot>>,
        registry: std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>>,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        let mut entries = Vec::new();
        let session_cmds = self.config.session_commands();

        if let Some(cmd) = session_cmds.get("commands") {
            let key = match &cmd.key {
                Some(k) => Some(crate::input::Key::parse_binding(
                    &crate::workflow::config::normalize_key(k)?,
                )?),
                None => None,
            };
            let label = cmd.label.clone();
            let target = self
                .config
                .resolve_view("__selectors:commands")
                .unwrap_or_else(|_| "__selectors:commands".to_string());

            let config_clone = std::sync::Arc::clone(&self.config);
            let snapshot_clone = std::sync::Arc::clone(&shared_snapshot);
            let registry_clone = std::sync::Arc::clone(&registry);
            let service_clone = std::sync::Arc::new(self.clone());
            let invocation_clone = std::sync::Arc::clone(&self.invocation);
            let cancellation_clone = self.cancellation.clone();
            let action = std::sync::Arc::new(move || {
                let snapshot = snapshot_clone.read().unwrap();
                let parameters = snapshot.to_picker_parameters();
                let active_caller = snapshot.active_instance.unwrap_or(ViewInstanceId(0));
                drop(snapshot);

                let nav = crate::workflow::command::NavigationRequest::new(target.clone(), "")
                    .with_parameters(parameters)
                    .with_presentation(crate::workflow::config::ViewPresentation {
                        mode: crate::workflow::config::ViewPresentationMode::Popup,
                        width: Some(72),
                        height: Some(16),
                    });
                let req = protocol_navigation_request(&config_clone, nav)?;

                let processor_service = std::sync::Arc::clone(&service_clone);
                let processor_registry = std::sync::Arc::clone(&registry_clone);
                let processor_snapshot = std::sync::Arc::clone(&snapshot_clone);
                let result_processor: CallResultProcessor =
                    std::sync::Arc::new(move |_source, caller, snapshot, result| {
                        if active_caller != ViewInstanceId(0) && caller.instance != active_caller {
                            return Ok(ViewDecision::Stay);
                        }
                        let command_id = result
                            .value
                            .as_str()
                            .or_else(|| result.value.get("id").and_then(|v| v.as_str()))
                            .or_else(|| {
                                result
                                    .value
                                    .get("ref")
                                    .and_then(|r| r.get("id"))
                                    .and_then(|v| v.as_str())
                            })
                            .map(str::to_owned);
                        let Some(command_id) = command_id else {
                            return Ok(ViewDecision::Stay);
                        };
                        let view_cmds = processor_service.build_view_commands(caller, snapshot)?;
                        let engine_cmds =
                            processor_service.build_engine_commands(caller, snapshot)?;
                        let mut registry = processor_registry.write().unwrap();
                        registry.replace_scope(crate::command::CommandScope::View, view_cmds)?;
                        registry
                            .replace_scope(crate::command::CommandScope::Engine, engine_cmds)?;
                        {
                            let mut snap = processor_snapshot.write().unwrap();
                            *snap = crate::command::ChromeSnapshot::from_registry(&registry)
                                .with_active_instance(Some(caller.instance))
                                .with_active_view(
                                    Some(caller.location.target.clone()),
                                    snapshot.parameters.clone(),
                                    snapshot.raw_input.clone(),
                                );
                        }
                        match registry.dispatch_id(&command_id) {
                            Ok(decision) => Ok(decision),
                            Err(_) => Ok(ViewDecision::Stay),
                        }
                    });
                let handler = std::sync::Arc::new(ProtocolCallReturnHandler {
                    config: std::sync::Arc::clone(&config_clone),
                    invocation: std::sync::Arc::clone(&invocation_clone),
                    cancellation: cancellation_clone.clone(),
                    origin: None,
                    context: None,
                    return_processor: None,
                    result_processor: Some(result_processor),
                });

                let boundary = CallBoundary {
                    caller: active_caller,
                    handler,
                };

                Ok(crate::view::ViewDecision::Transition(
                    crate::view::TransitionRequest::Call {
                        request: req,
                        continuation: crate::view::Continuation::Call(boundary),
                    },
                ))
            });

            entries.push(crate::command::CommandEntry::new(
                "commands",
                Some(label),
                key,
                crate::command::CommandScope::Host,
                action,
            ));
        }

        for (id, cmd) in session_cmds {
            if id == "commands" {
                continue;
            }
            let key = match &cmd.key {
                Some(k) => Some(crate::input::Key::parse_binding(
                    &crate::workflow::config::normalize_key(k)?,
                )?),
                None => None,
            };
            let config_clone = std::sync::Arc::clone(&self.config);
            let invocation_clone = std::sync::Arc::clone(&self.invocation);
            let cancellation_clone = self.cancellation.clone();
            let snapshot_clone = std::sync::Arc::clone(&shared_snapshot);
            let id_clone = id.clone();
            let cmd_clone = cmd.clone();
            let action = std::sync::Arc::new(move || {
                let active_snapshot = snapshot_clone.read().unwrap().clone();
                let active_view = active_snapshot
                    .active_view
                    .clone()
                    .unwrap_or_else(|| invocation_clone.root_view().to_string());
                let active_parameters = active_snapshot.active_parameters.clone();
                let active_raw_input = active_snapshot.active_raw_input.clone();
                let execution = crate::workflow::command::CommandExecution {
                    invocation: crate::workflow::command::CommandInvocation::session_command(
                        invocation_clone.root_view(),
                        &id_clone,
                        cmd_clone.clone(),
                    ),
                    context: crate::workflow::command::CommandContext {
                        page: crate::workflow::command::CommandOwnerContext {
                            view_ref: active_view.clone(),
                            parameters: crate::workflow::parameter::ParameterSnapshot::from_parts(
                                active_parameters.clone(),
                                active_raw_input.clone(),
                                crate::input::InputSourceIdentity {
                                    frame: crate::input::ViewMountId(0),
                                    generation: 0,
                                },
                                0,
                            ),
                        },
                        owner: crate::workflow::command::CommandOwnerContext {
                            view_ref: active_view,
                            parameters: crate::workflow::parameter::ParameterSnapshot::from_parts(
                                active_parameters,
                                active_raw_input,
                                crate::input::InputSourceIdentity {
                                    frame: crate::input::ViewMountId(0),
                                    generation: 0,
                                },
                                0,
                            ),
                        },
                        current: serde_json::Value::Null,
                        engine_type: "session".to_string(),
                    },
                };
                let prepared = crate::workflow::command::prepare_command_action(
                    &config_clone,
                    &invocation_clone,
                    execution,
                    &cancellation_clone,
                )
                .map_err(crate::view::operation_failure)?;
                let caller = snapshot_clone
                    .read()
                    .unwrap()
                    .active_instance
                    .unwrap_or(ViewInstanceId(1));
                map_prepared_action(
                    &config_clone,
                    &invocation_clone,
                    &cancellation_clone,
                    prepared,
                    caller,
                )
                .map_err(crate::view::operation_failure)
            });
            entries.push(crate::command::CommandEntry::new(
                id,
                Some(cmd.label),
                key,
                crate::command::CommandScope::Host,
                action,
            ));
        }

        Ok(entries)
    }

    fn build_view_commands(
        &self,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        self.build_commands_for_scope(context, snapshot, crate::command::CommandScope::View)
    }

    fn build_engine_commands(
        &self,
        _context: &ViewContext,
        _snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        Ok(Vec::new())
    }
}

fn command_owner(
    config: &crate::workflow::config::CompiledConfig,
    view_ref: &str,
    context: &ViewContext,
    snapshot: &ViewCommandSnapshot,
) -> crate::workflow::command::CommandOwnerContext {
    if view_ref == context.location.target {
        command_page_owner(context, snapshot)
    } else {
        let (values, raw_input) = if let Ok(state) = config.instantiate_parameters(view_ref) {
            (
                config
                    .parameter_values(&state)
                    .unwrap_or(serde_json::Value::Null),
                state.raw_input().to_string(),
            )
        } else {
            (snapshot.parameters.clone(), snapshot.raw_input.clone())
        };
        let parameters = crate::workflow::parameter::ParameterSnapshot::from_parts(
            values,
            raw_input,
            crate::input::InputSourceIdentity {
                frame: crate::input::ViewMountId(context.instance.0),
                generation: snapshot.revision,
            },
            snapshot.revision,
        );
        crate::workflow::command::CommandOwnerContext {
            view_ref: view_ref.to_string(),
            parameters,
        }
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
    }
}

type CallResultProcessor = std::sync::Arc<
    dyn Fn(&ViewLocation, &ViewContext, &ViewCommandSnapshot, &ViewResult) -> Result<ViewDecision>
        + Send
        + Sync,
>;

#[derive(Clone)]
struct ProtocolCallReturnHandler {
    config: std::sync::Arc<crate::workflow::config::CompiledConfig>,
    invocation: std::sync::Arc<crate::workflow::InvocationContext>,
    cancellation: crate::lifecycle::CancellationToken,
    origin: Option<crate::workflow::command::CommandOrigin>,
    context: Option<crate::workflow::command::CommandContext>,
    return_processor: Option<crate::workflow::config::ReturnProcessor>,
    result_processor: Option<CallResultProcessor>,
}

impl CallReturnHandler for ProtocolCallReturnHandler {
    fn post_commit(&self) -> bool {
        self.return_processor.is_some() || self.result_processor.is_some()
    }

    fn resume(
        &self,
        _source: &ViewLocation,
        caller: &ViewContext,
        _snapshot: &ViewCommandSnapshot,
        result: &ViewResult,
    ) -> Result<ViewDecision> {
        if let Some(processor) = &self.result_processor {
            return processor(_source, caller, _snapshot, result);
        }
        let (Some(origin), Some(context)) = (self.origin.clone(), self.context.clone()) else {
            return Ok(ViewDecision::Stay);
        };
        if let Some(processor) = &self.return_processor {
            let action = crate::workflow::command::prepare_return_processor(
                &self.config,
                &self.invocation,
                processor,
                origin,
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
            let is_parameter_form = call.request.view_ref == "__selectors:form";
            let request = protocol_navigation_request(config, call.request)?;
            let result_processor = is_parameter_form.then(|| {
                let config = std::sync::Arc::clone(config);
                std::sync::Arc::new(
                    move |_source: &ViewLocation,
                          _caller: &ViewContext,
                          _snapshot: &ViewCommandSnapshot,
                          result: &ViewResult|
                          -> anyhow::Result<ViewDecision> {
                        let value = result.value.clone();
                        let target = value
                            .get("target")
                            .and_then(|value| value.as_str())
                            .ok_or_else(|| {
                                anyhow::anyhow!("parameter form result has no target")
                            })?;
                        let parameters = value.get("parameters").cloned().ok_or_else(|| {
                            anyhow::anyhow!("parameter form result has no parameters")
                        })?;
                        config.validate_parameter_values(&target, &parameters)?;
                        let request =
                            crate::workflow::command::NavigationRequest::with_defaults(target)
                                .with_parameters(parameters);
                        let request = protocol_navigation_request(&config, request)?;
                        Ok(ViewDecision::Transition(
                            crate::view::TransitionRequest::Replace(request),
                        ))
                    },
                ) as CallResultProcessor
            });
            let boundary = CallBoundary {
                caller,
                handler: std::sync::Arc::new(ProtocolCallReturnHandler {
                    config: std::sync::Arc::clone(config),
                    invocation: std::sync::Arc::clone(invocation),
                    cancellation: cancellation.clone(),
                    origin: Some(call.origin),
                    context: Some(call.context),
                    return_processor: call.return_processor,
                    result_processor,
                }),
            };
            Ok(ViewDecision::Transition(TransitionRequest::Call {
                request,
                continuation: crate::view::Continuation::Call(boundary),
            }))
        }
        PreparedAction::Return { value } => {
            Ok(ViewDecision::Return(ViewResult::new(value)))
        }
        PreparedAction::Execute {
            prepared,
            exit,
            success_message,
        } => {
            let effect = ViewDecision::Effect(crate::view::EffectRequest::RunPrepared {
                prepared,
                success_message,
            });
            Ok(if exit {
                ViewDecision::Batch(vec![effect, ViewDecision::Exit])
            } else {
                effect
            })
        }
        PreparedAction::Feedback { message, level } => {
            Ok(ViewDecision::Effect(crate::view::EffectRequest::ShowFeedback {
                message,
                level,
            }))
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
        request.input.as_ref().map(|input| {
            let text = crate::terminal::sanitize_terminal_text(&input.params);
            let cursor = if text == input.params {
                input.cursor
            } else {
                crate::terminal::sanitize_terminal_text(&input.params[..input.cursor]).len()
            };
            (text, cursor)
        })
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
                success_message: Some("Copied to clipboard".into()),
            },
            ViewInstanceId(1),
        )
        .unwrap();

        assert_eq!(
            decision,
            ViewDecision::Effect(EffectRequest::RunPrepared {
                prepared,
                success_message: Some("Copied to clipboard".into())
            })
        );
    }
}
