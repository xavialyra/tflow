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

    fn is_view_dynamic(&self, target: &str) -> bool;

    fn update_active_snapshot(&self, snapshot: &ViewCommandSnapshot);
}

pub(crate) trait ViewCommandProvider: Send + Sync {
    fn is_dynamic(&self) -> bool;
    fn provide_commands(
        &self,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>>;
}

pub(crate) struct StaticViewCommandProvider {
    target: String,
    member_id: String,
    config: std::sync::Arc<crate::workflow::config::CompiledConfig>,
    invocation: std::sync::Arc<crate::workflow::InvocationContext>,
    cancellation: crate::lifecycle::CancellationToken,
    active_snapshot: std::sync::Arc<std::sync::RwLock<Option<ViewCommandSnapshot>>>,
}

impl ViewCommandProvider for StaticViewCommandProvider {
    fn is_dynamic(&self) -> bool {
        false
    }

    fn provide_commands(
        &self,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        let source = ViewCommandSource::new(
            &self.config,
            &self.invocation,
            &self.cancellation,
            &self.active_snapshot,
            context.instance,
            &self.target,
            &self.member_id,
        );
        let page = command_page_owner(context, snapshot);
        let declared = self
            .config
            .view(&self.target)
            .and_then(|view| view.keymap.as_ref())
            .is_some_and(|keymap| !keymap.bindings.is_empty());

        if declared {
            return Ok(source.declared_entries(&page));
        }

        // Without a keymap table every workflow command of the View's own
        // workflow is a candidate, keyed only where the command declares one.
        let mut seen_keys = std::collections::HashSet::new();
        let mut entries = Vec::new();
        for (fqid, cmd) in self.config.workflow_commands(&self.member_id) {
            let mut key = cmd.key.as_ref().and_then(|raw| {
                crate::workflow::config::normalize_key(raw)
                    .ok()
                    .and_then(|normalized| crate::input::Key::parse_binding(&normalized).ok())
            });
            if let Some(bound) = key
                && !seen_keys.insert(bound.binding_identity())
            {
                key = None;
            }
            let local_id = source.local_id(&fqid).to_string();
            if let Some(entry) = source.entry(&page, &fqid, local_id, key) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}

pub(crate) struct ItemViewCommandProvider {
    target: String,
    member_id: String,
    config: std::sync::Arc<crate::workflow::config::CompiledConfig>,
    invocation: std::sync::Arc<crate::workflow::InvocationContext>,
    cancellation: crate::lifecycle::CancellationToken,
    active_snapshot: std::sync::Arc<std::sync::RwLock<Option<ViewCommandSnapshot>>>,
}

impl ViewCommandProvider for ItemViewCommandProvider {
    fn is_dynamic(&self) -> bool {
        true
    }

    fn provide_commands(
        &self,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        let source = ViewCommandSource::new(
            &self.config,
            &self.invocation,
            &self.cancellation,
            &self.active_snapshot,
            context.instance,
            &self.target,
            &self.member_id,
        );
        let page = command_page_owner(context, snapshot);

        // The View's own bindings form the base layer and stay available even
        // when the item list is empty, so keys the View owns (route completion,
        // for example) do not depend on there being a focused item.
        let mut entries = source.declared_entries(&page);

        let Some(bindings) = snapshot
            .publication
            .as_ref()
            .and_then(|publ| {
                publ.current
                    .get("item")
                    .and_then(|item| item.get("bindings"))
                    .or_else(|| publ.current.get("bindings"))
            })
            .and_then(|bindings| bindings.as_object())
        else {
            return Ok(entries);
        };

        let mut seen_keys = std::collections::HashSet::new();

        for (key_str, val) in bindings {
            if val.as_bool() == Some(false) {
                continue;
            }
            let Some(cmd_target) = val.as_str() else {
                continue;
            };
            let mut key = parse_binding_key(key_str);
            if let Some(bound) = key {
                let identity = bound.binding_identity();
                if !seen_keys.insert(identity) {
                    key = None;
                } else {
                    // The focused item overrides the base binding for this key.
                    entries.retain(|entry| {
                        entry.key.map(|bound| bound.binding_identity()) != Some(identity)
                    });
                }
            }
            if let Some(entry) = source.entry(&page, cmd_target, cmd_target.to_string(), key) {
                entries.push(entry);
            }
        }

        Ok(entries)
    }
}

fn parse_binding_key(key_str: &str) -> Option<crate::input::Key> {
    if key_str.is_empty() {
        None
    } else {
        crate::input::Key::parse_binding(key_str).ok()
    }
}

/// Turns one declared keymap entry or focused-item binding into a View-scope
/// command entry. Both providers share the same wiring, so only the layer that
/// supplies the binding (View keymap vs. focused item) differs between them.
struct ViewCommandSource<'a> {
    config: &'a std::sync::Arc<crate::workflow::config::CompiledConfig>,
    invocation: &'a std::sync::Arc<crate::workflow::InvocationContext>,
    cancellation: &'a crate::lifecycle::CancellationToken,
    active_snapshot: &'a std::sync::Arc<std::sync::RwLock<Option<ViewCommandSnapshot>>>,
    caller: ViewInstanceId,
    target: &'a str,
    member_id: &'a str,
}

impl<'a> ViewCommandSource<'a> {
    fn new(
        config: &'a std::sync::Arc<crate::workflow::config::CompiledConfig>,
        invocation: &'a std::sync::Arc<crate::workflow::InvocationContext>,
        cancellation: &'a crate::lifecycle::CancellationToken,
        active_snapshot: &'a std::sync::Arc<std::sync::RwLock<Option<ViewCommandSnapshot>>>,
        caller: ViewInstanceId,
        target: &'a str,
        member_id: &'a str,
    ) -> Self {
        Self {
            config,
            invocation,
            cancellation,
            active_snapshot,
            caller,
            target,
            member_id,
        }
    }

