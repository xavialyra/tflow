use crate::input::{InputSourceIdentity, ViewMountId};
use crate::lifecycle::CancellationToken;
use crate::terminal::sanitize_text;
use crate::workflow::config::{
    PickerItemsProjection, ProducerKind, parse_producer_script_handler, toml_to_json,
};
use crate::workflow::parameter::ParameterSnapshot;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

const MAX_ITEMS_PER_SESSION: usize = 100_000;

/// Immutable compiled metadata for one Picker view presentation.
#[derive(Clone)]
pub(crate) struct PickerItemsDefinition {
    pub(crate) view_ref: String,
    pub(crate) alias: Option<String>,
    items: Option<Value>,
    source: Arc<PickerItemsProjection>,
}

impl fmt::Debug for PickerItemsDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PickerItemsDefinition")
            .field("view_ref", &self.view_ref)
            .field("alias", &self.alias)
            .field("items", &self.items)
            .finish_non_exhaustive()
    }
}

impl PickerItemsDefinition {
    pub(crate) fn new(projection: Arc<PickerItemsProjection>, view_ref: &str) -> Result<Arc<Self>> {
        let (_, view) = projection.target_view();
        let items = view.items.as_ref().map(toml_to_json).transpose()?;
        Ok(Arc::new(Self {
            view_ref: view_ref.to_string(),
            alias: view.alias.clone(),
            items,
            source: projection,
        }))
    }

    pub(crate) fn has_items(&self) -> bool {
        self.items.is_some()
    }

    pub(crate) fn items_value(&self) -> Option<Value> {
        self.items.clone()
    }

    pub(crate) fn input_value(&self) -> &Value {
        self.source.input_value()
    }

    pub(crate) fn workflow_root(&self) -> Option<&Path> {
        self.source.workflow_root()
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
    #[serde(default)]
    bindings: BTreeMap<String, String>,
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
    pub(crate) bindings: BTreeMap<String, String>,
    pub(crate) source_view: String,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ItemsResult {
    pub(crate) items: Vec<Item>,
    pub(crate) errors: Vec<String>,
}

/// The complete identity of one Picker items request.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ItemsRequestIdentity {
    pub(super) mount_id: ViewMountId,
    pub(super) source: InputSourceIdentity,
    pub(super) generation: u64,
    pub(super) input_generation: u64,
    pub(super) input: String,
    pub(super) parameter_revision: u64,
    pub(super) binding_raw: String,
    pub(super) page_parameters: ParameterSnapshot,
}

impl ItemsRequestIdentity {
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
            "picker items request source does not belong to mount {:?}",
            self.mount_id
        );
        anyhow::ensure!(
            self.input_generation == self.source.generation,
            "picker items request input generation does not match its source"
        );
        anyhow::ensure!(
            self.generation > 0,
            "picker items request generation must be greater than zero"
        );
        anyhow::ensure!(
            self.page_parameters.source() == self.source,
            "picker items request source does not match page parameters"
        );
        anyhow::ensure!(
            self.page_parameters.revision() == self.parameter_revision,
            "picker items request parameter revision does not match page parameters"
        );
        Ok(())
    }

    pub(super) fn matches_response(&self, view: &str, identity: &ItemsRequestIdentity) -> bool {
        !view.is_empty() && self == identity
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ItemsRequest {
    pub(crate) view: String,
    pub(crate) identity: ItemsRequestIdentity,
    pub(crate) engine_state: Value,
}

impl ItemsRequest {
    pub(super) fn new(view: String, identity: ItemsRequestIdentity) -> Result<Self> {
        let request = Self {
            view,
            identity,
            engine_state: Value::Null,
        };
        request.validate()?;
        Ok(request)
    }

    pub(super) fn with_engine_state(mut self, engine_state: Value) -> Result<Self> {
        self.engine_state = engine_state;
        self.validate()?;
        Ok(self)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.view.is_empty(), "picker items request view is empty");
        self.identity.validate()
    }

    pub(super) fn matches_context(
        &self,
        frame: ViewMountId,
        view: &str,
        input: &str,
        binding_raw: &str,
        page_parameters: &ParameterSnapshot,
    ) -> bool {
        self.view == view
            && self.identity.mount_id == frame
            && self.identity.parameter_revision == page_parameters.revision()
            && self.identity.input == input
            && self.identity.binding_raw == binding_raw
            && &self.identity.page_parameters == page_parameters
    }

    pub(super) fn matches_response(&self, view: &str, identity: &ItemsRequestIdentity) -> bool {
        self.identity.matches_response(view, identity)
    }
}

#[derive(Debug, Clone)]
pub(super) struct ItemsResponse {
    pub(super) view: String,
    pub(super) identity: ItemsRequestIdentity,
    pub(super) result: std::result::Result<ItemsResult, String>,
}

#[derive(Debug)]
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

