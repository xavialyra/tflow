use super::api::{Engine, EngineValidationContext, ViewContext, ViewFactory, ViewInstance};
use crate::config::{Config, Defaults, EngineConfigValidator, View};
use crate::expression::validate_json_value;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) struct EngineRegistry {
    engines: BTreeMap<&'static str, Box<dyn Engine>>,
}

impl EngineConfigValidator for EngineRegistry {
    fn validate_defaults(&self, defaults: &Defaults) -> Result<()> {
        EngineRegistry::validate_defaults(self, defaults)
    }

    fn validate_view(&self, name: &str, view: &View, script_root: Option<&Path>) -> Result<()> {
        self.validate_view_with_root(name, view, script_root)
    }

    fn validate_relations(&self, config: &Config) -> Result<()> {
        EngineRegistry::validate_relations(self, config)
    }
}

impl EngineRegistry {
    pub(crate) fn new() -> Self {
        let mut registry = Self {
            engines: BTreeMap::new(),
        };
        registry.register(Box::new(super::picker::PickerEngine));
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

    pub(crate) fn validate_defaults(&self, defaults: &Defaults) -> Result<()> {
        for engine in self.engines.values() {
            engine
                .validate_defaults(defaults)
                .with_context(|| format!("{} defaults", engine.engine_type()))?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn validate_config(&self, name: &str, view: &View) -> Result<()> {
        self.validate_view_with_root(name, view, None)
    }

    fn validate_view_with_root(
        &self,
        name: &str,
        view: &View,
        script_root: Option<&Path>,
    ) -> Result<()> {
        let engine = self
            .engines
            .get(view.selected_engine_type())
            .with_context(|| {
                format!(
                    "view {:?} uses unsupported engine {:?}",
                    name,
                    view.selected_engine_type()
                )
            })?;
        if !engine.supports_data_sources()
            && (view.selected_items().is_some() || !view.selected_feeds().is_empty())
        {
            bail!(
                "view {:?} using engine {:?} cannot provide picker items",
                name,
                view.selected_engine_type()
            );
        }
        engine.validate_config(EngineValidationContext {
            view_ref: name,
            view,
            script_root,
        })?;
        engine.validate_keymap(name, view)
    }

    pub(crate) fn validate_relations(&self, config: &Config) -> Result<()> {
        for engine in self.engines.values() {
            engine.validate_relations(config)?;
        }
        Ok(())
    }
}

impl ViewFactory for EngineRegistry {
    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>> {
        let engine_type = context.config.engine(&context.request.view_ref)?;
        let engine = self
            .engines
            .get(engine_type)
            .with_context(|| format!("unsupported view engine {:?}", engine_type))?;
        engine.create_view(context)
    }
}

pub(crate) fn validate_fields(name: &str, view: &View, allowed: &[&str]) -> Result<()> {
    for (field, value) in view.selected_engine_config() {
        if !allowed.contains(&field.as_str()) {
            bail!(
                "view {:?} using engine {:?} has unsupported field {:?}",
                name,
                view.selected_engine_type(),
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
            view.selected_engine_type(),
            field
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineHost, EngineTerminal, ViewContext, ViewEffect, ViewInstance};
    use anyhow::Result;

    #[test]
    fn accepts_custom_engine_implementations() {
        struct TestEngine;

        struct TestView;

        impl ViewInstance for TestView {
            fn step(
                &mut self,
                _host: &mut EngineHost<'_>,
                _terminal: &mut dyn EngineTerminal,
            ) -> Result<ViewEffect> {
                Ok(ViewEffect::Continue)
            }

            fn render(
                &mut self,
                _host: &EngineHost<'_>,
                _frame: &mut ratatui::Frame,
                _area: ratatui::layout::Rect,
            ) {
            }
        }

        impl Engine for TestEngine {
            fn engine_type(&self) -> &'static str {
                "test"
            }

            fn validate_config(&self, _context: EngineValidationContext<'_>) -> Result<()> {
                Ok(())
            }

            fn validate_relations(&self, _config: &Config) -> Result<()> {
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
        let embedded = view("[engine]\ntype = 'embedded'\n[engine.config]\ncommand = 'sh'");
        assert!(registry.validate_config("bad-embedded", &embedded).is_err());

        let mixed_embedded =
            view("[engine]\ntype = 'embedded'\n[engine.config]\ncommand = 'sh {{ page.input }}'");
        assert!(
            registry
                .validate_config("mixed-embedded", &mixed_embedded)
                .is_err()
        );

        let bound_embedded = view(
            "[engine]\ntype = 'embedded'\n[engine.config]\ncommand = ['sh']\n[commands.cancel]\nkey = 'ctrl+b'\nlabel = 'Cancel'\npassthrough = true\ntype = 'return'",
        );
        registry
            .validate_config("bound-embedded", &bound_embedded)
            .expect("passthrough View commands should be accepted");

        let capture = view("[engine]\ntype = 'capture'\n[engine.config]\noutput = 1");
        assert!(registry.validate_config("bad-capture", &capture).is_err());

        let capture_with_items =
            view("[engine]\ntype = 'capture'\n[engine.config]\noutput = 'ok'\nitems = []");
        let error = registry
            .validate_config("capture-with-items", &capture_with_items)
            .expect_err("non-picker engines must reject picker data-source fields");
        assert!(error.to_string().contains("cannot provide picker items"));

        let image = view("[engine]\ntype = 'image'\n[engine.config]\npath = 'cover.png'");
        assert!(
            registry
                .validate_config("bad-image-engine", &image)
                .is_err()
        );
    }
}
