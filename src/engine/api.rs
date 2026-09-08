use crate::lifecycle::CancellationObserver;
use crate::workflow::config::View;
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
pub(crate) struct ProjectedEngineConfig {
    pub(crate) fields: BTreeMap<String, Value>,
    pub(crate) workflow_root: Option<PathBuf>,
    pub(crate) launch_input: Value,
}

impl ProjectedEngineConfig {
    pub(crate) fn field(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectedBindingConfig {
    pub(crate) defaults: Option<Value>,
    pub(crate) view_keymap: Option<Value>,
    pub(crate) engine_fields: BTreeMap<String, Value>,
}

impl ProjectedBindingConfig {
    pub(crate) fn engine_field(&self, name: &str) -> Option<&Value> {
        self.engine_fields.get(name)
    }
}

pub(crate) struct RuntimeFactoryContext {
    pub(crate) identity: ViewIdentity,
    pub(crate) config: ProjectedEngineConfig,
    pub(crate) parameters: crate::workflow::parameter::ParameterSnapshot,
    pub(crate) cancellation: CancellationObserver,
}

pub(crate) struct RendererFactoryContext;

pub(crate) struct InputBindingFactoryContext {
    pub(crate) identity: ViewIdentity,
    pub(crate) bindings: ProjectedBindingConfig,
}

pub(crate) struct EngineValidationContext<'a> {
    pub(crate) view_ref: &'a str,
    pub(crate) view: &'a View,
    pub(crate) script_root: Option<&'a Path>,
}
