use super::{CommandAction, CommandBindingVisibility, CompiledConfig, Defaults, View, ViewRef};
use anyhow::{Context, Result, bail};
use std::{collections::BTreeMap, path::Path};

pub(crate) trait EngineConfigValidator {
    fn validate_defaults(&self, defaults: &Defaults) -> Result<()>;
    fn validate_view(&self, name: &str, view: &View, script_root: Option<&Path>) -> Result<()>;
    fn validate_relations(&self, config: &CompiledConfig) -> Result<()>;
}

fn validate_command_action(
    view_ref: &str,
    command_id: &str,
    action: &CommandAction,
    views: &BTreeMap<ViewRef, View>,
    script_root: Option<&Path>,
    depth: usize,
) -> Result<()> {
    if depth > 16 {
        bail!(
            "view {:?} command {:?} action nesting exceeds 16 levels",
            view_ref,
            command_id
        );
    }
    if matches!(
        action,
        CommandAction::OpenCommands | CommandAction::OpenParameters
    ) {
        return Ok(());
    }
    validate_producer_action(view_ref, command_id, action, views, script_root)
}

fn validate_producer_action(
    view_ref: &str,
    command_id: &str,
    action: &CommandAction,
    views: &BTreeMap<ViewRef, View>,
    script_root: Option<&Path>,
) -> Result<()> {
    let owner = format!("view {:?} command {:?}", view_ref, command_id);
    let producer = action
        .producer()
        .expect("producer action validation requires a producer");
    let handler = producer_handler(action);
    let operation_type = action.operation_type();
    match producer {
        super::ProducerKind::Declared => {
            let operation = crate::protocol::parse_declared_operation(
                operation_type,
                handler,
                &format!("{owner} declared handler"),
            )?;
            validate_operation_target(view_ref, command_id, &operation, views)?;
        }
        super::ProducerKind::Script => {
            super::parse_producer_script_handler(handler, script_root)
                .with_context(|| format!("{owner} script handler"))?;
        }
    }

    if let CommandAction::Call {
        return_processor: Some(processor),
        ..
    } = action
    {
        validate_return_processor(view_ref, command_id, processor, views, script_root)?;
    }
    Ok(())
}

fn producer_handler(action: &CommandAction) -> &toml::Value {
    match action {
        CommandAction::Run { handler, .. }
        | CommandAction::Navigate { handler, .. }
        | CommandAction::Call { handler, .. }
        | CommandAction::Return { handler, .. } => handler,
        CommandAction::OpenCommands | CommandAction::OpenParameters => {
            unreachable!("built-in actions have no producer handler")
        }
    }
}

fn validate_return_processor(
    view_ref: &str,
    command_id: &str,
    processor: &super::ReturnProcessor,
    views: &BTreeMap<ViewRef, View>,
    script_root: Option<&Path>,
) -> Result<()> {
    let owner = format!(
        "view {:?} command {:?} return processor",
        view_ref, command_id
    );
    match processor.producer {
        super::ProducerKind::Declared => {
            let operation = crate::protocol::parse_declared_operation(
                &processor.operation,
                &processor.handler,
                &format!("{owner} declared handler"),
            )?;
            validate_operation_target(view_ref, command_id, &operation, views)?;
        }
        super::ProducerKind::Script => {
            anyhow::ensure!(
                matches!(
                    processor.operation.as_str(),
                    "navigate" | "call" | "return" | "run"
                ),
                "{owner} has unsupported operation {:?}",
                processor.operation
            );
            super::parse_producer_script_handler(&processor.handler, script_root)
                .with_context(|| format!("{owner} script handler"))?;
        }
    }
    Ok(())
}

fn validate_operation_target(
    view_ref: &str,
    command_id: &str,
    operation: &crate::protocol::ProtocolOperation,
    views: &BTreeMap<ViewRef, View>,
) -> Result<()> {
    let (kind, target, presentation) = match operation {
        crate::protocol::ProtocolOperation::Navigate {
            target,
            presentation,
            ..
        } => ("navigation", target, presentation),
        crate::protocol::ProtocolOperation::Call {
            target,
            presentation,
            ..
        } => ("call", target, presentation),
        _ => return Ok(()),
    };
    let configured = views.contains_key(target)
        || views
            .values()
            .any(|view| view.alias.as_deref() == Some(target));
    // A sibling route is a runtime integration point, not a package dependency.
    // Standalone loading must still validate local targets, but cannot require
    // another suite member to be installed.
    let external_route = target.split_once(':').is_some_and(|(member, view)| {
        !member.is_empty()
            && !view.is_empty()
            && !view.contains(':')
            && view_ref
                .split_once(':')
                .is_some_and(|(owner, _)| owner != member)
    });
    if !configured && !external_route {
        bail!(
            "view {:?} command {:?} references missing {} target {:?}",
            view_ref,
            command_id,
            kind,
            target
        );
    }
    if presentation.mode != super::ViewPresentationMode::Popup
        && (presentation.width.is_some() || presentation.height.is_some())
    {
        bail!("producer presentation width and height require popup mode");
    }
    if presentation.width == Some(0) || presentation.height == Some(0) {
        bail!("producer presentation width and height must be positive");
    }
    Ok(())
}