pub(crate) fn load_items_for_definition_with_outcome(
    definition: &Arc<PickerItemsDefinition>,
    page_parameters: &ParameterSnapshot,
    _binding_raw: &str,
    engine_state: &Value,
    cancellation: &CancellationToken,
) -> ItemsLoadOutcome {
    if cancellation.is_cancelled() || !definition.has_items() {
        return ItemsLoadOutcome {
            result: Ok(ItemsResult::default()),
            managed_child_reaped: false,
        };
    }
    let Some(items_val) = definition.items_value() else {
        return ItemsLoadOutcome {
            result: Ok(ItemsResult::default()),
            managed_child_reaped: false,
        };
    };

    let mut result = ItemsResult::default();
    let (value, managed_child_reaped) = if is_producer_value(&items_val) {
        let outcome = run_items_provider(
            definition,
            page_parameters,
            &items_val,
            engine_state,
            cancellation,
        );
        (outcome.result, outcome.managed_child_reaped)
    } else {
        (Ok(items_val), false)
    };

    match value {
        Ok(val) => {
            append_items_value(&mut result, &definition.view_ref, val, cancellation);
            ItemsLoadOutcome {
                result: Ok(result),
                managed_child_reaped,
            }
        }
        Err(err) => {
            result
                .errors
                .push(format!("{}: {}", definition.view_ref, err));
            ItemsLoadOutcome {
                result: Ok(result),
                managed_child_reaped,
            }
        }
    }
}

fn append_items_value(
    result: &mut ItemsResult,
    source_ref: &str,
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
    append_items_array(result, source_ref, value, cancellation);
}

fn append_items_array(
    result: &mut ItemsResult,
    source_ref: &str,
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
            bindings: parsed.bindings,
            source_view: source_ref.to_string(),
        });
    }
    result.items.extend(parsed_items);
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
    definition: &PickerItemsDefinition,
    page_parameters: &ParameterSnapshot,
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
    let source_label = format!("[views.{}.items]", definition.view_ref);
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
            let source = match parse_producer_script_handler(&handler, definition.workflow_root()) {
                Ok(source) => source,
                Err(error) => {
                    return ItemsScriptOutcome {
                        result: Err(error),
                        managed_child_reaped: false,
                    };
                }
            };
            let request = crate::protocol::items_request(
                page_parameters.values(),
                definition.input_value(),
                "picker",
                engine_state,
            );
            let outcome = crate::protocol::run_script_items_response(
                &definition.view_ref,
                &source_label,
                definition.workflow_root(),
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

pub(crate) fn run_items_producer_raw(
    view_ref: &str,
    producer_value: &toml::Value,
    script_root: Option<&Path>,
    parameters: &Value,
    raw_input: &str,
    cancellation: &CancellationToken,
) -> Result<Value> {
    let json_val = toml_to_json(producer_value)?;
    let source_label = format!("[views.{}.items]", view_ref);
    if json_val.is_array() {
        return validate_items_value(&source_label, json_val);
    }
    let provider: ItemsProducer = serde_json::from_value(json_val)
        .context("items producer must define producer and handler")?;
    match provider.producer {
        ProducerKind::Declared => {
            let handler: DeclaredItemsHandler = serde_json::from_value(provider.handler)
                .context("declared items handler must define an items array")?;
            validate_items_value(&source_label, handler.items)
        }
        ProducerKind::Script => {
            let handler = toml::Value::try_from(provider.handler)
                .context("items script handler could not be converted to TOML")?;
            let source = parse_producer_script_handler(&handler, script_root)?;
            let engine_state = serde_json::json!({
                "input": raw_input,
            });
            let request =
                crate::protocol::items_request(parameters, &Value::Null, "picker", &engine_state);
            let outcome = crate::protocol::run_script_items_response(
                view_ref,
                &source_label,
                script_root,
                &source,
                &request,
                cancellation,
            );
            outcome
                .result
                .and_then(|value| validate_items_value(&source_label, value))
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

    fn cancellation() -> CancellationToken {
        CancellationToken::new()
    }

    #[test]
    fn parses_structured_items_and_preserves_metadata() {
        let mut result = ItemsResult::default();
        append_items_array(
            &mut result,
            "core:items",
            serde_json::json!([{
                "display": "Example item",
                "value": "example-value",
                "metadata": {"text": "Example metadata"},
                "bindings": {"enter": "apps:open"}
            }]),
            &cancellation(),
        );
        assert!(result.errors.is_empty());
        assert_eq!(result.items[0].text, "Example item");
        assert_eq!(result.items[0].value.as_deref(), Some("example-value"));
        assert_eq!(result.items[0].metadata["text"], "Example metadata");
        assert_eq!(result.items[0].bindings["enter"], "apps:open");
        assert_eq!(result.items[0].source_view, "core:items");
    }

    #[test]
    fn rejects_malformed_item_shapes_before_aggregation() {
        let mut result = ItemsResult::default();
        append_items_array(
            &mut result,
            "core:items",
            serde_json::json!([{"value": "missing display"}]),
            &cancellation(),
        );
        assert!(result.items.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("index 0"));

        let mut later_invalid = ItemsResult::default();
        append_items_array(
            &mut later_invalid,
            "core:items",
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
        append_items_array(
            &mut result,
            "core:items",
            serde_json::json!([{"display": ""}]),
            &cancellation(),
        );
        assert!(result.items.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("index 0"));

        let mut allowed = ItemsResult::default();
        append_items_array(
            &mut allowed,
            "core:items",
            serde_json::json!([{"display": "", "allow_empty": true}]),
            &cancellation(),
        );
        assert!(allowed.errors.is_empty());
        assert_eq!(allowed.items.len(), 1);
    }

    #[test]
    fn item_session_limit_is_enforced() {
        let mut result = ItemsResult::default();
        let oversized = Value::Array(
            (0..MAX_ITEMS_PER_SESSION + 1)
                .map(|_| serde_json::json!({"display": "item"}))
                .collect(),
        );
        append_items_array(&mut result, "core:items", oversized, &cancellation());
        assert!(result.items.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("session limit"));
    }
}
