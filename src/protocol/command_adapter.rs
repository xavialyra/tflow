use crate::protocol::contracts::ViewInstanceId;
use crate::view::{
    CallBoundary, CallReturnHandler, ViewCommandSnapshot, ViewContext, ViewDecision, ViewLocation,
    ViewResult,
};
use anyhow::Result;

pub(crate) trait CommandService {
    fn registry(&self) -> std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>>;

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

    fn is_view_dynamic(&self, target: &str) -> bool;

    fn chrome_commands_show(&self, target: Option<&str>) -> Result<Vec<String>>;

    fn unbind_rules(&self, target: Option<&str>) -> Result<crate::command::UnbindRules>;

    fn update_active_snapshot(&self, snapshot: &ViewCommandSnapshot);

    fn host_companion_keys(&self) -> Vec<crate::input::Key> {
        Vec::new()
    }

    /// Executes a registry entry whose handler is resolved at dispatch time.
    fn execute_entry(
        &self,
        entry: &crate::command::CommandEntry,
        caller: ViewInstanceId,
        expected_call_revision: Option<u64>,
    ) -> Result<ViewDecision>;
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
}

impl ViewCommandProvider for StaticViewCommandProvider {
    fn is_dynamic(&self) -> bool {
        false
    }

    fn provide_commands(
        &self,
        _context: &ViewContext,
        _snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        let source = ViewCommandSource::new(&self.config, &self.target, &self.member_id);
        let declared = self
            .config
            .view(&self.target)
            .and_then(|view| view.bindings.as_ref())
            .is_some_and(|bindings| !bindings.is_empty());

        if declared {
            return Ok(source.declared_entries());
        }

        // Without a bindings table every workflow command of the View's own
        // workflow is a candidate without a physical shortcut.
        let mut entries = Vec::new();
        for (fqid, _cmd) in self.config.workflow_commands(&self.member_id) {
            if let Some(entry) = source.entry(&fqid, None) {
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
}

impl ViewCommandProvider for ItemViewCommandProvider {
    fn is_dynamic(&self) -> bool {
        true
    }

    fn provide_commands(
        &self,
        _context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        let source = ViewCommandSource::new(&self.config, &self.target, &self.member_id);

        // The View's own bindings form the base layer and stay available even
        // when the item list is empty, so keys the View owns (route completion,
        // for example) do not depend on there being a focused item.
        let mut entries = source.declared_entries();

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
            // Item bindings are command references: a non-string value names no
            // command, so an item can rebind a base key but never remove one.
            // Removing a base binding is the View's own decision
            // (`[views.<name>.unbind]`).
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
            if let Some(entry) = source.entry(cmd_target, key) {
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

/// Turns one declared bindings entry or focused-item binding into a View-layer
/// command entry. Both providers share the same wiring, so only the layer that
/// supplies the binding (View bindings vs. focused item) differs between them.
/// The entry stores only plain data; the owning View is recorded so the handler
/// can be resolved at dispatch time.
struct ViewCommandSource<'a> {
    config: &'a std::sync::Arc<crate::workflow::config::CompiledConfig>,
    target: &'a str,
    member_id: &'a str,
}

impl<'a> ViewCommandSource<'a> {
    fn new(
        config: &'a std::sync::Arc<crate::workflow::config::CompiledConfig>,
        target: &'a str,
        member_id: &'a str,
    ) -> Self {
        Self {
            config,
            target,
            member_id,
        }
    }

    /// Resolves one binding address into a View-layer command entry.
    ///
    /// The entry id is the resolved FQID, so the whole registry addresses every
    /// command the same way (`<workflow>.<command>` or `<engine>.<action>`): a
    /// command reached from the declared table, from a focused item's bindings,
    /// from `unbind.commands`, or from `--inspect` carries one single id, and
    /// the same command cannot appear twice under two spellings.
    fn entry(
        &self,
        address: &str,
        key: Option<crate::input::Key>,
    ) -> Option<crate::command::CommandEntry> {
        let resolved = self.config.resolve_address(self.member_id, address)?;
        Some(match resolved {
            crate::workflow::config::ResolvedAddress::Engine { fqid, label } => {
                crate::command::CommandEntry::new(
                    fqid,
                    Some(label.to_string()),
                    key,
                    crate::command::BindingLayer::View,
                )
            }
            crate::workflow::config::ResolvedAddress::Command { fqid, label } => {
                crate::command::CommandEntry::new(
                    fqid,
                    Some(label),
                    key,
                    crate::command::BindingLayer::View,
                )
            }
        })
    }

    /// The View's own `[views.<name>.bindings]` table. A `binding_mode = "view"`
    /// View publishes exactly these; a `binding_mode = "item_merge"` View
    /// publishes them as the base layer that the focused item's bindings
    /// override per physical key.
    fn declared_entries(&self) -> Vec<crate::command::CommandEntry> {
        let Some(bindings) = self
            .config
            .view(self.target)
            .and_then(|view| view.bindings.as_ref())
        else {
            return Vec::new();
        };
        bindings
            .iter()
            .filter_map(|(key_str, value)| {
                let cmd_target = value.as_str()?;
                self.entry(cmd_target, parse_binding_key(key_str))
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
    registry: std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>>,
    shared_snapshot: std::sync::Arc<
        std::sync::RwLock<
            Option<std::sync::Arc<std::sync::RwLock<crate::command::ChromeSnapshot>>>,
        >,
    >,
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
            registry: std::sync::Arc::new(std::sync::RwLock::new(
                crate::command::CommandRegistry::new(),
            )),
            shared_snapshot: std::sync::Arc::new(std::sync::RwLock::new(None)),
        }
    }

    /// The session command registry handle, shared with the host so engines can
    /// publish the live command projection.
    pub(crate) fn registry_handle(
        &self,
    ) -> std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>> {
        std::sync::Arc::clone(&self.registry)
    }

    pub(crate) fn view_command_provider(&self, target: &str) -> Box<dyn ViewCommandProvider> {
        let member_id = crate::workflow::config::package_id(target);
        let is_item_mode = self
            .config
            .view(target)
            .is_some_and(|v| v.binding_mode == crate::workflow::config::BindingMode::ItemMerge);
        if is_item_mode {
            Box::new(ItemViewCommandProvider {
                target: target.to_string(),
                member_id: member_id.to_string(),
                config: std::sync::Arc::clone(&self.config),
            })
        } else {
            Box::new(StaticViewCommandProvider {
                target: target.to_string(),
                member_id: member_id.to_string(),
                config: std::sync::Arc::clone(&self.config),
            })
        }
    }
}

impl ProtocolCommandService {
    /// Resolves the entry's handler at dispatch time and executes it.
    ///
    /// The registry stores only plain data, so the invocation is rebuilt from
    /// the live snapshot here. Depth is bounded so a workflow cannot recurse
    /// through `invoke-command` without limit.
    pub(crate) fn execute_entry(
        &self,
        entry: &crate::command::CommandEntry,
        caller: ViewInstanceId,
        expected_call_revision: Option<u64>,
    ) -> Result<ViewDecision> {
        let depth = crate::command::command_call_depth().saturating_add(1);
        crate::command::execute_at_depth(depth, || {
            if entry.id == crate::command::OPEN_COMPANION {
                return Ok(crate::view::ViewDecision::OpenCompanion);
            }
            let prepared = self
                .prepare_entry(entry, caller)
                .map_err(crate::view::operation_failure)?;
            match prepared {
                // A View whose feed is still loading keeps its own commands
                // inert, exactly as the previous pre-baked closure did.
                EntryExecution::Deferred => Ok(crate::view::ViewDecision::Stay),
                EntryExecution::Engine => {
                    // An engine action is a key event, not a definition: the
                    // instance this decision lands on lets its own engine run it.
                    Ok(crate::view::ViewDecision::EngineAction(entry.id.clone()))
                }
                EntryExecution::Command(action) => {
                    map_prepared_action(self, action, caller, expected_call_revision)
                        .map_err(crate::view::operation_failure)
                }
            }
        })
    }

    fn active_snapshot_is_loading(&self) -> bool {
        self.active_snapshot
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|snap| {
                snap.engine_type == crate::workflow::config::ENGINE_PICKER
                    && snap
                        .publication
                        .as_ref()
                        .is_some_and(|publication| !publication.ready)
            })
    }

    /// The one place that decides how a registry entry is executed.
    ///
    /// The binding layer only controls priority. The id selects either a
    /// built-in engine action or a workflow command; execution context always
    /// comes from the currently active View.
    fn prepare_entry(
        &self,
        entry: &crate::command::CommandEntry,
        caller: ViewInstanceId,
    ) -> Result<EntryExecution> {
        if entry.layer == crate::command::BindingLayer::View && self.active_snapshot_is_loading() {
            return Ok(EntryExecution::Deferred);
        }
        if crate::engine::engine_action_from_id(&entry.id).is_some() {
            return Ok(EntryExecution::Engine);
        }
        let execution = self.command_execution(entry, caller)?;
        Ok(EntryExecution::Command(
            crate::workflow::command::prepare_command_action(
                &self.config,
                &self.invocation,
                execution,
                &self.cancellation,
            )?,
        ))
    }

    fn command_execution(
        &self,
        entry: &crate::command::CommandEntry,
        caller: ViewInstanceId,
    ) -> Result<crate::workflow::command::CommandExecution> {
        let chrome = self
            .shared_snapshot
            .read()
            .unwrap()
            .clone()
            .map(|snapshot| snapshot.read().unwrap().clone())
            .unwrap_or_default();
        let active_view = chrome
            .active_view
            .clone()
            .unwrap_or_else(|| self.invocation.root_view().to_string());
        let command = self
            .config
            .find_command(crate::workflow::config::package_id(&active_view), &entry.id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("command {:?} is no longer configured", entry.id))?;
        let command_ref = crate::workflow::command::CommandRef {
            id: entry.id.clone(),
            revision: self.registry.read().unwrap().revision(),
        };
        let snap_opt = self.active_snapshot.read().unwrap().clone();
        let (current, engine_type, page) = if let Some(snap) = &snap_opt {
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
            (
                current,
                snap.engine_type.clone(),
                crate::workflow::command::CommandOwnerContext {
                    view_ref: active_view.clone(),
                    parameters: crate::workflow::parameter::ParameterSnapshot::from_parts(
                        snap.parameters.clone(),
                        snap.raw_input.clone(),
                        crate::input::InputSourceIdentity {
                            frame: crate::input::ViewMountId(caller.0),
                            generation: snap.revision,
                        },
                        snap.revision,
                    ),
                },
            )
        } else {
            (
                serde_json::Value::Null,
                "session".to_string(),
                crate::workflow::command::CommandOwnerContext {
                    view_ref: active_view.clone(),
                    parameters: crate::workflow::parameter::ParameterSnapshot::from_parts(
                        chrome.active_parameters,
                        chrome.active_raw_input,
                        crate::input::InputSourceIdentity {
                            frame: crate::input::ViewMountId(caller.0),
                            generation: 0,
                        },
                        0,
                    ),
                },
            )
        };
        let commands =
            crate::command::ChromeSnapshot::from_registry(&self.registry.read().unwrap())
                .runtime_envelope();
        Ok(crate::workflow::command::CommandExecution {
            invocation: crate::workflow::command::CommandInvocation::view(
                active_view,
                command_ref,
                command,
            ),
            context: crate::workflow::command::CommandContext {
                page: page.clone(),
                owner: page,
                current,
                engine_type,
                commands,
            },
        })
    }
}

impl CommandService for ProtocolCommandService {
    fn host_companion_keys(&self) -> Vec<crate::input::Key> {
        self.config
            .host_bindings()
            .iter()
            .filter(|(_, id)| *id == crate::command::OPEN_COMPANION)
            .filter_map(|(key, _)| crate::input::Key::parse_binding(key).ok())
            .collect()
    }

    fn registry(&self) -> std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>> {
        std::sync::Arc::clone(&self.registry)
    }

    fn build_host_commands(
        &self,
        shared_snapshot: std::sync::Arc<std::sync::RwLock<crate::command::ChromeSnapshot>>,
        _registry: std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>>,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        *self.shared_snapshot.write().unwrap() = Some(std::sync::Arc::clone(&shared_snapshot));
        let mut entries = Vec::new();
        for (key_name, id) in self.config.host_bindings() {
            let id = id.clone();
            let label = crate::command::host_action_label(&id)
                .map(str::to_string)
                .or_else(|| {
                    self.config
                        .find_command(&self.config.entrypoint, &id)
                        .map(|cmd| cmd.label.clone())
                })
                .ok_or_else(|| anyhow::anyhow!("host command {:?} is missing", id))?;
            let key = Some(crate::input::Key::parse_binding(key_name)?);
            entries.push(crate::command::CommandEntry::new(
                id,
                Some(label),
                key,
                crate::command::BindingLayer::Host,
            ));
        }

        Ok(entries)
    }

    fn is_view_dynamic(&self, target: &str) -> bool {
        self.view_command_provider(target).is_dynamic()
    }

    fn chrome_commands_show(&self, target: Option<&str>) -> Result<Vec<String>> {
        target
            .map(|target| self.config.chrome_commands_show(target))
            .unwrap_or_else(|| Ok(vec!["enter".to_string(), "ctrl+k".to_string()]))
    }

    fn unbind_rules(&self, target: Option<&str>) -> Result<crate::command::UnbindRules> {
        let Some((member_id, view)) =
            target.and_then(|target| self.config.view(target).map(|view| (target, view)))
        else {
            return Ok(crate::command::UnbindRules::default());
        };
        let member_id = crate::workflow::config::package_id(member_id);
        let mut rules = crate::command::UnbindRules::default();
        for layer in &view.unbind.layers {
            let layer = crate::command::BindingLayer::from_layer(layer)
                .ok_or_else(|| anyhow::anyhow!("unknown unbind layer {layer:?}"))?;
            rules.layers.insert(layer);
        }
        for address in &view.unbind.commands {
            let resolved = self
                .config
                .resolve_address(member_id, address)
                .ok_or_else(|| anyhow::anyhow!("unbind command {address:?} is not configured"))?;
            // Entry ids are FQIDs, so a command address needs no rewriting.
            rules.commands.insert(resolved.fqid().to_string());
        }
        for key in &view.unbind.keys {
            rules
                .keys
                .insert(crate::input::Key::parse_binding(key)?.binding_identity());
        }
        Ok(rules)
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

    fn update_active_snapshot(&self, snapshot: &ViewCommandSnapshot) {
        *self.active_snapshot.write().unwrap() = Some(snapshot.clone());
    }

    fn execute_entry(
        &self,
        entry: &crate::command::CommandEntry,
        caller: ViewInstanceId,
        expected_call_revision: Option<u64>,
    ) -> Result<ViewDecision> {
        ProtocolCommandService::execute_entry(self, entry, caller, expected_call_revision)
    }
}

type CallResultProcessor = std::sync::Arc<
    dyn Fn(&ViewLocation, &ViewContext, &ViewCommandSnapshot, &ViewResult) -> Result<ViewDecision>
        + Send
        + Sync,
>;

#[derive(Clone)]
struct ProtocolCallReturnHandler {
    commands: ProtocolCommandService,
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
        snapshot: &ViewCommandSnapshot,
        result: &ViewResult,
    ) -> Result<ViewDecision> {
        if let Some(processor) = &self.result_processor {
            return processor(_source, caller, snapshot, result);
        }
        let (Some(origin), Some(context)) = (self.origin.clone(), self.context.clone()) else {
            return Ok(ViewDecision::Stay);
        };
        let view_entries = self.commands.build_view_commands(caller, snapshot)?;
        self.commands
            .registry
            .write()
            .unwrap()
            .replace_layer(crate::command::BindingLayer::View, view_entries)?;

        if let Some(snap_ref) = self.commands.shared_snapshot.read().unwrap().as_ref() {
            let mut snap = snap_ref.write().unwrap();
            snap.active_instance = Some(caller.instance);
            snap.active_view = Some(caller.location.target.clone());
            snap.active_parameters = snapshot.parameters.clone();
            snap.active_raw_input = snapshot.raw_input.clone();
        }

        // The palette snapshot carries one registry revision on its envelope.
        let expected_call_revision = context
            .commands
            .get("revision")
            .and_then(serde_json::Value::as_u64);

        if let Some(processor) = &self.return_processor {
            let action = crate::workflow::command::prepare_return_processor(
                &self.commands.config,
                &self.commands.invocation,
                processor,
                origin,
                context,
                caller,
                result,
                &self.commands.cancellation,
            )?;
            return map_prepared_action(
                &self.commands,
                action,
                caller.instance,
                expected_call_revision,
            );
        }
        Ok(ViewDecision::Stay)
    }
}

/// What executing a registry entry turns into.
enum EntryExecution {
    /// A declared command's operation, mapped to a decision by the protocol layer.
    Command(crate::workflow::command::PreparedAction),
    /// An engine action: no definition to run, the View's engine owns it.
    Engine,
    /// A View command while that View's feed is still loading: inert, like the
    /// pre-baked closure it replaced.
    Deferred,
}

pub(crate) fn map_prepared_action(
    commands: &ProtocolCommandService,
    action: crate::workflow::command::PreparedAction,
    caller: ViewInstanceId,
    expected_call_revision: Option<u64>,
) -> anyhow::Result<crate::view::ViewDecision> {
    use crate::view::{TransitionRequest, ViewDecision, ViewResult};
    use crate::workflow::command::PreparedAction;
    match action {
        PreparedAction::Navigate {
            request,
            mode,
            clear_input,
        } => {
            let request = protocol_navigation_request(&commands.config, request)?;
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
            let request = protocol_navigation_request(&commands.config, call.request)?;
            let boundary = CallBoundary {
                caller,
                handler: std::sync::Arc::new(ProtocolCallReturnHandler {
                    commands: commands.clone(),
                    origin: Some(call.origin),
                    context: Some(call.context),
                    return_processor: call.return_processor,
                    result_processor: None,
                }),
            };
            Ok(ViewDecision::Transition(TransitionRequest::Call {
                request,
                continuation: crate::view::Continuation::Call(boundary),
            }))
        }
        PreparedAction::Return { value } => Ok(ViewDecision::Return(ViewResult::new(value))),
        PreparedAction::Companion { target, query } => {
            let (target, query) = if let Some(query) = query {
                let request = protocol_navigation_request(
                    &commands.config,
                    crate::workflow::command::NavigationRequest {
                        view_ref: target,
                        input: None,
                        parameters: Some(query),
                        presentation: Default::default(),
                    },
                )?;
                (request.target, Some(request.query.values))
            } else {
                let request = protocol_navigation_request(
                    &commands.config,
                    crate::workflow::command::NavigationRequest {
                        view_ref: target,
                        input: None,
                        parameters: None,
                        presentation: Default::default(),
                    },
                )?;
                (request.target, None)
            };
            Ok(ViewDecision::ToggleCompanion { target, query })
        }
        PreparedAction::InvokeCommand { command } => {
            let entry = {
                let registry = commands.registry.read().unwrap();
                let current_rev = registry.revision();
                let matches_current = command.revision == current_rev;
                let matches_call =
                    expected_call_revision.is_some_and(|rev| command.revision == rev);
                anyhow::ensure!(
                    command.revision == 0 || matches_current || matches_call,
                    "command reference is stale: expected revision {}, current revision {}",
                    command.revision,
                    current_rev
                );
                registry.resolve_id(&command.id).cloned().ok_or_else(|| {
                    anyhow::anyhow!("command {:?} is no longer available", command.id)
                })?
            };
            commands.execute_entry(&entry, caller, Some(command.revision))
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
        PreparedAction::Noop => Ok(ViewDecision::Stay),
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

    /// A `[views.*.bindings]` value addresses an engine action explicitly as
    /// `@engine:<engine>.<action>` (a View-layer engine action); a bare name
    /// only ever resolves to a command of the current workflow. `dmenu:main`
    /// binds `escape = "@engine:picker.exit"`.
    #[test]
    fn a_view_binding_addresses_an_engine_action_explicitly() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let member = crate::workflow::config::package_id("dmenu:main");
        let source = ViewCommandSource::new(&config, "dmenu:main", member);
        let entries = source.declared_entries();

        let exit = entries
            .iter()
            .find(|entry| entry.id == "picker.exit")
            .expect("@engine:picker.exit resolves to the engine action");
        assert_eq!(exit.layer, crate::command::BindingLayer::View);
        assert_eq!(exit.label.as_deref(), Some("Exit"));
        assert_eq!(
            exit.key.and_then(crate::input::Key::binding_name),
            Some("escape".to_string())
        );

        // A workflow command in the same table is still indexed by its FQID.
        let accept = entries
            .iter()
            .find(|entry| entry.id == "dmenu.accept")
            .expect("enter = \"accept\" resolves to the workflow command");
        assert_eq!(accept.layer, crate::command::BindingLayer::View);

        // A bare engine-action name is not a command and has no engine sigil,
        // so it resolves to nothing (validation rejects it earlier).
        assert!(source.entry("exit", None).is_none());

        // `@workflow:<command>` names a workflow command explicitly.
        let explicit = source
            .entry("@workflow:accept", None)
            .expect("@workflow:accept resolves to the workflow command");
        assert_eq!(explicit.id, "dmenu.accept");
    }

    #[test]
    fn invoke_command_routes_engine_entries_and_rejects_stale_revisions() {
        let config = std::sync::Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let invocation = std::sync::Arc::new(
            crate::workflow::InvocationContext::new(
                "core:default".to_string(),
                serde_json::Value::Null,
                config.instantiate_parameters("core:default").unwrap(),
            )
            .unwrap(),
        );
        let service = ProtocolCommandService::new(
            config,
            invocation,
            crate::lifecycle::CancellationToken::new(),
        );
        let revision = {
            let mut registry = service.registry.write().unwrap();
            registry
                .replace_layer(
                    crate::command::BindingLayer::Engine,
                    vec![crate::command::CommandEntry::for_event(
                        "picker.select_next",
                        Some("Select Next".to_string()),
                        None,
                        crate::command::BindingLayer::Engine,
                    )],
                )
                .unwrap();
            registry.revision()
        };

        let decision = map_prepared_action(
            &service,
            crate::workflow::command::PreparedAction::InvokeCommand {
                command: crate::workflow::command::CommandRef {
                    id: "picker.select_next".to_string(),
                    revision,
                },
            },
            crate::protocol::contracts::ViewInstanceId(1),
            None,
        )
        .expect("an engine action is invokable by reference");
        assert_eq!(
            decision,
            crate::view::ViewDecision::EngineAction("picker.select_next".to_string()),
            "the instance the decision lands on lets its own engine run the action"
        );

        let stale = map_prepared_action(
            &service,
            crate::workflow::command::PreparedAction::InvokeCommand {
                command: crate::workflow::command::CommandRef {
                    id: "picker.select_next".to_string(),
                    revision: revision.saturating_add(100),
                },
            },
            crate::protocol::contracts::ViewInstanceId(1),
            None,
        )
        .expect_err("a stale command reference must be rejected");
        assert!(stale.to_string().contains("stale"));
    }

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
            timeout: None,
        };

        let service = ProtocolCommandService::new(
            config,
            invocation,
            crate::lifecycle::CancellationToken::new(),
        );
        let decision = map_prepared_action(
            &service,
            crate::workflow::command::PreparedAction::Execute {
                prepared: prepared.clone(),
                exit: false,
                success_message: Some("Copied to clipboard".into()),
            },
            ViewInstanceId(1),
            None,
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

    #[test]
    fn cross_workflow_view_command_keeps_the_callers_live_parameters() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let invocation = Arc::new(
            crate::workflow::InvocationContext::new(
                "core:default".to_string(),
                serde_json::Value::Null,
                config.instantiate_parameters("core:default").unwrap(),
            )
            .unwrap(),
        );
        let service = ProtocolCommandService::new(
            Arc::clone(&config),
            invocation,
            crate::lifecycle::CancellationToken::new(),
        );
        let snapshot = ViewCommandSnapshot {
            engine_type: crate::workflow::config::ENGINE_PICKER.to_string(),
            parameters: serde_json::json!({"search": "aggregate-query"}),
            raw_input: "aggregate-query".to_string(),
            runtime: serde_json::Value::Null,
            publication: None,
            revision: 7,
        };
        service.update_active_snapshot(&snapshot);

        let entry = crate::command::CommandEntry::new(
            "apps.open",
            Some("Open".to_string()),
            None,
            crate::command::BindingLayer::View,
        );
        let execution = service
            .command_execution(&entry, ViewInstanceId(1))
            .unwrap();

        assert_eq!(execution.context.page.view_ref, "core:default");
        assert_eq!(execution.context.owner.view_ref, "core:default");
        assert_eq!(
            execution.context.owner.parameters.values(),
            &snapshot.parameters
        );
        assert_eq!(
            execution.context.owner.parameters.raw_input(),
            snapshot.raw_input
        );
    }

    /// The fixture's `core:default` is `binding_mode = "item_merge"` with a base bindings.
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

        // The View layer rejects duplicate keys, so publishing it must be valid.
        crate::command::CommandRegistry::new()
            .replace_layer(crate::command::BindingLayer::View, entries)
            .unwrap();
    }

    /// A focused item may name its command in any address form; the entry is
    /// always keyed by the resolved FQID, exactly like the declared table, so
    /// `unbind.commands` and the runtime envelope address one single id.
    #[test]
    fn a_focused_item_overrides_the_item_mode_base_binding() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let service = item_mode_service(&config);
        let context = ViewContext::new(ViewInstanceId(51), "core:default");
        let snapshot = item_mode_snapshot(&config, Some(serde_json::json!({"tab": "accept"})));

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
        assert_eq!(
            tab[0].0, "core.accept",
            "a bare item target is keyed by its FQID, like the declared table"
        );
        // The base key the item did not claim survives.
        assert!(bound.iter().any(|(_, key)| key.as_deref() == Some("space")));

        crate::command::CommandRegistry::new()
            .replace_layer(crate::command::BindingLayer::View, entries)
            .unwrap();
    }
}