    fn local_id<'b>(&self, cmd_target: &'b str) -> &'b str {
        cmd_target
            .strip_prefix(&format!("{}:", self.member_id))
            .unwrap_or(cmd_target)
    }

    /// `id` is the View-scope key, which also identifies the command reference.
    fn entry(
        &self,
        page: &crate::workflow::command::CommandOwnerContext,
        cmd_target: &str,
        id: String,
        key: Option<crate::input::Key>,
    ) -> Option<crate::command::CommandEntry> {
        let cmd = self.config.find_command(self.member_id, cmd_target)?;
        let cmd_wf = cmd_target
            .split_once(':')
            .map(|(workflow, _)| workflow)
            .unwrap_or(self.member_id);
        let action = create_command_action(
            std::sync::Arc::clone(self.config),
            std::sync::Arc::clone(self.invocation),
            self.cancellation.clone(),
            self.caller,
            std::sync::Arc::clone(self.active_snapshot),
            crate::workflow::command::CommandRef {
                view: self.target.to_string(),
                id: id.clone(),
            },
            cmd.clone(),
            page.clone(),
            cmd_wf.to_string(),
        );
        Some(crate::command::CommandEntry::new(
            id,
            Some(cmd.label.clone()),
            key,
            crate::command::CommandScope::View,
            action,
        ))
    }

    /// The View's own `[views.<name>.keymap]` table. A `mode = "static"` View
    /// publishes exactly these; a `mode = "item"` View publishes them as the
    /// base layer that the focused item's bindings override per physical key.
    fn declared_entries(
        &self,
        page: &crate::workflow::command::CommandOwnerContext,
    ) -> Vec<crate::command::CommandEntry> {
        let Some(keymap) = self
            .config
            .view(self.target)
            .and_then(|view| view.keymap.as_ref())
        else {
            return Vec::new();
        };
        keymap
            .bindings
            .iter()
            .filter(|(_, value)| value.as_bool() != Some(false))
            .filter_map(|(key_str, value)| {
                let cmd_target = value.as_str()?;
                let id = self.local_id(cmd_target).to_string();
                self.entry(page, cmd_target, id, parse_binding_key(key_str))
            })
            .collect()
    }
}

