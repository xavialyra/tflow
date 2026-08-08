use super::api::{Engine, ViewContext, ViewInstance, ViewLocation};
use super::runtime::RuntimeHandle;
use crate::config::{Config, EngineDefinition};
use crate::expression::validate_json_value;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) struct EngineRegistry {
    engines: BTreeMap<&'static str, Box<dyn Engine>>,
}

impl EngineRegistry {
    pub(crate) fn new() -> Self {
        let mut registry = Self {
            engines: BTreeMap::new(),
        };
        registry.register(Box::new(super::launcher::LauncherEngine));
        registry.register(Box::new(super::capture::CaptureEngine));
        registry.register(Box::new(super::embedded::EmbeddedEngine));
        registry
    }

    pub(crate) fn register(&mut self, engine: Box<dyn Engine>) {
        self.engines.insert(engine.engine_type(), engine);
    }

    pub(crate) fn contains(&self, engine_type: &str) -> bool {
        self.engines.contains_key(engine_type)
    }

    pub(crate) fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()> {
        let engine = self
            .engines
            .get(definition.engine_type.as_str())
            .with_context(|| format!("unsupported viewtype engine {:?}", definition.engine_type))?;
        engine.validate_config(name, definition)
    }

    pub(crate) fn create_view(
        &self,
        config: &Config,
        location: &ViewLocation,
        log_file: Option<&Path>,
        runtime: RuntimeHandle,
        tasks: super::TaskScheduler,
    ) -> Result<Box<dyn ViewInstance>> {
        let engine_type = config.engine(&location.view_ref)?;
        let engine = self
            .engines
            .get(engine_type)
            .with_context(|| format!("unsupported view engine {:?}", engine_type))?;
        engine.create_view(ViewContext {
            config,
            location,
            log_file,
            runtime,
            tasks,
        })
    }
}

pub(crate) fn validate_fields(
    name: &str,
    definition: &EngineDefinition,
    allowed: &[&str],
) -> Result<()> {
    for (field, value) in &definition.config {
        if !allowed.contains(&field.as_str()) {
            bail!(
                "viewtype {:?} engine {:?} has unsupported config field {:?}",
                name,
                definition.engine_type,
                field
            );
        }
        let value = serde_json::to_value(value).context("engine config is not valid JSON")?;
        validate_json_value(&value)
            .with_context(|| format!("viewtype {:?} engine field {:?}", name, field))?;
    }
    Ok(())
}

pub(crate) fn require_field(name: &str, definition: &EngineDefinition, field: &str) -> Result<()> {
    if !definition.config.contains_key(field) {
        bail!(
            "viewtype {:?} engine {:?} requires config field {:?}",
            name,
            definition.engine_type,
            field
        );
    }
    Ok(())
}
