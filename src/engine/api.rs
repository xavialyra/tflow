use crate::config::View;
use crate::lifecycle::CancellationObserver;
use serde_json::Value;
use std::collections::BTreeMap;
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

#[derive(Debug, Clone, Default)]
pub(crate) struct EvaluatedEngineConfig {
    pub(crate) fields: BTreeMap<String, Value>,
    pub(crate) field_errors: BTreeMap<String, String>,
    pub(crate) plugin_root: Option<PathBuf>,
}

impl EvaluatedEngineConfig {
    pub(crate) fn field(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    pub(crate) fn field_error(&self, name: &str) -> Option<&str> {
        self.field_errors.get(name).map(String::as_str)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EvaluatedBindingConfig {
    pub(crate) defaults: Option<Value>,
    pub(crate) view_keymap: Option<Value>,
    pub(crate) engine_fields: BTreeMap<String, Value>,
}

impl EvaluatedBindingConfig {
    pub(crate) fn engine_field(&self, name: &str) -> Option<&Value> {
        self.engine_fields.get(name)
    }
}

pub(crate) struct RuntimeFactoryContext {
    pub(crate) identity: ViewIdentity,
    pub(crate) config: EvaluatedEngineConfig,
    pub(crate) parameters: crate::parameter::ParameterSnapshot,
    pub(crate) cancellation: CancellationObserver,
}

pub(crate) struct RendererFactoryContext;

pub(crate) struct InputBindingFactoryContext {
    pub(crate) identity: ViewIdentity,
    pub(crate) bindings: EvaluatedBindingConfig,
}

pub(crate) struct EngineValidationContext<'a> {
    pub(crate) view_ref: &'a str,
    pub(crate) view: &'a View,
    pub(crate) script_root: Option<&'a Path>,
}