#[derive(Clone)]
pub(crate) struct ProtocolCommandService {
    config: std::sync::Arc<crate::workflow::config::CompiledConfig>,
    invocation: std::sync::Arc<crate::workflow::InvocationContext>,
    cancellation: crate::lifecycle::CancellationToken,
    active_snapshot: std::sync::Arc<std::sync::RwLock<Option<ViewCommandSnapshot>>>,
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
            active_snapshot: std::sync::Arc::new(std::sync::RwLock::new(None)),
        }
    }

    pub(crate) fn view_command_provider(&self, target: &str) -> Box<dyn ViewCommandProvider> {
        let member_id = crate::workflow::config::package_id(target);
        let is_item_mode = self
            .config
            .view(target)
            .and_then(|v| v.keymap.as_ref())
            .is_some_and(|k| k.mode == crate::workflow::config::KeymapMode::Item);
        if is_item_mode {
            Box::new(ItemViewCommandProvider {
                target: target.to_string(),
                member_id: member_id.to_string(),
                config: std::sync::Arc::clone(&self.config),
                invocation: std::sync::Arc::clone(&self.invocation),
                cancellation: self.cancellation.clone(),
                active_snapshot: std::sync::Arc::clone(&self.active_snapshot),
            })
        } else {
            Box::new(StaticViewCommandProvider {
                target: target.to_string(),
                member_id: member_id.to_string(),
                config: std::sync::Arc::clone(&self.config),
                invocation: std::sync::Arc::clone(&self.invocation),
                cancellation: self.cancellation.clone(),
                active_snapshot: std::sync::Arc::clone(&self.active_snapshot),
            })
        }
    }
}

fn create_command_action(
    config: std::sync::Arc<crate::workflow::config::CompiledConfig>,
    invocation: std::sync::Arc<crate::workflow::InvocationContext>,
    cancellation: crate::lifecycle::CancellationToken,
    caller: ViewInstanceId,
    active_snapshot: std::sync::Arc<std::sync::RwLock<Option<ViewCommandSnapshot>>>,
    command_ref: crate::workflow::command::CommandRef,
    command: crate::workflow::config::Command,
    page: crate::workflow::command::CommandOwnerContext,
    cmd_wf: String,
) -> std::sync::Arc<dyn crate::command::CommandAction> {
    std::sync::Arc::new(move || {
        let snap_opt = active_snapshot.read().unwrap().clone();
        if snap_opt.as_ref().is_some_and(|snap| {
            snap.engine_type == crate::workflow::config::ENGINE_PICKER
                && snap
                    .publication
                    .as_ref()
                    .is_some_and(|publication| !publication.ready)
        }) {
            return Ok(ViewDecision::Stay);
        }
        let (current, engine_type, owner) = if let Some(snap) = &snap_opt {
            let mut current = snap
                .publication
                .as_ref()
                .map(|p| p.current.clone())
                .unwrap_or_else(|| {
                    if snap.engine_type == crate::workflow::config::ENGINE_PICKER {
                        serde_json::json!({ "input": snap.raw_input })
                    } else {
                        serde_json::Value::Null
                    }
                });
            if let serde_json::Value::Object(ref mut map) = current
                && snap.engine_type == crate::workflow::config::ENGINE_PICKER
            {
                map.insert(
                    "input".to_string(),
                    serde_json::Value::String(snap.raw_input.clone()),
                );
            }
            let live_owner = command_owner_from_snap(&config, &cmd_wf, caller, snap, &page);
            (current, snap.engine_type.clone(), live_owner)
        } else {
            (serde_json::Value::Null, "view".to_string(), page.clone())
        };

        let execution = crate::workflow::command::CommandExecution {
            invocation: crate::workflow::command::CommandInvocation::view(
                command_ref.clone(),
                command.clone(),
            ),
            context: crate::workflow::command::CommandContext {
                page: page.clone(),
                owner,
                current,
                engine_type,
            },
        };
        let prepared = crate::workflow::command::prepare_command_action(
            &config,
            &invocation,
            execution,
            &cancellation,
        )
        .map_err(crate::view::operation_failure)?;
        map_prepared_action(&config, &invocation, &cancellation, prepared, caller)
            .map_err(crate::view::operation_failure)
    })
}

