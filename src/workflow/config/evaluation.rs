use super::Config;
use crate::expression::{
    Budget, ContextRequirements, EvalContext, EvaluationStage, Namespace, TemplateRegistry,
    clone_json_value_with_budget, evaluate_json_value_with_budget,
};
use crate::lifecycle::CancellationToken;
use anyhow::{Context, Result};
use serde_json::Value;
use std::{borrow::Cow, cell::RefCell};

#[derive(Debug, Clone, Copy)]
pub(crate) enum ConfigSource<'a> {
    Root,
    View(&'a str),
}

/// The immutable data required by dynamic configuration evaluation. Engines
/// may provide a narrow projection instead of retaining the complete Config.
pub(crate) trait EvaluationData {
    fn template_registry(&self) -> &TemplateRegistry;

    fn view_value(
        &self,
        view_ref: &str,
        parameters: &crate::parameter::ParameterSnapshot,
        binding_raw: Option<&str>,
    ) -> Result<Value>;
}

/// Immutable invocation data captured before the session starts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InvocationScope<'a> {
    input: &'a Value,
}

impl<'a> InvocationScope<'a> {
    pub(crate) fn new(input: &'a Value) -> Self {
        Self { input }
    }

    fn populate(
        self,
        requirements: &ContextRequirements,
        cancellation: Option<&CancellationToken>,
        budget: &mut Budget,
        context: &mut serde_json::Map<String, Value>,
    ) -> Result<()> {
        if requirements.requires(Namespace::Input) {
            context.insert(
                Namespace::Input.name().to_string(),
                clone_json_value_with_budget(self.input, cancellation, budget)?,
            );
        }
        Ok(())
    }
}

/// Immutable active-session data captured for one evaluation operation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SessionScope<'a> {
    runtime: &'a Value,
}

impl<'a> SessionScope<'a> {
    pub(crate) fn new(runtime: &'a Value) -> Self {
        Self { runtime }
    }

    fn populate(
        self,
        requirements: &ContextRequirements,
        cancellation: Option<&CancellationToken>,
        budget: &mut Budget,
        context: &mut serde_json::Map<String, Value>,
    ) -> Result<()> {
        if requirements.requires(Namespace::Page) {
            context.insert(
                Namespace::Page.name().to_string(),
                public_page_context(self.runtime, requirements, cancellation, budget)?,
            );
        }
        if requirements.requires(Namespace::Selection) {
            let default = Value::Null;
            let selection = self
                .runtime
                .pointer("/view/current/selected_item")
                .unwrap_or(&default);
            context.insert(
                Namespace::Selection.name().to_string(),
                clone_json_value_with_budget(selection, cancellation, budget)?,
            );
        }
        if requirements.requires(Namespace::Session) {
            context.insert(
                Namespace::Session.name().to_string(),
                public_session_context(self.runtime, requirements, cancellation, budget)?,
            );
        }
        Ok(())
    }
}

/// The View that owns the configuration value being evaluated.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OwnerViewScope<'a> {
    view_ref: &'a str,
    parameters: &'a crate::parameter::ParameterSnapshot,
    /// Feed defaults retain the coordinating page's committed raw binding.
    binding_raw: Option<&'a str>,
}

impl<'a> OwnerViewScope<'a> {
    pub(crate) fn new(
        view_ref: &'a str,
        parameters: &'a crate::parameter::ParameterSnapshot,
    ) -> Self {
        Self {
            view_ref,
            parameters,
            binding_raw: None,
        }
    }

    pub(crate) fn with_binding_raw(mut self, binding_raw: Option<&'a str>) -> Self {
        self.binding_raw = binding_raw;
        self
    }

    fn populate<D: EvaluationData + ?Sized>(
        self,
        data: &D,
        requirements: &ContextRequirements,
        cancellation: Option<&CancellationToken>,
        budget: &mut Budget,
        context: &mut serde_json::Map<String, Value>,
    ) -> Result<()> {
        if requirements.requires(Namespace::View) {
            let view = data.view_value(self.view_ref, self.parameters, self.binding_raw)?;
            context.insert(
                Namespace::View.name().to_string(),
                clone_json_value_with_budget(&view, cancellation, budget)?,
            );
        }
        Ok(())
    }
}

/// A returned value exposed only while preparing a continuation or result handler.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReturnScope<'a> {
    value: &'a Value,
}

impl<'a> ReturnScope<'a> {
    pub(crate) fn new(value: &'a Value) -> Self {
        Self { value }
    }

    fn populate(
        self,
        requirements: &ContextRequirements,
        cancellation: Option<&CancellationToken>,
        budget: &mut Budget,
        context: &mut serde_json::Map<String, Value>,
    ) -> Result<()> {
        if requirements.requires(Namespace::Result) {
            context.insert(
                Namespace::Result.name().to_string(),
                clone_json_value_with_budget(self.value, cancellation, budget)?,
            );
        }
        Ok(())
    }
}

