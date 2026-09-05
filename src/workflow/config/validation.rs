use super::{
    CommandAction, CommandBindingVisibility, Config, Defaults, ScriptSourceSpec, View, ViewRef,
    validate_script_source_args,
};
use crate::expression::{EvaluationStage, Template, TemplateRegistry, is_dynamic_string};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

pub(crate) trait EngineConfigValidator {
    fn validate_defaults(&self, defaults: &Defaults) -> Result<()>;
    fn validate_view(&self, name: &str, view: &View, script_root: Option<&Path>) -> Result<()>;
    fn validate_relations(&self, config: &Config) -> Result<()>;
}

pub(super) fn validate_json_requirements(
    templates: &TemplateRegistry,
    value: &Value,
    stage: EvaluationStage,
    consumer: &str,
) -> Result<()> {
    templates
        .requirements_for_value(value)?
        .validate_stage(stage, consumer)
}

pub(super) fn validate_toml_requirements(
    templates: &TemplateRegistry,
    value: &toml::Value,
    stage: EvaluationStage,
    consumer: &str,
) -> Result<()> {
    let value = super::toml_to_json(value)?;
    validate_json_requirements(templates, &value, stage, consumer)
}

pub(super) fn validate_string_requirements(
    templates: &TemplateRegistry,
    value: &str,
    stage: EvaluationStage,
    consumer: &str,
) -> Result<()> {
    validate_json_requirements(
        templates,
        &Value::String(value.to_string()),
        stage,
        consumer,
    )
}

pub(super) fn validate_optional_string_requirements(
    templates: &TemplateRegistry,
    value: Option<&str>,
    stage: EvaluationStage,
    consumer: &str,
) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    validate_string_requirements(templates, value, stage, consumer)
}

pub(super) fn validate_run_payload(
    payload: &super::RunPayload,
    script_root: Option<&Path>,
    owner: &str,
) -> Result<()> {
    if let Some(script) = &payload.script {
        if script.trim().is_empty() {
            bail!("{} has an empty script body", owner);
        }
        return Ok(());
    }
    let Some(handler) = &payload.handler else {
        bail!("{} run command must define either script or handler", owner);
    };
    validate_run_handler(handler, script_root, owner)
}

pub(super) fn validate_run_handler(
    handler: &toml::Value,
    script_root: Option<&Path>,
    owner: &str,
) -> Result<()> {
    let spec = match handler {
        toml::Value::Table(_) => {
            let spec = ScriptSourceSpec::parse(handler)
                .with_context(|| format!("{} has an invalid command handler source", owner))?;
            spec.validate_command_handler()
                .with_context(|| format!("{} has an invalid command handler source", owner))?;
            spec
        }
        toml::Value::String(source) if is_dynamic_string(source) => {
            let template = Template::parse(source)
                .with_context(|| format!("{} has an invalid command handler source", owner))?;
            anyhow::ensure!(
                template.is_complete_path(),
                "{} command handler must be a script source table or complete dynamic path",
                owner
            );
            return Ok(());
        }
        toml::Value::String(source) if source.starts_with("#!") || source.contains('\n') => {
            return Ok(());
        }
        toml::Value::String(file) => {
            return validate_script_file_target(file, script_root, owner);
        }
        _ => {
            bail!(
                "{} command handler must be a script source table, inline script, or complete dynamic path",
                owner
            )
        }
    };
    if let Some(script) = spec.script_value() {
        if script.trim().is_empty() {
            bail!("{} has an empty command handler script", owner);
        }
        return Ok(());
    }
    let Some(file) = spec.file_value() else {
        bail!(
            "{} command handler file must be a string or dynamic path",
            owner
        );
    };
    if file.trim().is_empty() {
        bail!("{} has an empty command handler file", owner);
    }
    if is_dynamic_string(file) {
        return Ok(());
    }
    spec.validate_target(script_root)
        .with_context(|| format!("{} has an invalid command handler file", owner))
}

fn validate_script_file_target(file: &str, script_root: Option<&Path>, owner: &str) -> Result<()> {
    if is_dynamic_string(file) {
        return Ok(());
    }
    let path = Path::new(file);
    if let Some(root) = script_root {
        crate::execution::validate_script_target(root, file)
            .with_context(|| format!("{} has an invalid command handler file", owner))
    } else if path.is_relative() {
        bail!(
            "single-file workflow {} cannot reference relative script file {:?}; workflows must be self-contained using inline scripts or system binaries",
            owner,
            file
        );
    } else {
        Ok(())
    }
}

