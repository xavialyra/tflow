use super::{CommandAction, CompiledConfig, Defaults, ResolvedAddress, Unbind, View, ViewRef};
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
    validate_command_action_execution(view_ref, command_id, action, views, script_root)
}

fn validate_command_action_execution(
    view_ref: &str,
    command_id: &str,
    action: &CommandAction,
    views: &BTreeMap<ViewRef, View>,
    script_root: Option<&Path>,
) -> Result<()> {
    let owner = format!("view {:?} command {:?}", view_ref, command_id);
    let execution = action.execution_mode();
    let payload = action.payload();
    let operation_type = action.operation_type();
    match execution {
        super::ExecutionMode::Declared => {
            let operation =
                crate::protocol::parse_declared_operation(operation_type, payload, &owner)?;
            validate_operation_target(view_ref, command_id, &operation, views)?;
        }
        super::ExecutionMode::Script => {
            super::parse_script_source(payload, script_root)
                .with_context(|| format!("{owner} script"))?;
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
    match processor.execution {
        super::ExecutionMode::Declared => {
            let operation = crate::protocol::parse_declared_operation(
                processor
                    .operation
                    .as_deref()
                    .context("declared return processor requires type")?,
                &processor.payload,
                &owner,
            )?;
            validate_operation_target(view_ref, command_id, &operation, views)?;
        }
        super::ExecutionMode::Script => {
            if let Some(operation) = processor.operation.as_deref() {
                anyhow::ensure!(
                    matches!(
                        operation,
                        "navigate" | "call" | "return" | "run" | "companion"
                    ),
                    "{owner} has unsupported operation {:?}",
                    operation
                );
            }
            super::parse_script_source(&processor.payload, script_root)
                .with_context(|| format!("{owner} script"))?;
        }
    }
    Ok(())
}

static DEFAULT_PRESENTATION: std::sync::LazyLock<super::ViewPresentation> =
    std::sync::LazyLock::new(super::ViewPresentation::default);

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
        } => ("navigation", target.as_str(), presentation),
        crate::protocol::ProtocolOperation::Call {
            target,
            presentation,
            ..
        } => ("call", target.as_str(), presentation),
        crate::protocol::ProtocolOperation::Companion { target, slot, .. } => {
            if let Some(target) = target {
                ("companion", target.as_str(), &*DEFAULT_PRESENTATION)
            } else if let Some(slot_name) = slot {
                ("companion", slot_name.as_str(), &*DEFAULT_PRESENTATION)
            } else {
                bail!(
                    "view {:?} command {:?} requires target or slot",
                    view_ref,
                    command_id
                );
            }
        }
        _ => return Ok(()),
    };

    let caller_pkg = super::package_id(view_ref);
    let explicit_local = target
        .strip_prefix("self:")
        .or_else(|| target.strip_prefix(':'));

    if let Some(local_view) = explicit_local {
        let scoped_key = format!("{caller_pkg}:{local_view}");
        if !views.contains_key(&scoped_key) && !views.contains_key(local_view) {
            bail!(
                "view {:?} command {:?} references missing local {} target {:?}",
                view_ref,
                command_id,
                kind,
                target
            );
        }
    } else {
        let local_scoped_key =
            (!target.contains(':') && !caller_pkg.is_empty() && caller_pkg != "<root>")
                .then(|| format!("{caller_pkg}:{target}"));

        let configured = views.contains_key(target)
            || local_scoped_key
                .as_ref()
                .is_some_and(|k| views.contains_key(k))
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
    }
    let is_popup = presentation.mode == super::ViewPresentationMode::Popup;
    if !is_popup
        && (presentation.width.is_some()
            || presentation.height.is_some()
            || presentation.anchor != super::PopupAnchor::Center
            || presentation.offset_x.is_some()
            || presentation.offset_y.is_some()
            || presentation.min_width.is_some()
            || presentation.max_width.is_some()
            || presentation.min_height.is_some()
            || presentation.max_height.is_some()
            || !presentation.show_title)
    {
        bail!("producer presentation width and height require popup mode");
    }
    if presentation.width.as_ref().is_some_and(|w| w.is_zero())
        || presentation.height.as_ref().is_some_and(|h| h.is_zero())
    {
        bail!("producer presentation width and height must be positive");
    }
    if let (Some(min), Some(max)) = (presentation.min_width, presentation.max_width)
        && min > max
    {
        bail!("presentation min_width ({min}) cannot exceed max_width ({max})");
    }
    if let (Some(min), Some(max)) = (presentation.min_height, presentation.max_height)
        && min > max
    {
        bail!("presentation min_height ({min}) cannot exceed max_height ({max})");
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
            if let Some(query) = &self.entrypoint_query {
                self.validate_parameter_values(&self.entrypoint, query)
                    .with_context(|| {
                        format!("invalid suite entrypoint query for {:?}", self.entrypoint)
                    })?;
            }
        }
        engines.validate_defaults(&self.defaults)?;
        for (package_id, workflow) in &self.workflows {
            if workflow.name.trim().is_empty() {
                bail!("workflow {:?} has an empty name", package_id);
            }
        }

        // Host bindings are references into `all_commands`; the definition-map
        // loop below validates every action, so here we only check identity and
        // physical-key uniqueness.
        let mut command_keys = BTreeMap::new();
        for (key, id) in &self.host_bindings {
            let key = crate::input::Key::canonical_binding_name(key)?;
            if let Some(previous) = command_keys.insert(key.clone(), id) {
                bail!(
                    "host command bindings {:?} and {:?} both use key {:?}",
                    previous,
                    id,
                    key
                );
            }
            if !self.all_commands.contains_key(id)
                && crate::command::host_action_label(id).is_none()
            {
                bail!("host command binding {:?} targets unknown command", id);
            }
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
            // The index is `<owner>.<name>`: an engine action owns
            // `<engine>.<action>` and a workflow command owns
            // `<workflow>.<command>`. A workflow command landing on an engine
            // action's id would make one identity name two different commands, so
            // the collision is rejected instead of resolved by priority.
            if crate::command::host_action_label(cmd_id).is_some() {
                bail!("command {cmd_id:?} collides with a built-in Host action");
            }
            if let Some((action_fqid, label)) = crate::engine::engine_action_from_id(cmd_id) {
                bail!(
                    "command {cmd_id:?} collides with the {action_fqid:?} engine action ({label:?}), \
                     which already owns that id; every command FQID must name exactly one command"
                );
            }
            let Some((wf_id, _)) = cmd_id.split_once('.') else {
                continue;
            };
            if command.label.trim().is_empty() {
                bail!("command {:?} has an empty label", cmd_id);
            }
            let caller_view = self
                .workflows
                .get(wf_id)
                .and_then(|workflow| workflow.entrypoint.as_deref())
                .map(|entrypoint| format!("{wf_id}:{entrypoint}"))
                .or_else(|| {
                    self.views
                        .keys()
                        .find(|view| super::package_id(view) == wf_id)
                        .cloned()
                })
                .unwrap_or_else(|| format!("{wf_id}:main"));
            validate_command_action(
                &caller_view,
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
            self.chrome_commands_show(view_ref)?;
            let wf_id = super::package_id(view_ref);
            if let Some(target) = &view.companion {
                if target.trim().is_empty() {
                    bail!("view {:?} companion target cannot be empty", view_ref);
                }
                let target_view = if let Some(cmd) = self.find_command(wf_id, target)
                    && let crate::workflow::config::CommandAction::Companion { payload, .. } = &cmd.action
                    && let toml::Value::Table(table) = payload
                {
                    table.get("target").and_then(|v| v.as_str()).unwrap_or(target)
                } else {
                    target.as_str()
                };
                let _resolved = self
                    .resolve_view_scoped(target_view, view_ref)
                    .with_context(|| {
                        format!(
                            "view {:?} companion references unknown view {:?}",
                            view_ref, target_view
                        )
                    })?;
            }
            self.validate_unbind(&view.unbind, view_ref, view)?;
            if let Some(bindings) = &view.bindings {
                // Both modes may declare bindings. `binding_mode = "item_merge"`
                // only adds the focused item's bindings on top of them (item
                // wins per key). A binding names a command: a key is claimed by
                // running something, never by a bare boolean. `false` exists
                // only in the engine default tables and `[host.bindings]`, where
                // it declares that layer to have no binding for the key.
                for (key, val) in bindings {
                    let Some(address) = val.as_str() else {
                        bail!(
                            "view {:?} binding {:?} must name a command (or an engine action written @engine:<engine>.<action>); to release a key in this view, list it in [views.<name>.unbind] keys",
                            view_ref,
                            key
                        );
                    };
                    self.validate_address(
                        view_ref,
                        view,
                        wf_id,
                        &format!("bindings {key:?}"),
                        address,
                    )?;
                }
            }
        }
        engines.validate_relations(self)?;
        Ok(())
    }

    /// Validates a View's `unbind` table. Each field maps to one axis: `keys`
    /// are physical keys, `commands` are command addresses (the unique index),
    /// and `layers` are priority layers.
    fn validate_unbind(&self, unbind: &Unbind, view_ref: &str, view: &View) -> Result<()> {
        for key in &unbind.keys {
            crate::input::Key::parse_binding(key).with_context(|| {
                format!("view {view_ref:?} unbind.keys has invalid physical key {key:?}")
            })?;
        }
        for layer in &unbind.layers {
            if crate::command::BindingLayer::from_layer(layer).is_none() {
                bail!(
                    "view {view_ref:?} unbind.layers has unknown layer {layer:?} (expected view, engine, or host)"
                );
            }
        }
        let workflow_id = super::package_id(view_ref);
        for address in &unbind.commands {
            self.validate_address(view_ref, view, workflow_id, "unbind.commands", address)?;
        }
        Ok(())
    }

    /// A View binding value and an `unbind.commands` entry accept exactly the
    /// same addresses, so they share one check: a command of the View's own
    /// workflow, or an action of the View's own engine written
    /// `@engine:<engine>.<action>`. `location` names the offending field for the
    /// error message.
    fn validate_address(
        &self,
        view_ref: &str,
        view: &View,
        workflow_id: &str,
        location: &str,
        address: &str,
    ) -> Result<()> {
        let engine = view.selected_engine_type();
        let accepted = match self.resolve_address(workflow_id, address) {
            Some(ResolvedAddress::Command { .. }) => true,
            Some(ResolvedAddress::Engine { fqid, .. }) => fqid.starts_with(&format!("{engine}.")),
            None => false,
        };
        if !accepted {
            bail!(
                "view {view_ref:?} {location} maps to unknown command {address:?} (this View's engine actions are addressed as @engine:{engine}.<action>)"
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_return_processor_accepts_companion_operation() {
        let processor = toml::from_str::<super::super::ReturnProcessor>(
            "type = 'companion'\nscript = 'printf response'",
        )
        .unwrap();
        validate_return_processor("sample:main", "open", &processor, &BTreeMap::new(), None)
            .unwrap();
    }
}