/// Immutable scope composition captured for one configuration operation.
///
/// ConfigSource selects raw configuration independently. The owner scope can
/// therefore differ from the active session page while a feed is evaluated.
#[derive(Debug)]
pub(crate) struct EvaluationSnapshot<'a> {
    invocation: InvocationScope<'a>,
    session: SessionScope<'a>,
    owner: Option<OwnerViewScope<'a>>,
    current: Option<&'a Value>,
    current_fields: Option<&'a [&'static str]>,
    returned: Option<ReturnScope<'a>>,
    cancellation: Option<&'a CancellationToken>,
    budget: RefCell<Budget>,
}

impl<'a> EvaluationSnapshot<'a> {
    pub(crate) fn new(
        invocation: InvocationScope<'a>,
        session: SessionScope<'a>,
        owner: Option<OwnerViewScope<'a>>,
        cancellation: Option<&'a CancellationToken>,
    ) -> Self {
        Self {
            invocation,
            session,
            owner,
            current: None,
            current_fields: None,
            returned: None,
            cancellation,
            budget: RefCell::new(Budget::default()),
        }
    }

    pub(crate) fn with_current(mut self, current: &'a Value) -> Self {
        self.current = Some(current);
        self
    }

    pub(crate) fn with_current_fields(mut self, current_fields: &'a [&'static str]) -> Self {
        self.current_fields = Some(current_fields);
        self
    }

    pub(crate) fn with_return_scope(mut self, returned: Option<ReturnScope<'a>>) -> Self {
        self.returned = returned;
        self
    }

    fn owner_scope(&self) -> Result<OwnerViewScope<'a>> {
        self.owner
            .context("dynamic namespace \"view\" is unavailable without an owning View")
    }

    fn return_scope(&self) -> Result<ReturnScope<'a>> {
        self.returned
            .context("dynamic namespace \"result\" is unavailable in this operation")
    }

    fn expression_context<D: EvaluationData + ?Sized>(
        &self,
        data: &D,
        requirements: &ContextRequirements,
        budget: &mut Budget,
    ) -> Result<Value> {
        let mut context = serde_json::Map::new();
        self.invocation
            .populate(requirements, self.cancellation, budget, &mut context)?;
        self.session
            .populate(requirements, self.cancellation, budget, &mut context)?;
        if requirements.requires(Namespace::View) {
            self.owner_scope()?.populate(
                data,
                requirements,
                self.cancellation,
                budget,
                &mut context,
            )?;
        }
        if requirements.requires(Namespace::Current) {
            let current = self.current.context(
                "dynamic namespace \"current\" is unavailable without a mounted ViewContext",
            )?;
            requirements.validate_current_fields(
                self.current_fields
                    .unwrap_or(crate::expression::current_fields()),
            )?;
            context.insert(
                Namespace::Current.name().to_string(),
                crate::expression::clone_json_value_with_budget(
                    current,
                    self.cancellation,
                    budget,
                )?,
            );
        }
        if requirements.requires(Namespace::Result) {
            self.return_scope()?
                .populate(requirements, self.cancellation, budget, &mut context)?;
        }
        Ok(Value::Object(context))
    }

    pub(crate) fn resolve<D: EvaluationData + ?Sized>(
        &self,
        data: &D,
        stage: EvaluationStage,
        value: &Value,
    ) -> Result<Value> {
        let requirements = data.template_registry().requirements_for_value(value)?;
        requirements.validate_stage(stage, "runtime configuration value")?;
        let mut budget = self.budget.borrow_mut();
        let root = if requirements.is_empty() {
            Value::Null
        } else {
            self.expression_context(data, &requirements, &mut budget)?
        };
        let evaluator = EvalContext {
            root: &root,
            cancellation: self.cancellation,
            templates: Some(data.template_registry()),
        };
        evaluate_json_value_with_budget(value, &evaluator, &mut budget)
    }
}

impl Config {
    pub(crate) fn get(
        &self,
        source: ConfigSource<'_>,
        snapshot: &EvaluationSnapshot<'_>,
        stage: EvaluationStage,
        path: &[&str],
    ) -> Result<Option<Value>> {
        let raw = self.source_config_value(source)?;
        let Some(value) = path
            .iter()
            .try_fold(raw.as_ref(), |value, segment| value.get(segment))
        else {
            return Ok(None);
        };
        snapshot.resolve(self, stage, value).map(Some)
    }

    pub(crate) fn evaluate_value(
        &self,
        snapshot: &EvaluationSnapshot<'_>,
        stage: EvaluationStage,
        value: &toml::Value,
    ) -> Result<Value> {
        let value = super::toml_to_json(value)?;
        snapshot.resolve(self, stage, &value)
    }