pub(super) fn validate_argv_arguments(arguments: &toml::Value, owner: &str) -> Result<()> {
    validate_script_source_args(Some(arguments))
        .with_context(|| format!("{} args must be an array or complete dynamic path", owner))
}

pub(super) fn validate_run_arguments(arguments: &toml::Value, owner: &str) -> Result<()> {
    validate_argv_arguments(arguments, &format!("{} command", owner))
}

pub(super) fn validate_command_action_requirements(
    templates: &TemplateRegistry,
    action: &CommandAction,
    stage: EvaluationStage,
    consumer: &str,
    depth: usize,
) -> Result<()> {
    if depth > 16 {
        bail!("{consumer} action nesting exceeds 16 levels");
    }
    let validate = |value: &toml::Value, label: &str, value_stage: EvaluationStage| {
        validate_toml_requirements(templates, value, value_stage, label)
    };
    match action {
        CommandAction::Run { .. } => {
            let payload = action.run_payload().unwrap_or_default();
            if let Some(handler) = &payload.handler {
                validate(handler, &format!("{consumer} handler"), stage)?;
            }
            if let Some(args) = &payload.args {
                validate(args, &format!("{consumer} args"), stage)?;
            }
            if let Some(shell) = &payload.shell {
                validate(
                    &toml::Value::String(shell.clone()),
                    &format!("{consumer} shell"),
                    stage,
                )?;
            }
        }
        CommandAction::Navigate { payload } => {
            validate_presentation(&format!("{consumer} presentation"), &payload.presentation)?;
            validate(&payload.target, &format!("{consumer} target"), stage)?;
            if let Some(query) = &payload.query {
                validate(query, &format!("{consumer} query"), stage)?;
            }
        }
        CommandAction::Call { payload } => {
            validate_presentation(&format!("{consumer} presentation"), &payload.presentation)?;
            validate(&payload.target, &format!("{consumer} target"), stage)?;
            if let Some(query) = &payload.query {
                validate(query, &format!("{consumer} query"), stage)?;
            }
            if let Some(engine) = &payload.engine {
                validate(engine, &format!("{consumer} engine"), stage)?;
            }
            if let Some(then) = &payload.then {
                validate_command_action_requirements(
                    templates,
                    then,
                    EvaluationStage::Return,
                    &format!("{consumer} continuation"),
                    depth + 1,
                )?;
            }
        }
        CommandAction::Return { .. } => {
            let payload = action.return_payload().unwrap_or_default();
            if let Some(value) = &payload.value {
                validate(value, &format!("{consumer} value"), stage)?;
            }
            if let Some(handler) = &payload.handler {
                validate(
                    handler,
                    &format!("{consumer} handler"),
                    EvaluationStage::Return,
                )?;
            }
            if let Some(args) = &payload.args {
                validate(args, &format!("{consumer} args"), EvaluationStage::Return)?;
            }
        }
        CommandAction::EditInput { payload } => {
            validate(&payload.value, &format!("{consumer} value"), stage)?;
            if let Some(cursor) = &payload.cursor {
                validate(cursor, &format!("{consumer} cursor"), stage)?;
            }
        }
        CommandAction::Invoke { payload } => {
            validate(&payload.command, &format!("{consumer} command"), stage)?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
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
    let owner = format!("{}:{}", view_ref, command_id);
    match action {
        CommandAction::Run { .. } => {
            let payload = action.run_payload().context("invalid run action")?;
            validate_run_payload(&payload, script_root, &owner)?;
            if let Some(args) = &payload.args {
                validate_run_arguments(args, &owner)?;
            }
        }

        CommandAction::Navigate { payload } => {
            validate_target(view_ref, command_id, "navigation", &payload.target, views)?;
            if let Some(query) = &payload.query {
                validate_templates(query)?;
            }
        }
        CommandAction::Call { payload } => {
            validate_target(view_ref, command_id, "call", &payload.target, views)?;
            if let Some(value) = &payload.query {
                validate_templates(value)?;
            }
            if let Some(value) = &payload.engine {
                validate_templates(value)?;
            }
            if let Some(then) = &payload.then {
                validate_command_action(view_ref, command_id, then, views, script_root, depth + 1)?;
            }
        }
        CommandAction::Return { .. } => {
            let payload = action.return_payload().unwrap_or_default();
            if depth > 0 && (payload.handler.is_some() || payload.args.is_some()) {
                bail!(
                    "view {:?} command {:?} continuation return cannot define handler or args",
                    view_ref,
                    command_id
                );
            }
            if payload.handler.is_none() && payload.args.is_some() {
                bail!(
                    "view {:?} command {:?} return args require a handler",
                    view_ref,
                    command_id
                );
            }
            if let Some(value) = &payload.value {
                validate_templates(value)?;
            }
            if let Some(handler) = &payload.handler {
                validate_result_handler(handler, &owner, script_root)?;
            }
            if let Some(args) = &payload.args {
                validate_argv_arguments(
                    args,
                    &format!("view {:?} command {:?}", view_ref, command_id),
                )?;
            }
        }
        CommandAction::EditInput { payload } => {
            validate_templates(&payload.value)?;
            if let Some(cursor) = &payload.cursor {
                validate_templates(cursor)?;
            }
        }
        CommandAction::Invoke { payload } => validate_templates(&payload.command)?,
    }
    Ok(())
}

fn validate_presentation(consumer: &str, presentation: &super::ViewPresentation) -> Result<()> {
    if presentation.mode != super::ViewPresentationMode::Popup
        && (presentation.width.is_some() || presentation.height.is_some())
    {
        bail!("{consumer} width and height require popup mode");
    }
    if presentation.width == Some(0) || presentation.height == Some(0) {
        bail!("{consumer} width and height must be positive");
    }
    Ok(())
}

fn validate_target(
    view_ref: &str,
    command_id: &str,
    kind: &str,
    target: &toml::Value,
    views: &BTreeMap<ViewRef, View>,
) -> Result<()> {
    validate_templates(target).with_context(|| {
        format!(
            "view {:?} command {:?} has invalid {} payload",
            view_ref, command_id, kind
        )
    })?;
    let target = target
        .as_str()
        .with_context(|| format!("{} target must be a string or dynamic path", kind))?;
    let configured = views.contains_key(target)
        || views
            .values()
            .any(|view| view.alias.as_deref() == Some(target));
    if !is_dynamic_string(target) && !configured {
        bail!(
            "view {:?} command {:?} references missing view {:?}",
            view_ref,
            command_id,
            target
        );
    }
    Ok(())
}

fn validate_result_handler(
    handler: &toml::Value,
    owner: &str,
    script_root: Option<&Path>,
) -> Result<()> {
    let handler = handler
        .as_str()
        .with_context(|| format!("{} result handler must be a string or dynamic path", owner))?;
    if is_dynamic_string(handler) {
        Template::parse(handler)?;
        return Ok(());
    }
    let root =
        script_root.with_context(|| format!("{} result handler has no plugin root", owner))?;
    let path = Path::new(handler);
    if handler.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!("{} has invalid result handler {:?}", owner, handler);
    }
    crate::execution::validate_script_target(root, handler)
        .with_context(|| format!("{} has an invalid result handler target", owner))?;
    Ok(())
}

pub(crate) fn validate_templates(value: &toml::Value) -> Result<()> {
    match value {
        toml::Value::String(source) => {
            Template::parse(source)?;
        }
        toml::Value::Array(values) => {
            for value in values {
                validate_templates(value)?;
            }
        }
        toml::Value::Table(values) => {
            for value in values.values() {
                validate_templates(value)?;
            }
        }
        toml::Value::Boolean(_)
        | toml::Value::Datetime(_)
        | toml::Value::Float(_)
        | toml::Value::Integer(_) => {}
    }
    Ok(())
}

impl Config {
    pub(crate) fn validate_with_engines<V>(&self, engines: &V) -> Result<()>
    where
        V: EngineConfigValidator,
    {
        if let Some(default_view) = &self.default_view {
            self.engine(default_view)?;
        }

        engines.validate_defaults(&self.compiled.defaults)?;
        if let Some(bindings) = &self.compiled.defaults.picker.bindings {
            validate_toml_requirements(
                &self.compiled.template_registry,
                bindings,
                EvaluationStage::Operation,
                "root picker defaults",
            )?;
        }
        if let Some(bindings) = &self.compiled.defaults.capture.bindings {
            validate_toml_requirements(
                &self.compiled.template_registry,
                bindings,
                EvaluationStage::Operation,
                "root capture defaults",
            )?;
        }
        for (package_id, workflow) in &self.compiled.workflows {
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
            let action = binding.command_action(id).with_context(|| {
                format!("session command binding {id:?} must define a call action")
            })?;
            if !matches!(action, CommandAction::Call { .. }) {
                bail!("session command binding {id:?} must use a call action");
            }
            let key_source = binding
                .key(id)
                .with_context(|| format!("session command binding {id:?} has no key"))?;
            let label = binding
                .label(id)
                .with_context(|| format!("session command binding {id:?} has no label"))?;
            let visibility = binding
                .visibility(id)
                .with_context(|| format!("session command binding {id:?} has no visibility"))?;
            validate_string_requirements(
                &self.compiled.template_registry,
                key_source,
                EvaluationStage::Bootstrap,
                &format!("session command binding {id:?} key"),
            )?;
            validate_string_requirements(
                &self.compiled.template_registry,
                label,
                EvaluationStage::Bootstrap,
                &format!("session command binding {id:?} label"),
            )?;
            let key = super::normalize_key(key_source)?;
            if let Some(previous) = command_keys.insert(key.clone(), id) {
                bail!(
                    "session command bindings {:?} and {:?} both use key {:?}",
                    previous,
                    id,
                    key
                );
            }
            if visibility == CommandBindingVisibility::Overflow {
                overflow_commands += 1;
            }
            validate_command_action(
                self.default_view.as_deref().unwrap_or("<root>"),
                &format!("session:command:{id}"),
                &action,
                &self.compiled.views,
                None,
                0,
            )?;
            validate_command_action_requirements(
                &self.compiled.template_registry,
                &action,
                EvaluationStage::Operation,
                &format!("session command binding {id:?}"),
                0,
            )?;
        }
        if overflow_commands > 1 {
            bail!("session commands can define at most one overflow binding");
        }

        let mut aliases = BTreeMap::<&str, &str>::new();
        for (view_ref, view) in &self.compiled.views {
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
            let items = view.selected_items();
            engines.validate_view(view_ref, view, self.plugin_root(view_ref))?;
            self.validate_view_operation_requirements(view_ref, view)?;
            if let Some(items) = items {
                validate_toml_requirements(
                    &self.compiled.template_registry,
                    items,
                    EvaluationStage::Operation,
                    &format!("view {view_ref:?} items"),
                )?;
            }

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
                validate_command_action(
                    view_ref,
                    command_id,
                    &command.action,
                    &self.compiled.views,
                    self.plugin_root(view_ref),
                    0,
                )?;
            }
        }
        engines.validate_relations(self)?;

        Ok(())
    }

    fn validate_view_operation_requirements(&self, view_ref: &str, view: &View) -> Result<()> {
        for (field, value) in view.selected_engine_config() {
            validate_toml_requirements(
                &self.compiled.template_registry,
                value,
                EvaluationStage::Operation,
                &format!("view {view_ref:?} engine field {field:?}"),
            )?;
        }
        if let Some(keymap) = &view.keymap {
            validate_toml_requirements(
                &self.compiled.template_registry,
                keymap,
                EvaluationStage::Operation,
                &format!("view {view_ref:?} keymap"),
            )?;
        }
        validate_optional_string_requirements(
            &self.compiled.template_registry,
            view.run_shell.as_deref(),
            EvaluationStage::Operation,
            &format!("view {view_ref:?} run_shell"),
        )?;
        for (command_id, command) in &view.commands {
            validate_command_action_requirements(
                &self.compiled.template_registry,
                &command.action,
                EvaluationStage::Operation,
                &format!("view {view_ref:?} command {command_id:?}"),
                0,
            )?;
        }
        Ok(())
    }
}

fn validate_view_ref(view_ref: &str) -> Result<()> {
    let Some((plugin, view)) = view_ref.split_once(':') else {
        bail!("view reference {:?} must use plugin:view form", view_ref);
    };
    if plugin.is_empty() || view.is_empty() || view.contains(':') {
        bail!("invalid view reference {:?}", view_ref);
    }
    Ok(())
}
