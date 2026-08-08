use super::api::{Engine, ViewContext, ViewInstance, ViewLocation};
use super::runtime::RuntimeHandle;
use crate::config::{Config, View};
use crate::expression::validate_json_value;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

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

    #[cfg(test)]
    pub(crate) fn contains(&self, engine_type: &str) -> bool {
        self.engines.contains_key(engine_type)
    }

    pub(crate) fn validate_config(&self, name: &str, view: &View) -> Result<()> {
        let engine = self
            .engines
            .get(view.engine_type.as_str())
            .with_context(|| {
                format!(
                    "view {:?} uses unsupported engine {:?}",
                    name, view.engine_type
                )
            })?;
        engine.validate_config(name, view)
    }

    pub(crate) fn create_view(
        &self,
        config: &Config,
        location: &ViewLocation,
        log_file: Option<&Path>,
        runtime: RuntimeHandle,
        tasks: super::TaskScheduler,
        router: Arc<crate::router::Router>,
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
            router,
        })
    }
}

pub(crate) fn validate_fields(name: &str, view: &View, allowed: &[&str]) -> Result<()> {
    for (field, value) in &view.engine_config {
        if !allowed.contains(&field.as_str()) {
            bail!(
                "view {:?} using engine {:?} has unsupported field {:?}",
                name,
                view.engine_type,
                field
            );
        }
        let value = serde_json::to_value(value).context("view field is not valid JSON")?;
        validate_json_value(&value)
            .with_context(|| format!("view {:?} field {:?}", name, field))?;
    }
    Ok(())
}

pub(crate) fn require_field(name: &str, view: &View, field: &str) -> Result<()> {
    if view.engine_field(field).is_none() {
        bail!(
            "view {:?} using engine {:?} requires field {:?}",
            name,
            view.engine_type,
            field
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineHost, ViewContext, ViewEffect, ViewInstance};
    use crate::terminal::Terminal;
    use anyhow::Result;

    #[test]
    fn accepts_custom_engine_implementations() {
        struct TestEngine;

        struct TestView;

        impl ViewInstance for TestView {
            fn step(
                &mut self,
                _host: &mut EngineHost<'_>,
                _terminal: &mut Terminal,
            ) -> Result<ViewEffect> {
                Ok(ViewEffect::Continue)
            }

            fn render(
                &mut self,
                _host: &EngineHost<'_>,
                _terminal: &Terminal,
                _chrome: &crate::chrome::ChromeFrame,
            ) -> Result<()> {
                Ok(())
            }
        }

        impl Engine for TestEngine {
            fn engine_type(&self) -> &'static str {
                "test"
            }

            fn validate_config(&self, _name: &str, _view: &View) -> Result<()> {
                Ok(())
            }

            fn create_view(&self, _context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>> {
                Ok(Box::new(TestView))
            }
        }

        let mut registry = EngineRegistry::new();
        registry.register(Box::new(TestEngine));
        assert!(registry.contains("test"));
    }

    #[test]
    fn rejects_static_engine_field_shape_errors() {
        fn view(source: &str) -> View {
            toml::from_str(source).unwrap()
        }

        let registry = EngineRegistry::new();
        let embedded = view("type = 'embedded'\ncommand = 'sh'");
        assert!(registry.validate_config("bad-embedded", &embedded).is_err());

        let mixed_embedded =
            view("type = 'embedded'\ncommand = 'sh {{ runtime:view.current.input }}'");
        assert!(
            registry
                .validate_config("mixed-embedded", &mixed_embedded)
                .is_err()
        );

        let capture = view("type = 'capture'\noutput = 1");
        assert!(registry.validate_config("bad-capture", &capture).is_err());
    }
}