    pub(crate) fn evaluate_argv(
        &self,
        configured: Option<&toml::Value>,
        snapshot: &EvaluationSnapshot<'_>,
        stage: EvaluationStage,
        label: &str,
    ) -> Result<Vec<String>> {
        let Some(configured) = configured else {
            return Ok(Vec::new());
        };
        let value = self.evaluate_value(snapshot, stage, configured)?;
        crate::execution::resolve_argv(Some(&value), label)
    }

    fn view_config(&self, view_ref: &str) -> Result<&Value> {
        let (plugin, view) = view_ref
            .split_once(':')
            .with_context(|| format!("invalid view reference {:?}", view_ref))?;
        self.compiled
            .config_value
            .get("plugins")
            .and_then(|plugins| plugins.get(plugin))
            .and_then(|plugin| plugin.get("views"))
            .and_then(|views| views.get(view))
            .with_context(|| format!("view {:?} configuration disappeared", view_ref))
    }

    fn source_config_value(&self, source: ConfigSource<'_>) -> Result<Cow<'_, Value>> {
        match source {
            ConfigSource::Root => Ok(Cow::Borrowed(&self.compiled.config_value)),
            ConfigSource::View(view_ref) => match self.view_config(view_ref) {
                Ok(value) => Ok(Cow::Borrowed(value)),
                Err(_)
                    if self
                        .compiled
                        .config_value
                        .as_object()
                        .is_some_and(|value| value.is_empty()) =>
                {
                    Ok(Cow::Owned(self.fallback_view_config_value(view_ref)?))
                }
                Err(error) => Err(error),
            },
        }
    }

    fn fallback_view_config_value(&self, view_ref: &str) -> Result<Value> {
        let view = self
            .view(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        let mut values = serde_json::Map::new();
        if let Some(keymap) = &view.keymap {
            values.insert("keymap".to_string(), super::toml_to_json(keymap)?);
        }
        if let Some(items) = view.selected_items() {
            values.insert("items".to_string(), super::toml_to_json(items)?);
        }
        for (name, value) in view.selected_engine_config() {
            values.insert(name.clone(), super::toml_to_json(value)?);
        }
        Ok(Value::Object(values))
    }
}

impl EvaluationData for Config {
    fn template_registry(&self) -> &TemplateRegistry {
        &self.compiled.template_registry
    }

    fn view_value(
        &self,
        view_ref: &str,
        parameters: &crate::parameter::ParameterSnapshot,
        binding_raw: Option<&str>,
    ) -> Result<Value> {
        Config::view_value(self, view_ref, parameters, binding_raw)
    }
}

pub(super) fn public_page_context(
    runtime: &Value,
    requirements: &ContextRequirements,
    cancellation: Option<&CancellationToken>,
    budget: &mut Budget,
) -> Result<Value> {
    let current = runtime.pointer("/view/current").and_then(Value::as_object);
    let mut page = serde_json::Map::new();
    let fields = [
        ("ref", Value::Null),
        ("input", Value::String(String::new())),
        ("raw_input", Value::String(String::new())),
        ("query", Value::Null),
        ("state_revision", Value::Null),
        ("items", serde_json::json!([])),
        ("selected_item", Value::Null),
        ("command_owner", Value::Null),
    ];
    for (field, default) in fields {
        if requirements.requires_field(Namespace::Page, field) {
            let value = current
                .and_then(|values| values.get(field))
                .unwrap_or(&default);
            page.insert(
                field.to_string(),
                clone_json_value_with_budget(value, cancellation, budget)?,
            );
        }
    }
    if requirements.requires_field(Namespace::Page, "commands") {
        let default = Value::Array(Vec::new());
        let value = current
            .and_then(|values| values.get("command").or_else(|| values.get("commands")))
            .unwrap_or(&default);
        page.insert(
            "commands".to_string(),
            clone_json_value_with_budget(value, cancellation, budget)?,
        );
    }
    Ok(Value::Object(page))
}

fn public_session_context(
    runtime: &Value,
    requirements: &ContextRequirements,
    cancellation: Option<&CancellationToken>,
    budget: &mut Budget,
) -> Result<Value> {
    let session = runtime.get("session").and_then(Value::as_object);
    let mut public = serde_json::Map::new();
    if requirements.requires_field(Namespace::Session, "input") {
        let default = Value::Object(serde_json::Map::new());
        let value = session
            .and_then(|values| values.get("input"))
            .unwrap_or(&default);
        public.insert(
            "input".to_string(),
            clone_json_value_with_budget(value, cancellation, budget)?,
        );
    }
    if requirements.requires_field(Namespace::Session, "views") {
        let default = Value::Array(Vec::new());
        let value = session
            .and_then(|values| values.get("views"))
            .unwrap_or(&default);
        public.insert(
            "views".to_string(),
            clone_json_value_with_budget(value, cancellation, budget)?,
        );
    }
    Ok(Value::Object(public))
}
