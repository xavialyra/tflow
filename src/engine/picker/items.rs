#[cfg(test)]
use crate::config::Config;
use crate::config::{
    EvaluationData, EvaluationSnapshot, InvocationScope, OwnerViewScope, PickerItemsProjection,
    ResolvedScriptSource, SessionScope, toml_to_json,
};
use crate::execution::{ensure_script_success, run_script};
use crate::expression::EvaluationStage;
use crate::input::{InputSourceIdentity, ViewMountId};
use crate::lifecycle::CancellationToken;
use crate::parameter::{ParameterBinding, ParameterSnapshot};
use crate::terminal::sanitize_text;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

const MAX_ITEMS_PER_SESSION: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FeedId(pub(crate) String);

/// Immutable compiled metadata for one Picker feed mount.
#[derive(Clone)]
pub(crate) struct FeedDefinition {
    pub(crate) feed_id: FeedId,
    pub(crate) owner_view: String,
    pub(crate) alias: Option<String>,
    pub(crate) binding: ParameterBinding,
    items: Option<Value>,
    source: Arc<PickerItemsProjection>,
}

impl fmt::Debug for FeedDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FeedDefinition")
            .field("feed_id", &self.feed_id)
            .field("owner_view", &self.owner_view)
            .field("alias", &self.alias)
            .field("items", &self.items)
            .finish_non_exhaustive()
    }
}

