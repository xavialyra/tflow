use super::{CommandAction, CompiledConfig, Defaults, View, ViewRef};
use anyhow::{Context, Result, bail};
use std::{collections::BTreeMap, path::Path};

pub(crate) trait EngineConfigValidator {
    fn validate_defaults(&self, defaults: &Defaults) -> Result<()>;
    fn validate_view(&self, name: &str, view: &View, script_root: Option<&Path>) -> Result<()>;
    fn validate_relations(&self, config: &CompiledConfig) -> Result<()>;
}

pub(super) fn validate_view_commands(
    view_ref: &str,
    commands: &BTreeMap<String, super::Command>,
    views: &BTreeMap<ViewRef, View>,
    script_root: Option<&Path>,
) -> Result<()> {
    for (command_id, command) in commands {
        validate_command_action(view_ref, command_id, &command.action, views, script_root, 0)?;
    }
    Ok(())
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
        | CommandAction::Return { handler, .. }
        | CommandAction::EditInput { handler, .. }
        | CommandAction::Invoke { handler, .. } => handler,
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
                    "navigate" | "call" | "return" | "run" | "edit-input" | "invoke"
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
    if !configured {
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
        if let Some(default_view) = &self.default_view {
            self.engine(default_view)?;
        }
        engines.validate_defaults(&self.defaults)?;
        for (package_id, workflow) in &self.workflows {
            if workflow.name.trim().is_empty() {
                bail!("workflow {:?} has an empty name", package_id);
            }
        }

        let mut command_keys = BTreeMap::new();
        for (id, binding) in &self.commands.bindings {
            let action = binding
                .command_action(id)
                .with_context(|| format!("session command binding {id:?} must define an action"))?;
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
            validate_command_action(
                self.default_view.as_deref().unwrap_or("<root>"),
                &format!("session:command:{id}"),
                &action,
                &self.views,
                None,
                0,
            )?;
        }
        let mut aliases = BTreeMap::<&str, &str>::new();
        for (view_ref, view) in &self.views {
            validate_view_ref(view_ref)?;
            if let Some(alias) = &view.alias {
                if alias.trim().is_empty()
                    || alias.contains(':')
                    || alias.chars().any(char::is_whitespace)
                {
                    bail!("view {:?} has an invalid alias {:?}", view_ref, alias);
                }
                if let Some(previous) = aliases.insert(alias, view_ref) {
                    bail!(
                        "view alias {:?} is assigned to both {:?} and {:?}",
                        alias,
                        previous,
                        view_ref
                    );
                }
            }
            engines.validate_view(view_ref, view, self.workflow_root(view_ref))?;
            validate_view_commands(
                view_ref,
                &view.commands,
                &self.views,
                self.workflow_root(view_ref),
            )?;
            let mut keys = BTreeMap::new();
            for (command_id, command) in &view.commands {
                if command.label.trim().is_empty() {
                    bail!(
                        "view {:?} command {:?} has an empty label",
                        view_ref,
                        command_id
                    );
                }
                if let Some(raw_key) = &command.key {
                    let key = super::normalize_key(raw_key)
                        .with_context(|| format!("view {:?} command {:?}", view_ref, command_id))?;
                    if keys.insert(key.clone(), command_id).is_some() {
                        bail!("view {:?} has duplicate command key {:?}", view_ref, key);
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