fn command_owner_from_snap(
    config: &crate::workflow::config::CompiledConfig,
    cmd_wf: &str,
    caller: ViewInstanceId,
    snap: &ViewCommandSnapshot,
    page: &crate::workflow::command::CommandOwnerContext,
) -> crate::workflow::command::CommandOwnerContext {
    let member_id = crate::workflow::config::package_id(&page.view_ref);
    if cmd_wf == member_id {
        page.clone()
    } else {
        let (values, raw_input) = if let Ok(state) = config.instantiate_parameters(cmd_wf) {
            (
                config
                    .parameter_values(&state)
                    .unwrap_or(serde_json::Value::Null),
                state.raw_input().to_string(),
            )
        } else {
            (snap.parameters.clone(), snap.raw_input.clone())
        };
        let parameters = crate::workflow::parameter::ParameterSnapshot::from_parts(
            values,
            raw_input,
            crate::input::InputSourceIdentity {
                frame: crate::input::ViewMountId(caller.0),
                generation: snap.revision,
            },
            snap.revision,
        );
        crate::workflow::command::CommandOwnerContext {
            view_ref: cmd_wf.to_string(),
            parameters,
        }
    }
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
                .resolve_view("__commands:main")
                .unwrap_or_else(|_| "__commands:main".to_string());

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
                        processor_service.update_active_snapshot(snapshot);
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

    fn is_view_dynamic(&self, target: &str) -> bool {
        self.view_command_provider(target).is_dynamic()
    }

    fn build_view_commands(
        &self,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        self.update_active_snapshot(snapshot);
        let provider = self.view_command_provider(&context.location.target);
        provider.provide_commands(context, snapshot)
    }

    fn build_engine_commands(
        &self,
        _context: &ViewContext,
        _snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        Ok(Vec::new())
    }

    fn update_active_snapshot(&self, snapshot: &ViewCommandSnapshot) {
        *self.active_snapshot.write().unwrap() = Some(snapshot.clone());
    }
}

pub(crate) fn command_page_owner(
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
        PreparedAction::Navigate {
            request,
            mode,
            clear_input,
        } => {
            let request = protocol_navigation_request(config, request)?;
            let transition = ViewDecision::Transition(match mode {
                crate::workflow::command::NavigationMode::Push => TransitionRequest::Push(request),
                crate::workflow::command::NavigationMode::Replace => {
                    TransitionRequest::Replace(request)
                }
            });
            if clear_input {
                Ok(ViewDecision::Batch(vec![
                    ViewDecision::ClearInput,
                    transition,
                ]))
            } else {
                Ok(transition)
            }
        }
        PreparedAction::Call(call) => {
            let is_parameter_form = call.request.view_ref == "__form:main";
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
                        config.validate_parameter_values(target, &parameters)?;
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
        PreparedAction::Return { value } => Ok(ViewDecision::Return(ViewResult::new(value))),
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
        PreparedAction::Feedback { message, level } => Ok(ViewDecision::Effect(
            crate::view::EffectRequest::ShowFeedback { message, level },
        )),
    }
}