impl CompiledConfig {
    pub(crate) fn validate_with_engines<V>(&self, engines: &V) -> Result<()>
    where
        V: EngineConfigValidator,
    {
        if !self.entrypoint.is_empty() {
            self.engine(&self.entrypoint)?;
        }
        engines.validate_defaults(&self.defaults)?;
        for (package_id, workflow) in &self.workflows {
            if workflow.name.trim().is_empty() {
                bail!("workflow {:?} has an empty name", package_id);
            }
        }

        let mut command_keys = BTreeMap::new();
        let mut overflow_commands = 0;
        for (id, binding) in &self.commands.bindings {
            if id == "commands"
                && (binding.action.is_some()
                    || binding.label.is_some()
                    || binding.visibility.is_some())
            {
                bail!("session command \"commands\" is built in; configure only its key");
            }
            let action = binding
                .command_action(id)
                .with_context(|| format!("session command binding {id:?} must define an action"))?;
            if id == "commands" {
                anyhow::ensure!(
                    matches!(action, CommandAction::OpenCommands),
                    "session command binding \"commands\" is built in"
                );
                anyhow::ensure!(
                    self.view("__commands:main").is_some(),
                    "session command binding \"commands\" requires view \"__commands:main\""
                );
            }
            let key = binding
                .key(id)
                .with_context(|| format!("session command binding {id:?} has no key"))?;
            let _label = binding
                .label(id)
                .with_context(|| format!("session command binding {id:?} has no label"))?;
            let key = super::normalize_key(key)?;
            if let Some(previous) = command_keys.insert(key.clone(), id) {
                bail!(
                    "session command bindings {:?} and {:?} both use key {:?}",
                    previous,
                    id,
                    key
                );
            }
            let visibility = binding
                .visibility(id)
                .with_context(|| format!("session command binding {id:?} has no visibility"))?;
            if visibility == CommandBindingVisibility::Overflow {
                overflow_commands += 1;
            }
            validate_command_action(
                if self.entrypoint.is_empty() {
                    "<root>"
                } else {
                    &self.entrypoint
                },
                &format!("session:command:{id}"),
                &action,
                &self.views,
                None,
                0,
            )?;
        }
        if overflow_commands > 1 {
            bail!("session commands can define at most one overflow binding");
        }

        for (alias, target) in &self.aliases {
            if alias.trim().is_empty()
                || alias.contains(':')
                || alias.chars().any(char::is_whitespace)
            {
                bail!("alias {:?} is invalid", alias);
            }
            if !self.views.contains_key(target) {
                bail!("alias {:?} targets missing view {:?}", alias, target);
            }
        }

        for (cmd_id, command) in &self.all_commands {
            let Some((wf_id, _)) = cmd_id.split_once(':') else {
                continue;
            };
            if command.label.trim().is_empty() {
                bail!("command {:?} has an empty label", cmd_id);
            }
            validate_command_action(
                cmd_id,
                cmd_id,
                &command.action,
                &self.views,
                self.workflow_root(wf_id),
                0,
            )?;
        }

        for (view_ref, view) in &self.views {
            validate_view_ref(view_ref)?;
            if let Some(alias) = &view.alias {
                bail!(
                    "view {:?} cannot declare alias {:?}; ADR 0005 centralizes aliases in suite manifests ([aliases])",
                    view_ref,
                    alias
                );
            }
            engines.validate_view(view_ref, view, self.workflow_root(view_ref))?;
            let wf_id = super::package_id(view_ref);
            if let Some(keymap) = &view.keymap {
                match keymap.mode {
                    super::KeymapMode::Item => {
                        if !keymap.bindings.is_empty() {
                            bail!(
                                "view {:?} keymap uses mode = \"item\" and cannot define static key bindings",
                                view_ref
                            );
                        }
                    }
                    super::KeymapMode::Static => {
                        for (key, val) in &keymap.bindings {
                            if val.as_bool() == Some(false) {
                                continue;
                            }
                            let Some(cmd_target) = val.as_str() else {
                                bail!("view {:?} keymap binding {:?} must be an action, command, or false", view_ref, key);
                            };
                            let is_command = self.find_command(wf_id, cmd_target).is_some();
                            let is_action = crate::engine::is_picker_action(cmd_target)
                                || crate::engine::is_capture_action(cmd_target);
                            if !is_command && !is_action {
                                bail!(
                                    "view {:?} keymap binds {:?} to unknown command or action {:?}",
                                    view_ref,
                                    key,
                                    cmd_target
                                );
                            }
                        }
                    }
                }
            }
        }
        engines.validate_relations(self)?;
        Ok(())
    }
}

fn validate_view_ref(view_ref: &str) -> Result<()> {
    let Some((workflow, view)) = view_ref.split_once(':') else {
        bail!("view reference {:?} must use workflow:view form", view_ref);
    };
    if workflow.is_empty() || view.is_empty() || view.contains(':') {
        bail!("invalid view reference {:?}", view_ref);
    }
    Ok(())
}