impl FeedDefinition {
    pub(crate) fn collection(
        projection: Arc<PickerItemsProjection>,
        page_view: &str,
    ) -> Result<Arc<Vec<Arc<Self>>>> {
        let definitions = projection
            .feed_views(page_view)?
            .into_iter()
            .map(|(owner_view, view)| {
                let items = view.items.as_ref().map(toml_to_json).transpose()?;
                Ok(Arc::new(Self {
                    feed_id: FeedId(owner_view.clone()),
                    owner_view,
                    alias: view.alias.clone(),
                    binding: view.binding.clone(),
                    items,
                    source: Arc::clone(&projection),
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Arc::new(definitions))
    }

    fn has_items(&self) -> bool {
        self.items.is_some()
    }

    fn items_value(&self, snapshot: &EvaluationSnapshot<'_>) -> Result<Option<Value>> {
        let Some(items) = self.items.as_ref() else {
            return Ok(None);
        };
        snapshot
            .resolve(self, EvaluationStage::Operation, items)
            .map(Some)
    }

    fn input_value(&self) -> &Value {
        self.source.input_value()
    }

    fn plugin_root(&self) -> Option<&Path> {
        self.source.plugin_root(&self.owner_view)
    }
}

impl EvaluationData for FeedDefinition {
    fn template_registry(&self) -> &crate::expression::TemplateRegistry {
        self.source.template_registry()
    }

    fn view_value(
        &self,
        view_ref: &str,
        parameters: &ParameterSnapshot,
        binding_raw: Option<&str>,
    ) -> Result<Value> {
        anyhow::ensure!(
            view_ref == self.owner_view,
            "feed definition {:?} cannot evaluate view {:?}",
            self.owner_view,
            view_ref
        );
        let state = self.binding.state_from_snapshot(parameters, false)?;
        let input = match binding_raw {
            Some(raw) => raw.to_string(),
            None => self.binding.render_input(&state)?,
        };
        Ok(serde_json::json!({
            "ref": self.owner_view,
            "query": self.binding.parameter_values(&state)?,
            "input": input,
            "raw_input": input,
            "state_revision": state.revision(),
        }))
    }
}

/// Parameters resolved for one request and one immutable feed definition.
#[derive(Debug, Clone)]
pub(crate) struct FeedInstance {
    pub(crate) definition: Arc<FeedDefinition>,
    pub(crate) parameters: ParameterSnapshot,
    pub(crate) binding_raw: String,
}

impl FeedInstance {
    pub(crate) fn resolve(
        definition: Arc<FeedDefinition>,
        page_view: &str,
        page_parameters: &ParameterSnapshot,
        binding_raw: &str,
    ) -> Result<Self> {
        let parameters = if definition.owner_view == page_view {
            page_parameters.clone()
        } else {
            let mut state = definition.binding.instantiate()?;
            definition
                .binding
                .bind_feed_input(&mut state, binding_raw)?;
            definition.binding.validate_instance(&state)?;
            ParameterSnapshot::from_parts(
                definition.binding.parameter_values(&state)?,
                state.raw_input().to_string(),
                InputSourceIdentity::default(),
                state.revision(),
            )
        };
        Ok(Self {
            definition,
            parameters,
            binding_raw: binding_raw.to_string(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct ItemValue {
    label: String,
    #[serde(default)]
    allow_empty: bool,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub(crate) prefix: String,
    pub(crate) text: String,
    pub(crate) value: Option<String>,
    pub(crate) metadata: Value,
    /// Stable provenance only; the response-level context owns state and raw binding.
    pub(crate) source_view: String,
    pub(crate) feed_id: FeedId,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ItemsResult {
    pub(crate) items: Vec<Item>,
    pub(crate) contexts: BTreeMap<FeedId, FeedInstance>,
    pub(crate) errors: Vec<String>,
}

/// The complete identity of one Picker feed request.
///
/// All fields participate in stale-result validation. `source` carries the
/// input generation; `generation` identifies the request itself.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FeedRequestIdentity {
    pub(super) mount_id: ViewMountId,
    pub(super) source: InputSourceIdentity,
    pub(super) generation: u64,
    pub(super) input_generation: u64,
    pub(super) input: String,
    pub(super) parameter_revision: u64,
    pub(super) binding_raw: String,
    pub(super) page_parameters: ParameterSnapshot,
}

impl FeedRequestIdentity {
    pub(super) fn new(
        mount_id: ViewMountId,
        source: InputSourceIdentity,
        generation: u64,
        input: String,
        parameter_revision: u64,
        binding_raw: String,
        page_parameters: ParameterSnapshot,
    ) -> Result<Self> {
        let identity = Self {
            mount_id,
            source,
            generation,
            input_generation: source.generation,
            input,
            parameter_revision,
            binding_raw,
            page_parameters,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub(super) fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.mount_id == self.source.frame,
            "picker feed request source does not belong to mount {:?}",
            self.mount_id
        );
        anyhow::ensure!(
            self.input_generation == self.source.generation,
            "picker feed request input generation does not match its source"
        );
        anyhow::ensure!(
            self.generation > 0,
            "picker feed request generation must be greater than zero"
        );
        anyhow::ensure!(
            self.page_parameters.source() == self.source,
            "picker feed request source does not match page parameters"
        );
        anyhow::ensure!(
            self.page_parameters.revision() == self.parameter_revision,
            "picker feed request parameter revision does not match page parameters"
        );
        Ok(())
    }

    /// Compare the request fields bound to a ViewContext. Request generation
    /// is intentionally excluded; it identifies a task, not the context.
    pub(super) fn matches_context(
        &self,
        mount_id: ViewMountId,
        input: &str,
        binding_raw: &str,
        page_parameters: &ParameterSnapshot,
    ) -> bool {
        self.mount_id == mount_id
            && self.source.frame == mount_id
            && self.source == page_parameters.source()
            && self.input_generation == page_parameters.source().generation
            && self.input == input
            && self.parameter_revision == page_parameters.revision()
            && self.binding_raw == binding_raw
            && self.page_parameters == *page_parameters
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ItemsRequest {
    pub(super) view: String,
    pub(super) identity: FeedRequestIdentity,
}

impl ItemsRequest {
    pub(super) fn new(view: String, identity: FeedRequestIdentity) -> Result<Self> {
        anyhow::ensure!(!view.is_empty(), "picker feed request view is empty");
        identity.validate()?;
        Ok(Self { view, identity })
    }

    pub(super) fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.view.is_empty(), "picker feed request view is empty");
        self.identity.validate()
    }

    pub(super) fn matches_context(
        &self,
        mount_id: ViewMountId,
        view: &str,
        input: &str,
        binding_raw: &str,
        page_parameters: &ParameterSnapshot,
    ) -> bool {
        self.view == view
            && self
                .identity
                .matches_context(mount_id, input, binding_raw, page_parameters)
    }

    pub(super) fn matches_response(&self, view: &str, identity: &FeedRequestIdentity) -> bool {
        self.view == view && self.identity == *identity
    }
}

#[derive(Debug, Clone)]
pub(super) struct ItemsResponse {
    pub(super) view: String,
    pub(super) identity: FeedRequestIdentity,
    pub(super) result: std::result::Result<ItemsResult, String>,
}

pub(crate) struct ItemsEvent {
    pub(crate) view: String,
    pub(crate) errors: Vec<String>,
    pub(crate) failure: Option<String>,
}

pub(crate) type ItemsTaskHandle = crate::task::TaskHandle<ItemsResponse>;

pub(crate) trait PickerItemsLoader: Send + Sync {
    fn load(
        &self,
        request: &ItemsRequest,
        runtime: &Value,
        cancellation: &CancellationToken,
    ) -> Result<ItemsResult>;
}

pub(crate) fn load_items_for_definitions(
    definitions: &[Arc<FeedDefinition>],
    page_view: &str,
    page_parameters: &ParameterSnapshot,
    binding_raw: &str,
    runtime: &Value,
    cancellation: &CancellationToken,
) -> Result<ItemsResult> {
    let mut result = ItemsResult::default();

    for definition in definitions {
        if cancellation.is_cancelled() {
            return Ok(result);
        }
        if result.items.len() >= MAX_ITEMS_PER_SESSION {
            result.errors.push(format!(
                "items exceeded the session limit of {}",
                MAX_ITEMS_PER_SESSION
            ));
            break;
        }
        if !definition.has_items() {
            continue;
        }
        let prefix = definition
            .alias
            .clone()
            .unwrap_or_else(|| definition.owner_view.clone());
        let instance = match FeedInstance::resolve(
            Arc::clone(definition),
            page_view,
            page_parameters,
            binding_raw,
        ) {
            Ok(instance) => instance,
            Err(error) => {
                result
                    .errors
                    .push(format!("{}: {}", definition.owner_view, error));
                continue;
            }
        };
        let feed_id = definition.feed_id.clone();
        result.contexts.insert(feed_id.clone(), instance.clone());
        let owner_scope =
            OwnerViewScope::new(&instance.definition.owner_view, &instance.parameters)
                .with_binding_raw(Some(&instance.binding_raw));
        let snapshot = EvaluationSnapshot::new(
            InvocationScope::new(instance.definition.input_value()),
            SessionScope::new(runtime),
            Some(owner_scope),
            Some(cancellation),
        );
        let value = match instance.definition.items_value(&snapshot) {
            Ok(Some(value)) if ResolvedScriptSource::is_candidate(&value) => {
                ResolvedScriptSource::parse(&value)
                    .and_then(|source| {
                        run_items_source(
                            &instance.definition,
                            &instance.definition.owner_view,
                            &source,
                            cancellation,
                        )
                    })
                    .map(Some)
            }
            value => value,
        };
        let value = match value {
            Ok(Some(value)) => value,
            Ok(None) => continue,
            Err(error) => {
                result
                    .errors
                    .push(format!("{}: {}", definition.owner_view, error));
                continue;
            }
        };
        append_items_value(
            &mut result,
            &definition.owner_view,
            &feed_id,
            &prefix,
            value,
            cancellation,
        );
        if cancellation.is_cancelled() {
            return Ok(result);
        }
    }

    Ok(result)
}

#[cfg(test)]
pub(crate) fn load_items_for_page(
    projection: &PickerItemsProjection,
    page_view: &str,
    page_parameters: &ParameterSnapshot,
    binding_raw: &str,
    runtime: &Value,
    cancellation: &CancellationToken,
) -> Result<ItemsResult> {
    let definitions = FeedDefinition::collection(Arc::new(projection.clone()), page_view)?;
    load_items_for_definitions(
        &definitions,
        page_view,
        page_parameters,
        binding_raw,
        runtime,
        cancellation,
    )
}

#[cfg(test)]
fn load_items(
    config: &Config,
    view_ref: &str,
    runtime: &Value,
    cancellation: &CancellationToken,
) -> Result<ItemsResult> {
    let mut config = config.clone();
    config.test_rebuild_compiled()?;
    let page_state = config.instantiate_parameters(view_ref)?;
    let page_parameters = config.parameter_snapshot(&page_state, InputSourceIdentity::default())?;
    let projection = PickerItemsProjection::from_config(&config, view_ref)?;
    load_items_for_page(
        &projection,
        view_ref,
        &page_parameters,
        "",
        runtime,
        cancellation,
    )
}

fn append_items_value(
    result: &mut ItemsResult,
    source_ref: &str,
    feed_id: &FeedId,
    prefix: &str,
    value: Value,
    cancellation: &CancellationToken,
) {
    if !value.is_array() {
        result.errors.push(format!(
            "{}: items must resolve to an array or a script source, got {}",
            source_ref,
            value_type(&value)
        ));
        return;
    }
    append_items_array(result, source_ref, feed_id, prefix, value, cancellation);
}

fn append_items_array(
    result: &mut ItemsResult,
    source_ref: &str,
    feed_id: &FeedId,
    prefix: &str,
    value: Value,
    cancellation: &CancellationToken,
) {
    let Value::Array(items) = value else {
        result.errors.push(format!(
            "{}: items source must produce a JSON array",
            source_ref
        ));
        return;
    };
    let remaining = MAX_ITEMS_PER_SESSION.saturating_sub(result.items.len());
    if items.len() > remaining {
        result.errors.push(format!(
            "{}: items exceeded the session limit of {}",
            source_ref, MAX_ITEMS_PER_SESSION
        ));
        return;
    }
    let mut parsed_items = Vec::with_capacity(items.len());
    for (index, value) in items.into_iter().enumerate() {
        if cancellation.is_cancelled() {
            return;
        }
        let parsed = match serde_json::from_value::<ItemValue>(value) {
            Ok(item) => item,
            Err(error) => {
                result.errors.push(format!(
                    "{}: invalid items JSON at index {}: {}",
                    source_ref, index, error
                ));
                return;
            }
        };
        let text = sanitize_text(&parsed.label);
        if text.is_empty() && !parsed.allow_empty {
            result.errors.push(format!(
                "{}: items JSON at index {} has an empty label",
                source_ref, index
            ));
            return;
        }
        parsed_items.push(Item {
            prefix: prefix.to_string(),
            text,
            value: parsed.value,
            metadata: parsed.metadata,
            source_view: source_ref.to_string(),
            feed_id: feed_id.clone(),
        });
    }
    result.items.extend(parsed_items);
}

#[cfg(test)]
fn append_items(
    result: &mut ItemsResult,
    source_ref: &str,
    feed_id: &FeedId,
    prefix: &str,
    value: Value,
    cancellation: &CancellationToken,
) {
    append_items_array(result, source_ref, feed_id, prefix, value, cancellation);
}

fn run_items_source(
    definition: &FeedDefinition,
    source_ref: &str,
    source: &ResolvedScriptSource,
    cancellation: &CancellationToken,
) -> Result<Value> {
    let root = definition
        .plugin_root()
        .with_context(|| format!("items source {:?} has no plugin root", source_ref))?;
    let args = source.script_args("picker script args")?;
    let output = run_script(
        root,
        &source.file,
        &args,
        source.max_output_bytes,
        cancellation,
    )?;
    ensure_script_success(&output)?;
    if output.stdout.is_empty() {
        bail!("script produced no JSON output");
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("script {} did not produce valid JSON", source.file))
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Command, CommandAction, ENGINE_PICKER, EngineOptions, EngineSpec, FeedSpec, PluginMetadata,
        View,
    };
    use std::collections::BTreeMap;
    use std::env;
    use std::fs;
    use std::sync::Arc;

    fn script_source(file: &str, args: Option<toml::Value>) -> toml::Value {
        let mut source = toml::Table::new();
        source.insert("source".to_string(), "script".into());
        source.insert("file".to_string(), file.into());
        if let Some(args) = args {
            source.insert("args".to_string(), args);
        }
        toml::Value::Table(source)
    }

    fn test_config() -> Config {
        let mut views = BTreeMap::new();
        views.insert(
            "core:default".to_string(),
            View {
                engine: EngineSpec {
                    engine_type: ENGINE_PICKER.to_string(),
                    config: EngineOptions {
                        feeds: vec![FeedSpec {
                            view: "apps:main".to_string(),
                        }],
                        ..Default::default()
                    },
                },
                alias: None,
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                keymap: None,
                commands: BTreeMap::new(),
            },
        );
        views.insert(
            "apps:main".to_string(),
            View {
                engine: EngineSpec {
                    engine_type: ENGINE_PICKER.to_string(),
                    config: EngineOptions {
                        items: Some("{{ page.items }}".into()),
                        ..Default::default()
                    },
                },
                alias: Some("app".to_string()),
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                keymap: None,
                commands: BTreeMap::from([(
                    "open".to_string(),
                    Command {
                        key: Some("enter".to_string()),
                        label: "Open".to_string(),
                        scope: crate::config::CommandScope::Selection,
                        requires: crate::config::CommandRequirement::Items,
                        passthrough: false,
                        action: CommandAction::Run {
                            payload: crate::config::RunPayload {
                                handler: crate::config::ScriptSourceSpec::script_file(
                                    "scripts/run.sh",
                                )
                                .as_toml_value(),
                                args: None,
                                shell: None,
                                exit: false,
                            },
                        },
                    },
                )]),
            },
        );
        Config::test_new(
            Some("core:default".to_string()),
            views,
            BTreeMap::from([
                (
                    "core".to_string(),
                    PluginMetadata {
                        name: "core".to_string(),
                    },
                ),
                (
                    "apps".to_string(),
                    PluginMetadata {
                        name: "applications".to_string(),
                    },
                ),
            ]),
            BTreeMap::new(),
            Value::Object(serde_json::Map::new()),
        )
        .unwrap()
    }

    #[test]
    fn parses_structured_items() {
        let item: ItemValue = serde_json::from_str(
            r#"{"label":"Termius","value":"termius.desktop","metadata":{"kind":"app"}}"#,
        )
        .unwrap();
        assert_eq!(item.label, "Termius");
        assert_eq!(item.value.as_deref(), Some("termius.desktop"));
        assert_eq!(item.metadata["kind"], "app");
    }

    #[test]
    fn item_aggregation_has_a_session_limit_and_observes_cancellation() {
        let mut result = ItemsResult::default();
        let items = Value::Array(
            (0..=MAX_ITEMS_PER_SESSION)
                .map(|index| serde_json::json!({"label": index.to_string()}))
                .collect(),
        );
        append_items(
            &mut result,
            "feed:main",
            &FeedId("feed:main".to_string()),
            "feed",
            items,
            &CancellationToken::new(),
        );
        assert!(result.items.is_empty());
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.contains("session limit"))
        );

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        append_items(
            &mut result,
            "feed:main",
            &FeedId("feed:main".to_string()),
            "feed",
            serde_json::json!([{"label": "ignored"}]),
            &cancellation,
        );
        assert!(result.items.is_empty());
    }

    #[test]
    fn items_keep_their_source_view() {
        let result = load_items(
            &test_config(),
            "core:default",
            &serde_json::json!({
                "view": {
                    "current": {
                        "items": [{"label": "Second"}, {"label": "First"}]
                    }
                }
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            result
                .items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["Second", "First"]
        );
        assert_eq!(result.items[0].source_view, "apps:main");
        assert_eq!(result.items[0].prefix, "app");
        assert_eq!(result.contexts.len(), 1, "one context per feed response");
        assert!(
            result
                .items
                .iter()
                .all(|item| item.feed_id == FeedId("apps:main".to_string()))
        );
    }

    #[test]
    fn feed_definitions_create_independent_instances_for_single_and_aggregate_mounts() {
        let mut config = test_config();
        config.test_views_mut().insert(
            "sys:main".to_string(),
            View {
                engine: EngineSpec {
                    engine_type: ENGINE_PICKER.to_string(),
                    config: EngineOptions {
                        items: Some(toml::Value::Array(vec![toml::Value::Table(
                            [("label".to_string(), "SysItem".into())]
                                .into_iter()
                                .collect(),
                        )])),
                        ..Default::default()
                    },
                },
                alias: Some("sys".to_string()),
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                keymap: None,
                commands: BTreeMap::new(),
            },
        );
        config
            .test_views_mut()
            .get_mut("core:default")
            .unwrap()
            .engine
            .config
            .feeds = vec![
            FeedSpec {
                view: "apps:main".to_string(),
            },
            FeedSpec {
                view: "sys:main".to_string(),
            },
        ];
        let projection =
            Arc::new(PickerItemsProjection::from_config(&config, "core:default").unwrap());
        let ordinary_definitions =
            FeedDefinition::collection(Arc::clone(&projection), "apps:main").unwrap();
        let aggregate_definitions = FeedDefinition::collection(projection, "core:default").unwrap();
        assert_eq!(ordinary_definitions.len(), 1);
        assert_eq!(aggregate_definitions.len(), 2);
        assert_eq!(
            aggregate_definitions
                .iter()
                .map(|definition| definition.owner_view.as_str())
                .collect::<Vec<_>>(),
            ["apps:main", "sys:main"]
        );

        let ordinary_source = InputSourceIdentity {
            frame: ViewMountId(201),
            generation: 0,
        };
        let ordinary_parameters = ParameterSnapshot::from_parts(
            serde_json::json!("ordinary"),
            "ordinary".to_string(),
            ordinary_source,
            1,
        );
        let ordinary = load_items_for_definitions(
            &ordinary_definitions,
            "apps:main",
            &ordinary_parameters,
            "ordinary",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(ordinary.contexts.len(), 1);
        assert!(Arc::ptr_eq(
            &ordinary.contexts[&FeedId("apps:main".to_string())].definition,
            &ordinary_definitions[0]
        ));

        let aggregate_parameters = ParameterSnapshot::from_parts(
            serde_json::json!("page"),
            "page".to_string(),
            InputSourceIdentity {
                frame: ViewMountId(202),
                generation: 0,
            },
            1,
        );
        let runtime = serde_json::json!({
            "view": {"current": {"items": [{"label": "AppItem"}]}}
        });
        let first_mount = load_items_for_definitions(
            &aggregate_definitions,
            "core:default",
            &aggregate_parameters,
            "first-mount",
            &runtime,
            &CancellationToken::new(),
        )
        .unwrap();
        let second_mount = load_items_for_definitions(
            &aggregate_definitions,
            "core:default",
            &aggregate_parameters,
            "second-mount",
            &runtime,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(first_mount.contexts.len(), 2);
        assert_eq!(second_mount.contexts.len(), 2);
        let first_app = &first_mount.contexts[&FeedId("apps:main".to_string())];
        let second_app = &second_mount.contexts[&FeedId("apps:main".to_string())];
        assert!(Arc::ptr_eq(
            &first_app.definition,
            &aggregate_definitions[0]
        ));
        assert!(Arc::ptr_eq(
            &second_app.definition,
            &aggregate_definitions[0]
        ));
        assert!(!std::ptr::eq(first_app, second_app));
        assert_eq!(first_app.binding_raw, "first-mount");
        assert_eq!(second_app.binding_raw, "second-mount");
        assert_ne!(
            first_app.parameters.values(),
            second_app.parameters.values()
        );
        assert_eq!(
            first_app.parameters.values(),
            &serde_json::json!("first-mount")
        );
        assert_eq!(
            second_app.parameters.values(),
            &serde_json::json!("second-mount")
        );
    }

    #[test]
    fn feed_owner_and_active_page_are_distinct_dynamic_roots() {
        let mut config = test_config();
        config
            .test_views_mut()
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(toml::Value::Array(vec![toml::Value::Table(
            [(
                "label".to_string(),
                "{{ view.ref }} <- {{ page.ref }}".into(),
            )]
            .into_iter()
            .collect(),
        )]));
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {
                    "current": {
                        "ref": "core:default",
                        "input": "",
                        "raw_input": "",
                        "query": "",
                        "items": []
                    }
                }
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.items[0].text, "apps:main <- core:default");
    }

    #[test]
    fn called_picker_items_read_declared_query_values() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut state = config.instantiate_parameters("selectors:commands").unwrap();
        config
            .update_sanitized_initial_parameter_values(
                &mut state,
                &serde_json::json!({
                    "commands": [{
                        "ref": {"view": "apps:main", "id": "open"},
                        "owner": "apps:main",
                        "key": "enter",
                        "label": "Open",
                    }],
                }),
            )
            .unwrap();
        let projection = PickerItemsProjection::from_config(&config, "selectors:commands").unwrap();
        let result = load_items_for_page(
            &projection,
            "selectors:commands",
            &config
                .parameter_snapshot(&state, InputSourceIdentity::default())
                .unwrap(),
            "open",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.items.iter().any(|item| item.metadata["command"]
            == serde_json::json!({"view": "apps:main", "id": "open"})));
    }

    #[test]
    fn evaluated_items_require_json_arrays() {
        let mut config = test_config();
        config
            .test_views_mut()
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some("{{ page.query }}".into());
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {"current": {"query": "not-an-array"}}
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.items.is_empty());
        assert!(result.errors[0].contains("items must resolve to an array"));
    }

    #[test]
    fn feed_sources_cannot_read_the_mounted_current_projection() {
        let mut config = test_config();
        config
            .test_views_mut()
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some("{{ current.value }}".into());
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {"current": {"value": "must-not-leak-into-feed"}}
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.items.is_empty());
        assert!(
            result
                .errors
                .iter()
                .any(|error| { error.contains("namespace \"current\" is unavailable") })
        );
    }

    #[test]
    fn dynamic_item_labels_are_evaluated_recursively() {
        let mut config = test_config();
        config
            .test_views_mut()
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(toml::Value::Array(vec![toml::Value::Table(
            [("label".to_string(), "{{ page.input }}".into())]
                .into_iter()
                .collect(),
        )]));

        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({"view": {"current": {"input": "dynamic"}}}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.items[0].text, "dynamic");
    }

    #[test]
    fn path_results_can_become_script_sources() {
        let root = env::temp_dir().join(format!(
            "tui-launcher-items-dynamic-source-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("items.sh"),
            "printf '%s\\n' '[{\"label\":\"manufactured source\"}]'\n",
        )
        .unwrap();

        let mut config = test_config();
        config
            .test_views_mut()
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some("{{ page.query }}".into());
        *config.test_config_value_mut() = serde_json::json!({
            "plugins": {
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "items": "{{ page.query }}"
                        }
                    }
                }
            }
        });
        config.test_rebuild_parameter_registry().unwrap();
        config.rebuild_template_registry().unwrap();
        config
            .test_plugin_roots_mut()
            .insert("apps".to_string(), root.clone());
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {"current": {"query": {"source": "script", "file": "items.sh"}}}
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.items[0].text, "manufactured source");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_items_are_reported_without_partial_results() {
        let result = load_items(
            &test_config(),
            "core:default",
            &serde_json::json!({
                "view": {
                    "current": {
                        "items": [{"label": "Valid"}, {"value": "missing-label"}]
                    }
                }
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.items.is_empty());
        assert!(result.errors[0].contains("index 1"));
    }

    #[test]
    fn dynamic_source_args_pass_view_query_to_scripts() {
        let root =
            env::temp_dir().join(format!("tui-launcher-items-script-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("items.sh"),
            "jq -cn --arg label \"$1\" '[{label: $label}]'\n",
        )
        .unwrap();

        let mut config = test_config();
        config
            .test_views_mut()
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(script_source(
            "items.sh",
            Some(toml::Value::Array(vec!["{{ view.query }}".into()])),
        ));
        *config.test_config_value_mut() = serde_json::json!({
            "plugins": {
                "core": {
                    "views": {
                        "default": {"type": "picker", "feeds": [{"view": "apps:main"}]}
                    }
                },
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "alias": "app",
                            "items": {"source": "script", "file": "items.sh", "args": ["{{ view.query }}"]}
                        }
                    }
                }
            }
        });
        config.test_rebuild_parameter_registry().unwrap();
        config.rebuild_template_registry().unwrap();
        config
            .test_plugin_roots_mut()
            .insert("apps".to_string(), root.clone());
        let page_state = config.instantiate_parameters("core:default").unwrap();
        let page_parameters = config
            .parameter_snapshot(&page_state, InputSourceIdentity::default())
            .unwrap();
        let projection = PickerItemsProjection::from_config(&config, "core:default").unwrap();
        let result = load_items_for_page(
            &projection,
            "core:default",
            &page_parameters,
            "fire",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            result.items.len(),
            1,
            "items={:?} errors={:?}",
            result.items,
            result.errors
        );
        assert_eq!(result.items[0].text, "fire");
        assert_eq!(result.contexts.len(), 1);
        let context = &result.contexts[&result.items[0].feed_id];
        assert_eq!(context.binding_raw, "fire");
        assert_eq!(context.parameters.values(), &serde_json::json!("fire"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn feed_binding_raw_input_keeps_empty_binding() {
        let root = env::temp_dir().join(format!("tui-launcher-items-raw-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("items.sh"),
            concat!(
                "raw=$1\n",
                "text=$2\n",
                "jq -cn --arg raw \"$raw\" --arg text \"$text\" \
",
                "  '[{label:(\"RAW:\" + $raw + \"|TEXT:\" + $text)}]'\n",
            ),
        )
        .unwrap();

        let mut config = test_config();
        config
            .test_views_mut()
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(script_source(
            "items.sh",
            Some(toml::Value::Array(vec![
                "{{ view.raw_input }}".into(),
                "{{ view.query.text }}".into(),
            ])),
        ));
        *config.test_config_value_mut() = serde_json::json!({
            "plugins": {
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "query": {
                                "type": "object",
                                "input_order": ["text"],
                                "text": {"type": "string", "default": "source-default"}
                            },
                            "items": {"source": "script", "file": "items.sh", "args": ["{{ view.raw_input }}", "{{ view.query.text }}"]}
                        }
                    }
                }
            }
        });
        config.test_rebuild_parameter_registry().unwrap();
        config.rebuild_template_registry().unwrap();
        config
            .test_plugin_roots_mut()
            .insert("apps".to_string(), root.clone());
        let page_state = config.instantiate_parameters("core:default").unwrap();
        let page_parameters = config
            .parameter_snapshot(&page_state, InputSourceIdentity::default())
            .unwrap();
        let projection = PickerItemsProjection::from_config(&config, "core:default").unwrap();
        let result = load_items_for_page(
            &projection,
            "core:default",
            &page_parameters,
            "",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            result.items[0].text, "RAW:|TEXT:source-default",
            "errors={:?}",
            result.errors
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn required_feed_failure_does_not_block_a_later_defaulted_feed() {
        let root = env::temp_dir().join(format!(
            "tui-launcher-items-required-isolation-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        let invalid_provider_marker = root.join("invalid-provider-ran");
        fs::write(
            root.join("invalid-items.sh"),
            "printf 'ran\\n' > invalid-provider-ran\nprintf '[]\\n'\n",
        )
        .unwrap();
        fs::write(
            root.join("items.sh"),
            concat!(
                "payload=$1\n",
                "raw=$(printf '%s' \"$payload\" | jq -r .raw_input)\n",
                "text=$(printf '%s' \"$payload\" | jq -r .query.text)\n",
                "jq -cn --arg raw \"$raw\" --arg text \"$text\" ",
                "'[{label:(\"RAW:\" + $raw + \"|TEXT:\" + $text)}]'\n",
            ),
        )
        .unwrap();

        let mut config = test_config();
        config.test_views_mut().insert(
            "sys:main".to_string(),
            View {
                engine: EngineSpec {
                    engine_type: ENGINE_PICKER.to_string(),
                    config: EngineOptions {
                        items: Some(script_source(
                            "items.sh",
                            Some(toml::Value::Array(vec!["{{ view }}".into()])),
                        )),
                        ..Default::default()
                    },
                },
                alias: Some("sys".to_string()),
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                keymap: None,
                commands: BTreeMap::new(),
            },
        );
        config
            .test_views_mut()
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(script_source("invalid-items.sh", None));
        config
            .test_views_mut()
            .get_mut("core:default")
            .unwrap()
            .engine
            .config
            .feeds = vec![
            FeedSpec {
                view: "apps:main".to_string(),
            },
            FeedSpec {
                view: "sys:main".to_string(),
            },
        ];
        *config.test_config_value_mut() = serde_json::json!({
            "plugins": {
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "query": {
                                "type": "object",
                                "input_order": [],
                                "token": {"type": "string"}
                            },
                            "items": {"source": "script", "file": "invalid-items.sh"}
                        }
                    }
                },
                "sys": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "query": {
                                "type": "object",
                                "input_order": ["text"],
                                "text": {"type": "string", "default": "later-default"}
                            },
                            "items": {"source": "script", "file": "items.sh", "args": ["{{ view }}"]}
                        }
                    }
                }
            }
        });
        config.test_rebuild_parameter_registry().unwrap();
        config.rebuild_template_registry().unwrap();
        config
            .test_plugin_roots_mut()
            .insert("apps".to_string(), root.clone());
        config
            .test_plugin_roots_mut()
            .insert("sys".to_string(), root.clone());
        let page_state = config.instantiate_parameters("core:default").unwrap();
        let page_parameters = config
            .parameter_snapshot(&page_state, InputSourceIdentity::default())
            .unwrap();
        let projection = PickerItemsProjection::from_config(&config, "core:default").unwrap();
        let result = load_items_for_page(
            &projection,
            "core:default",
            &page_parameters,
            "",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();

        assert_eq!(result.items.len(), 1, "errors={:?}", result.errors);
        assert_eq!(result.items[0].text, "RAW:|TEXT:later-default");
        assert_eq!(result.items[0].source_view, "sys:main");
        assert_eq!(result.contexts.len(), 1);
        assert!(
            result
                .contexts
                .contains_key(&FeedId("sys:main".to_string()))
        );
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("apps:main"));
        assert!(result.errors[0].contains("--token is required"));
        assert!(
            !invalid_provider_marker.exists(),
            "invalid feed provider ran before state validation"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nonempty_binding_does_not_run_a_feed_without_ordered_fields() {
        let mut config = test_config();
        config
            .test_views_mut()
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some("{{ page.query.provider_should_not_run }}".into());
        *config.test_config_value_mut() = serde_json::json!({
            "plugins": {
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "query": {
                                "type": "object",
                                "input_order": [],
                                "token": {"type": "string", "default": "fixed"}
                            },
                            "items": "{{ page.query.provider_should_not_run }}"
                        }
                    }
                }
            }
        });
        config.test_rebuild_parameter_registry().unwrap();
        config.rebuild_template_registry().unwrap();
        let page_state = config.instantiate_parameters("core:default").unwrap();
        let page_parameters = config
            .parameter_snapshot(&page_state, InputSourceIdentity::default())
            .unwrap();

        let projection = PickerItemsProjection::from_config(&config, "core:default").unwrap();
        let result = load_items_for_page(
            &projection,
            "core:default",
            &page_parameters,
            "needle",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();

        assert!(result.items.is_empty());
        assert!(result.contexts.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("input_order is empty"));
        assert!(!result.errors[0].contains("provider_should_not_run"));
    }

    #[test]
    fn feeds_merge_items_in_config_order() {
        let mut config = test_config();
        config.test_views_mut().insert(
            "sys:main".to_string(),
            View {
                engine: EngineSpec {
                    engine_type: ENGINE_PICKER.to_string(),
                    config: EngineOptions {
                        items: Some(toml::Value::Array(vec![toml::Value::Table(
                            [(
                                "label".to_string(),
                                toml::Value::String("SysItem".to_string()),
                            )]
                            .into_iter()
                            .collect(),
                        )])),
                        ..Default::default()
                    },
                },
                alias: Some("sys".to_string()),
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                keymap: None,
                commands: BTreeMap::new(),
            },
        );
        config
            .test_views_mut()
            .get_mut("core:default")
            .unwrap()
            .engine
            .config
            .feeds = vec![
            FeedSpec {
                view: "apps:main".to_string(),
            },
            FeedSpec {
                view: "sys:main".to_string(),
            },
        ];
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {"current": {"items": [{"label": "AppItem"}]}},
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            result
                .items
                .iter()
                .map(|item| (item.text.as_str(), item.source_view.as_str()))
                .collect::<Vec<_>>(),
            [("AppItem", "apps:main"), ("SysItem", "sys:main")]
        );
    }

    #[test]
    fn feed_object_defaults_apply_with_empty_input() {
        let root = env::temp_dir().join(format!(
            "tui-launcher-items-defaults-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("items.sh"),
            "payload=${1#query=}\njq -cn --arg label \"$(printf '%s' \"$payload\" | jq -r .text)\" '[{label:(\"VALUE:\" + $label)}]'\n",
        )
        .unwrap();

        let mut config = test_config();
        config
            .test_views_mut()
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(script_source(
            "items.sh",
            Some(toml::Value::Array(vec!["query={{ view.query }}".into()])),
        ));
        *config.test_config_value_mut() = serde_json::json!({
            "plugins": {
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "query": {
                                "type": "object",
                                "input_order": ["text"],
                                "text": {"type": "string", "default": "source-default"}
                            },
                            "items": {"source": "script", "file": "items.sh", "args": ["query={{ view.query }}"]}
                        }
                    }
                }
            }
        });
        config.test_rebuild_parameter_registry().unwrap();
        config.rebuild_template_registry().unwrap();
        config
            .test_plugin_roots_mut()
            .insert("apps".to_string(), root.clone());
        let page_state = config.instantiate_parameters("core:default").unwrap();
        let page_parameters = config
            .parameter_snapshot(&page_state, InputSourceIdentity::default())
            .unwrap();
        let projection = PickerItemsProjection::from_config(&config, "core:default").unwrap();
        let result = load_items_for_page(
            &projection,
            "core:default",
            &page_parameters,
            "",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(result.items[0].text, "VALUE:source-default");
        fs::remove_dir_all(root).unwrap();
    }
}