fn protocol_navigation_request(
    config: &crate::workflow::config::CompiledConfig,
    request: crate::workflow::command::NavigationRequest,
) -> anyhow::Result<crate::view::NavigationRequest> {
    let mut state = config.instantiate_parameters(&request.view_ref)?;
    config.sanitize_initial_parameter_values(&mut state)?;

    let focus = request.parameters.as_ref().and_then(|parameters| {
        parameters.as_object().and_then(|object| {
            object
                .get("__engine")
                .and_then(|engine| engine.get("focus"))
                .or_else(|| object.get("__focus"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
    });

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
    if let Some(focus) = focus {
        protocol_request = protocol_request.with_focus(focus);
    }
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

    #[test]
    fn reserved_focus_parameters_seed_navigation_focus_and_do_not_leak_into_the_query() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        for parameters in [
            serde_json::json!({"__focus": "app"}),
            serde_json::json!({"__engine": {"focus": "app"}}),
        ] {
            let request = crate::workflow::command::NavigationRequest::new("core:default", "")
                .with_parameters(parameters);
            let resolved = protocol_navigation_request(&config, request).unwrap();
            assert_eq!(resolved.focus.as_deref(), Some("app"));
            let values = resolved
                .query
                .values
                .as_object()
                .cloned()
                .unwrap_or_default();
            assert!(
                !values.keys().any(|key| key.starts_with("__")),
                "reserved keys must not leak into the query: {values:?}"
            );
        }
    }

    /// The fixture's `core:default` is `mode = "item"` with a base keymap.
    fn item_mode_snapshot(
        config: &Arc<crate::workflow::config::CompiledConfig>,
        item_bindings: Option<serde_json::Value>,
    ) -> ViewCommandSnapshot {
        let parameters = config.instantiate_parameters("core:default").unwrap();
        let publication = match item_bindings {
            Some(bindings) => serde_json::json!({
                "input": "pass",
                "item": {"bindings": bindings.clone()},
                "bindings": bindings,
            }),
            None => serde_json::json!({"input": "pass", "item": null, "bindings": {}}),
        };
        ViewCommandSnapshot {
            engine_type: crate::workflow::config::ENGINE_PICKER.to_string(),
            parameters: config.parameter_values(&parameters).unwrap(),
            raw_input: "pass".to_string(),
            runtime: serde_json::Value::Null,
            publication: Some(crate::view::ViewPublication::new(publication, true)),
            revision: 1,
        }
    }

    fn item_mode_service(
        config: &Arc<crate::workflow::config::CompiledConfig>,
    ) -> ProtocolCommandService {
        let invocation = Arc::new(
            crate::workflow::InvocationContext::new(
                "core:default".to_string(),
                serde_json::Value::Null,
                config.instantiate_parameters("core:default").unwrap(),
            )
            .unwrap(),
        );
        ProtocolCommandService::new(
            Arc::clone(config),
            invocation,
            crate::lifecycle::CancellationToken::new(),
        )
    }

    fn bound_key(entry: &crate::command::CommandEntry) -> Option<String> {
        entry.key.and_then(crate::input::Key::binding_name)
    }

    /// Item-mode base bindings must not depend on a focused item, or a query
    /// that filters the list down to nothing would unbind them again.
    #[test]
    fn item_mode_base_bindings_apply_without_a_focused_item() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let service = item_mode_service(&config);
        let context = ViewContext::new(ViewInstanceId(50), "core:default");
        let snapshot = item_mode_snapshot(&config, None);

        let entries = service.build_view_commands(&context, &snapshot).unwrap();
        let keys: Vec<Option<String>> = entries.iter().map(bound_key).collect();
        assert!(keys.contains(&Some("tab".to_string())), "{keys:?}");
        assert!(keys.contains(&Some("space".to_string())), "{keys:?}");

        // The View scope rejects duplicate keys, so publishing it must be valid.
        crate::command::CommandRegistry::new()
            .replace_scope(crate::command::CommandScope::View, entries)
            .unwrap();
    }

    #[test]
    fn a_focused_item_overrides_the_item_mode_base_binding() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let service = item_mode_service(&config);
        let context = ViewContext::new(ViewInstanceId(51), "core:default");
        let snapshot = item_mode_snapshot(
            &config,
            Some(serde_json::json!({"tab": "core:accept"})),
        );

        let entries = service.build_view_commands(&context, &snapshot).unwrap();
        let bound: Vec<(String, Option<String>)> = entries
            .iter()
            .map(|entry| (entry.id.clone(), bound_key(entry)))
            .collect();
        let tab = bound
            .iter()
            .filter(|(_, key)| key.as_deref() == Some("tab"))
            .collect::<Vec<_>>();
        assert_eq!(tab.len(), 1, "the base binding must be replaced: {bound:?}");
        assert_eq!(tab[0].0, "core:accept");
        // The base key the item did not claim survives.
        assert!(bound.iter().any(|(_, key)| key.as_deref() == Some("space")));

        crate::command::CommandRegistry::new()
            .replace_scope(crate::command::CommandScope::View, entries)
            .unwrap();
    }
}
