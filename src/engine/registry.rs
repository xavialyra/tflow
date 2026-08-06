use super::api::{Engine, EngineDriver};
use super::launcher::CommandExecution;
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
        registry.register(Box::new(super::launcher::LauncherCommandEngine));
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
        view_ref: &str,
        input: &str,
        log_file: Option<&Path>,
    ) -> Result<Box<dyn EngineDriver>> {
        let engine_type = config.engine(view_ref)?;
        let engine = self
            .engines
            .get(engine_type)
            .with_context(|| format!("unsupported view engine {:?}", engine_type))?;
        engine.create_view(config, view_ref, input, log_file)
    }

    pub(crate) fn create_command(
        &self,
        execution: CommandExecution,
    ) -> Result<Box<dyn EngineDriver>> {
        let engine = self
            .engines
            .get(execution.engine_type.as_str())
            .with_context(|| {
                format!(
                    "unsupported command target engine {:?}",
                    execution.engine_type
                )
            })?;
        engine.create_command(execution)
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
