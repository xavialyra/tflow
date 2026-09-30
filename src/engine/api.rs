use crate::lifecycle::CancellationObserver;
use crate::workflow::config::View;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddedResultFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EmbeddedResultConfig {
    pub(crate) format: EmbeddedResultFormat,
    pub(crate) required: bool,
    pub(crate) max_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewIdentity {
    pub(crate) view_ref: String,
    pub(crate) engine_type: String,
}

impl ViewIdentity {
    pub(crate) fn new(view_ref: impl Into<String>, engine_type: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            engine_type: engine_type.into(),
        }
    }
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub(crate) struct PickerConfig {
    #[serde(default)]
    pub(crate) items: Option<toml::Value>,
    #[serde(default)]
    pub(crate) preview: Option<toml::Value>,
    #[serde(default)]
    pub(crate) input_placeholder: Option<String>,
    #[serde(default)]
    pub(crate) show_input: Option<bool>,
    #[serde(default)]
    pub(crate) show_divider: Option<bool>,
    #[serde(default)]
    pub(crate) show_left_prefix: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub(crate) struct CaptureConfig {
    pub(crate) output: toml::Value,
}

impl CaptureConfig {
    pub(crate) fn static_text(&self) -> Option<&str> {
        if let Some(s) = self.output.as_str() {
            return Some(s);
        }
        if let Some(t) = self.output.as_table() {
            if let Some(s) = t.get("content").and_then(|v| v.as_str()) {
                return Some(s);
            }
            if let Some(s) = t.get("output").and_then(|v| v.as_str()) {
                return Some(s);
            }
        }
        None
    }

    pub(crate) fn is_async(&self) -> bool {
        if let Some(t) = self.output.as_table() {
            t.contains_key("file") || t.contains_key("script")
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub(crate) struct FormConfig {
    pub(crate) content: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub(crate) struct EmbeddedConfig {
    #[serde(default)]
    pub(crate) command: Vec<String>,
    #[serde(default)]
    pub(crate) result: Option<toml::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TypedEngineConfig {
    Picker(PickerConfig),
    Capture(CaptureConfig),
    Form(FormConfig),
    Embedded(EmbeddedConfig),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectedEngineConfig {
    pub(crate) typed: Option<TypedEngineConfig>,
    pub(crate) workflow_root: Option<PathBuf>,
    pub(crate) launch_input: Value,
}

impl ProjectedEngineConfig {
    #[cfg(test)]
    pub(crate) fn from_typed(typed: TypedEngineConfig) -> Self {
        Self {
            typed: Some(typed),
            workflow_root: None,
            launch_input: Value::Null,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_picker(config: PickerConfig) -> Self {
        Self::from_typed(TypedEngineConfig::Picker(config))
    }

    #[cfg(test)]
    pub(crate) fn for_capture(config: CaptureConfig) -> Self {
        Self::from_typed(TypedEngineConfig::Capture(config))
    }

    #[cfg(test)]
    pub(crate) fn for_form(config: FormConfig) -> Self {
        Self::from_typed(TypedEngineConfig::Form(config))
    }

    #[cfg(test)]
    pub(crate) fn for_embedded(config: EmbeddedConfig) -> Self {
        Self::from_typed(TypedEngineConfig::Embedded(config))
    }

    pub(crate) fn as_picker(&self) -> Option<&PickerConfig> {
        match self.typed.as_ref() {
            Some(TypedEngineConfig::Picker(c)) => Some(c),
            _ => None,
        }
    }

    pub(crate) fn as_capture(&self) -> Option<&CaptureConfig> {
        match self.typed.as_ref() {
            Some(TypedEngineConfig::Capture(c)) => Some(c),
            _ => None,
        }
    }

    pub(crate) fn as_form(&self) -> Option<&FormConfig> {
        match self.typed.as_ref() {
            Some(TypedEngineConfig::Form(c)) => Some(c),
            _ => None,
        }
    }

    pub(crate) fn as_embedded(&self) -> Option<&EmbeddedConfig> {
        match self.typed.as_ref() {
            Some(TypedEngineConfig::Embedded(c)) => Some(c),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectedBindingConfig {
    pub(crate) defaults: Option<Value>,
}

pub(crate) struct RuntimeFactoryContext {
    pub(crate) identity: ViewIdentity,
    pub(crate) config: ProjectedEngineConfig,
    pub(crate) parameters: crate::workflow::parameter::ParameterSnapshot,
    pub(crate) cancellation: CancellationObserver,
}

pub(crate) struct RendererFactoryContext;

pub(crate) struct EngineValidationContext<'a> {
    pub(crate) view_ref: &'a str,
    pub(crate) view: &'a View,
    pub(crate) script_root: Option<&'a Path>,
}
