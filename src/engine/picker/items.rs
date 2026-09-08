use crate::input::{InputSourceIdentity, ViewMountId};
use crate::lifecycle::CancellationToken;
use crate::terminal::sanitize_text;
#[cfg(test)]
use crate::workflow::config::CompiledConfig;
use crate::workflow::config::{
    PickerItemsProjection, ProducerKind, parse_producer_script_handler, toml_to_json,
};
use crate::workflow::parameter::{ParameterBinding, ParameterSnapshot};
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

    fn items_value(&self) -> Option<Value> {
        self.items.clone()
    }

    fn input_value(&self) -> &Value {
        self.source.input_value()
    }

    fn workflow_root(&self) -> Option<&Path> {
        self.source.workflow_root(&self.owner_view)
    }
}

/// Parameters resolved for one request and one immutable feed definition.
#[derive(Debug, Clone)]
pub(crate) struct FeedInstance {
    pub(crate) definition: Arc<FeedDefinition>,
    pub(crate) parameters: ParameterSnapshot,
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
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemValue {
    display: super::display::ItemDisplayInput,
    #[serde(default)]
    allow_empty: bool,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    metadata: Value,
}

pub(super) fn validate_item_array(value: &Value) -> Result<()> {
    let Value::Array(items) = value else {
        bail!("items must be an array");
    };
    if items.len() > MAX_ITEMS_PER_SESSION {
        bail!(
            "items exceeded the session limit of {}",
            MAX_ITEMS_PER_SESSION
        );
    }
    for (index, value) in items.iter().enumerate() {
        let item: ItemValue = serde_json::from_value(value.clone())
            .with_context(|| format!("invalid items JSON at index {}", index))?;
        let display: super::display::NormalizedItemDisplay = item.display.into();
        if sanitize_text(&display.plain_text()).is_empty() && !item.allow_empty {
            bail!("items JSON at index {} has an empty display", index);
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub(crate) text: String,
    pub(crate) display: super::display::NormalizedItemDisplay,
    pub(crate) value: Option<String>,
    pub(crate) metadata: Value,
    /// Stable provenance used internally for feed routing and preview lookup.
    pub(crate) source_view: String,
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
    pub(super) engine_state: Value,
}

impl ItemsRequest {
    pub(super) fn new(view: String, identity: FeedRequestIdentity) -> Result<Self> {
        anyhow::ensure!(!view.is_empty(), "picker feed request view is empty");
        identity.validate()?;
        Ok(Self {
            view,
            identity,
            engine_state: Value::Null,
        })
    }

    pub(super) fn with_engine_state(mut self, engine_state: Value) -> Self {
        self.engine_state = engine_state;
        self
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

pub(crate) struct ItemsLoadOutcome {
    pub(crate) result: Result<ItemsResult>,
    pub(crate) managed_child_reaped: bool,
}

impl ItemsLoadOutcome {
    #[cfg(test)]
    pub(crate) fn without_managed_child(result: Result<ItemsResult>) -> Self {
        Self {
            result,
            managed_child_reaped: false,
        }
    }
}

pub(crate) trait PickerItemsLoader: Send + Sync {
    fn load(&self, request: &ItemsRequest, cancellation: &CancellationToken) -> ItemsLoadOutcome;
}

struct FeedLoadOutput {
    feed_id: FeedId,
    owner_view: String,
    context: Option<(FeedId, FeedInstance)>,
    value: std::result::Result<Option<Value>, String>,
    managed_child_reaped: bool,
}

fn load_single_feed(
    definition: &Arc<FeedDefinition>,
    page_view: &str,
    page_parameters: &ParameterSnapshot,
    binding_raw: &str,
    engine_state: &Value,
    cancellation: &CancellationToken,
) -> Option<FeedLoadOutput> {
    if cancellation.is_cancelled() || !definition.has_items() {
        return None;
    }
    let feed_id = definition.feed_id.clone();
    let owner_view = definition.owner_view.clone();
    let instance = match FeedInstance::resolve(
        Arc::clone(definition),
        page_view,
        page_parameters,
        binding_raw,
    ) {
        Ok(instance) => instance,
        Err(error) => {
            return Some(FeedLoadOutput {
                feed_id,
                owner_view: owner_view.clone(),
                context: None,
                value: Err(format!("{}: {}", owner_view, error)),
                managed_child_reaped: false,
            });
        }
    };

    let context = Some((feed_id.clone(), instance.clone()));
    let (value, managed_child_reaped) = match instance.definition.items_value() {
        Some(value) if is_producer_value(&value) => {
            let outcome = run_items_provider(&instance, &value, engine_state, cancellation);
            (outcome.result.map(Some), outcome.managed_child_reaped)
        }
        Some(value) => (Ok(Some(value)), false),
        None => (Ok(None), false),
    };
    let value = value.map_err(|error| format!("{}: {}", owner_view, error));

    Some(FeedLoadOutput {
        feed_id,
        owner_view,
        context,
        value,
        managed_child_reaped,
    })
}

#[cfg(test)]
pub(crate) fn load_items_for_definitions(
    definitions: &[Arc<FeedDefinition>],
    page_view: &str,
    page_parameters: &ParameterSnapshot,
    binding_raw: &str,
    cancellation: &CancellationToken,
) -> Result<ItemsResult> {
    load_items_for_definitions_with_outcome(
        definitions,
        page_view,
        page_parameters,
        binding_raw,
        &Value::Null,
        cancellation,
    )
    .result
}

pub(crate) fn load_items_for_definitions_with_outcome(
    definitions: &[Arc<FeedDefinition>],
    page_view: &str,
    page_parameters: &ParameterSnapshot,
    binding_raw: &str,
    engine_state: &Value,
    cancellation: &CancellationToken,
) -> ItemsLoadOutcome {
    let mut result = ItemsResult::default();
    let outputs: Vec<Option<FeedLoadOutput>> = if definitions.len() <= 1 {
        definitions
            .iter()
            .map(|definition| {
                load_single_feed(
                    definition,
                    page_view,
                    page_parameters,
                    binding_raw,
                    engine_state,
                    cancellation,
                )
            })
            .collect()
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = definitions
                .iter()
                .map(|definition| {
                    s.spawn(|| {
                        load_single_feed(
                            definition,
                            page_view,
                            page_parameters,
                            binding_raw,
                            engine_state,
                            cancellation,
                        )
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        })
    };

    let managed_child_reaped = outputs
        .iter()
        .flatten()
        .any(|output| output.managed_child_reaped);
    for output in outputs.into_iter().flatten() {
        if cancellation.is_cancelled() {
            return ItemsLoadOutcome {
                result: Ok(result),
                managed_child_reaped,
            };
        }
        if let Some((feed_id, instance)) = output.context {
            result.contexts.insert(feed_id, instance);
        }
        match output.value {
            Ok(Some(value)) => {
                if result.items.len() >= MAX_ITEMS_PER_SESSION {
                    result.errors.push(format!(
                        "items exceeded the session limit of {}",
                        MAX_ITEMS_PER_SESSION
                    ));
                    break;
                }
                append_items_value(
                    &mut result,
                    &output.owner_view,
                    &output.feed_id,
                    value,
                    cancellation,
                );
            }
            Ok(None) => {}
            Err(error) => {
                result.errors.push(error);
            }
        }
        if cancellation.is_cancelled() {
            return ItemsLoadOutcome {
                result: Ok(result),
                managed_child_reaped,
            };
        }
    }

    ItemsLoadOutcome {
        result: Ok(result),
        managed_child_reaped,
    }
}

#[cfg(test)]
pub(crate) fn load_items_for_page(
    projection: &PickerItemsProjection,
    page_view: &str,
    page_parameters: &ParameterSnapshot,
    binding_raw: &str,
    cancellation: &CancellationToken,
) -> Result<ItemsResult> {
    let definitions = FeedDefinition::collection(Arc::new(projection.clone()), page_view)?;
    load_items_for_definitions(
        &definitions,
        page_view,
        page_parameters,
        binding_raw,
        cancellation,
    )
}

#[cfg(test)]
fn load_items(
    config: &CompiledConfig,
    view_ref: &str,
    cancellation: &CancellationToken,
) -> Result<ItemsResult> {
    let page_state = config.instantiate_parameters(view_ref)?;
    let page_parameters = config.parameter_snapshot(&page_state, InputSourceIdentity::default())?;
    let projection = PickerItemsProjection::from_config(config, &Value::Null, view_ref)?;
    load_items_for_page(&projection, view_ref, &page_parameters, "", cancellation)
}

fn append_items_value(
    result: &mut ItemsResult,
    source_ref: &str,
    feed_id: &FeedId,
    value: Value,
    cancellation: &CancellationToken,
) {
    if !value.is_array() {
        result.errors.push(format!(
            "{}: items must be an array, got {}",
            source_ref,
            value_type(&value)
        ));
        return;
    }
    append_items_array(result, source_ref, feed_id, value, cancellation);
}

fn append_items_array(
    result: &mut ItemsResult,
    source_ref: &str,
    _feed_id: &FeedId,
    value: Value,
    cancellation: &CancellationToken,
) {
    let Value::Array(items) = value else {
        result.errors.push(format!(
            "{}: items response must be a JSON array",
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
        let display: super::display::NormalizedItemDisplay = parsed.display.into();
        let text = sanitize_text(&display.plain_text());
        if text.is_empty() && !parsed.allow_empty {
            result.errors.push(format!(
                "{}: items JSON at index {} has an empty display",
                source_ref, index
            ));
            return;
        }
        parsed_items.push(Item {
            text,
            display,
            value: parsed.value,
            metadata: parsed.metadata,
            source_view: source_ref.to_string(),
        });
    }
    result.items.extend(parsed_items);
}

#[cfg(test)]
fn append_items(
    result: &mut ItemsResult,
    source_ref: &str,
    feed_id: &FeedId,
    value: Value,
    cancellation: &CancellationToken,
) {
    append_items_array(result, source_ref, feed_id, value, cancellation);
}

struct ItemsScriptOutcome {
    result: Result<Value>,
    managed_child_reaped: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemsProducer {
    producer: ProducerKind,
    handler: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredItemsHandler {
    items: Value,
}

fn is_producer_value(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|fields| fields.contains_key("producer"))
}

fn run_items_provider(
    instance: &FeedInstance,
    value: &Value,
    engine_state: &Value,
    cancellation: &CancellationToken,
) -> ItemsScriptOutcome {
    let provider: Result<ItemsProducer> = serde_json::from_value(value.clone())
        .context("items producer must define producer and handler");
    let provider = match provider {
        Ok(provider) => provider,
        Err(error) => {
            return ItemsScriptOutcome {
                result: Err(error),
                managed_child_reaped: false,
            };
        }
    };
    let source_label = format!("[views.{}.items]", instance.definition.owner_view);
    match provider.producer {
        ProducerKind::Declared => {
            let handler: Result<DeclaredItemsHandler> = serde_json::from_value(provider.handler)
                .context("declared items handler must define an items array");
            match handler {
                Ok(handler) if handler.items.is_array() => ItemsScriptOutcome {
                    result: validate_items_value(&source_label, handler.items),
                    managed_child_reaped: false,
                },
                Ok(_) => ItemsScriptOutcome {
                    result: Err(anyhow::anyhow!(
                        "{} declared items handler must define an items array",
                        source_label
                    )),
                    managed_child_reaped: false,
                },
                Err(error) => ItemsScriptOutcome {
                    result: Err(error),
                    managed_child_reaped: false,
                },
            }
        }
        ProducerKind::Script => {
            let handler = match toml::Value::try_from(provider.handler)
                .context("items script handler could not be converted to TOML")
            {
                Ok(handler) => handler,
                Err(error) => {
                    return ItemsScriptOutcome {
                        result: Err(error),
                        managed_child_reaped: false,
                    };
                }
            };
            let source = match parse_producer_script_handler(
                &handler,
                instance.definition.workflow_root(),
            ) {
                Ok(source) => source,
                Err(error) => {
                    return ItemsScriptOutcome {
                        result: Err(error),
                        managed_child_reaped: false,
                    };
                }
            };
            let request = crate::protocol::items_request(
                instance.parameters.values(),
                instance.definition.input_value(),
                "picker",
                engine_state,
            );
            let outcome = crate::protocol::run_script_items_response(
                &instance.definition.owner_view,
                &source_label,
                instance.definition.workflow_root(),
                &source,
                &request,
                cancellation,
            );
            ItemsScriptOutcome {
                result: outcome
                    .result
                    .and_then(|value| validate_items_value(&source_label, value)),
                managed_child_reaped: outcome.managed_child_reaped,
            }
        }
    }
}

fn validate_items_value(source_label: &str, value: Value) -> Result<Value> {
    validate_item_array(&value)
        .with_context(|| format!("{} producer returned invalid items", source_label))?;
    Ok(value)
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
    use crate::input::InputSourceIdentity;

    fn cancellation() -> CancellationToken {
        CancellationToken::new()
    }

    #[test]
    fn parses_structured_items_and_preserves_literal_templates() {
        let mut result = ItemsResult::default();
        append_items(
            &mut result,
            "core:items",
            &FeedId("core:items".to_string()),
            serde_json::json!([{
                "display": "literal {{ page.input }}",
                "value": "{{ selection.value }}",
                "metadata": {"text": "{{ current.value }}"}
            }]),
            &cancellation(),
        );
        assert!(result.errors.is_empty());
        assert_eq!(result.items[0].text, "literal {{ page.input }}");
        assert_eq!(
            result.items[0].value.as_deref(),
            Some("{{ selection.value }}")
        );
        assert_eq!(result.items[0].metadata["text"], "{{ current.value }}");
        assert_eq!(result.items[0].source_view, "core:items");
    }

    #[test]
    fn rejects_malformed_item_shapes_before_aggregation() {
        let mut result = ItemsResult::default();
        append_items(
            &mut result,
            "core:items",
            &FeedId("core:items".to_string()),
            serde_json::json!([{"value": "missing display"}]),
            &cancellation(),
        );
        assert!(result.items.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("index 0"));

        let mut later_invalid = ItemsResult::default();
        append_items(
            &mut later_invalid,
            "core:items",
            &FeedId("core:items".to_string()),
            serde_json::json!([
                {"display": "valid"},
                {"value": "missing display"}
            ]),
            &cancellation(),
        );
        assert!(later_invalid.items.is_empty());
        assert_eq!(later_invalid.errors.len(), 1);
        assert!(later_invalid.errors[0].contains("index 1"));
    }

    #[test]
    fn empty_display_requires_allow_empty() {
        let mut result = ItemsResult::default();
        append_items(
            &mut result,
            "core:items",
            &FeedId("core:items".to_string()),
            serde_json::json!([{"display": ""}]),
            &cancellation(),
        );
        assert!(result.items.is_empty());
        assert!(result.errors[0].contains("empty display"));

        let mut allowed = ItemsResult::default();
        append_items(
            &mut allowed,
            "core:items",
            &FeedId("core:items".to_string()),
            serde_json::json!([{"display": "", "allow_empty": true}]),
            &cancellation(),
        );
        assert!(allowed.errors.is_empty());
        assert_eq!(allowed.items.len(), 1);
    }

    #[test]
    fn request_identity_requires_matching_mount_source_and_revision() {
        let source = InputSourceIdentity {
            frame: ViewMountId(7),
            generation: 3,
        };
        let parameters = ParameterSnapshot::from_parts(
            serde_json::json!({"query": "value"}),
            "value".to_string(),
            source,
            4,
        );
        let identity = FeedRequestIdentity::new(
            ViewMountId(7),
            source,
            1,
            "value".to_string(),
            4,
            "value".to_string(),
            parameters.clone(),
        )
        .unwrap();
        let request = ItemsRequest::new("core:items".to_string(), identity.clone()).unwrap();
        assert!(request.matches_context(
            ViewMountId(7),
            "core:items",
            "value",
            "value",
            &parameters
        ));
        assert!(request.matches_response("core:items", &identity));
        assert!(!request.matches_context(
            ViewMountId(8),
            "core:items",
            "value",
            "value",
            &parameters
        ));
    }

    #[test]
    fn item_session_limit_is_enforced() {
        let mut result = ItemsResult::default();
        let values = (0..=MAX_ITEMS_PER_SESSION)
            .map(|index| serde_json::json!({"display": format!("item-{index}")}))
            .collect();
        append_items(
            &mut result,
            "core:items",
            &FeedId("core:items".to_string()),
            Value::Array(values),
            &cancellation(),
        );
        assert!(result.items.is_empty());
        assert!(result.errors[0].contains("session limit"));
    }

    #[test]
    fn fixture_items_are_loaded_through_static_projection() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let result = load_items(&config, "core:default", &cancellation()).unwrap();
        assert!(!result.items.is_empty());
        assert!(result.items.iter().all(|item| !item.source_view.is_empty()));
    }
}
