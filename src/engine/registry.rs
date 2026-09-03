use super::api::EngineValidationContext;
use crate::config::{Config, Defaults, EngineConfigValidator, View};
use crate::expression::validate_json_value;
use anyhow::{Context, Result, bail};
use std::path::Path;

pub(crate) struct EngineRegistry;

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
        Self
    }

    pub(crate) fn definition(
        &self,
        config: &Config,
        view_ref: &str,
    ) -> Result<crate::engine::EngineDefinition> {
        let engine_type = config.engine(view_ref)?;
        self.definition_for_engine(engine_type)
            .with_context(|| format!("unsupported view engine {:?}", engine_type))
    }

    pub(crate) fn definition_for_engine(
        &self,
        engine_type: &str,
    ) -> Option<crate::engine::EngineDefinition> {
        match engine_type {
            crate::config::ENGINE_PICKER => Some(super::picker::definition()),
            crate::config::ENGINE_CAPTURE => Some(super::capture::definition()),
            crate::config::ENGINE_EMBEDDED => Some(super::embedded::definition()),
            _ => None,
        }
    }

    pub(crate) fn validate_defaults(&self, defaults: &Defaults) -> Result<()> {
        super::picker::validate_defaults(defaults).context("picker defaults")?;
        super::capture::validate_defaults(defaults).context("capture defaults")?;
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
        let context = EngineValidationContext {
            view_ref: name,
            view,
            script_root,
        };
        match view.selected_engine_type() {
            crate::config::ENGINE_PICKER => {
                super::picker::validate_config(context)?;
                super::picker::validate_keymap(name, view)
            }
            crate::config::ENGINE_CAPTURE => {
                super::capture::validate_config(context)?;
                super::capture::validate_keymap(name, view)
            }
            crate::config::ENGINE_EMBEDDED => {
                super::embedded::validate_config(context)?;
                default_validate_keymap(crate::config::ENGINE_EMBEDDED, name, view)
            }
            engine_type => bail!("view {:?} uses unsupported engine {:?}", name, engine_type),
        }
    }

    pub(crate) fn validate_relations(&self, config: &Config) -> Result<()> {
        super::picker::validate_relations(config)
    }
}

fn default_validate_keymap(engine_type: &str, name: &str, view: &View) -> Result<()> {
    if view.keymap.is_some() {
        bail!(
            "view {:?} using engine {:?} cannot define a keymap",
            name,
            engine_type
        );
    }
    Ok(())
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

    #[test]
    fn engine_definitions_are_the_action_schema_source() {
        let registry = EngineRegistry::new();
        let definition = registry.definition_for_engine("picker").unwrap();
        assert!(
            definition
                .action(&crate::engine::ActionId::new("picker.select_next"))
                .is_some()
        );
        assert!(
            definition
                .action(&crate::engine::ActionId::new("capture.copy"))
                .is_none()
        );
    }

    #[test]
    fn engine_definitions_declare_their_current_field_allowlists() {
        let registry = EngineRegistry::new();
        assert_eq!(
            registry
                .definition_for_engine(crate::config::ENGINE_PICKER)
                .unwrap()
                .current_fields,
            [
                "item",
                "source",
                "text",
                "value",
                "metadata",
                "selected_index"
            ]
        );
        assert_eq!(
            registry
                .definition_for_engine(crate::config::ENGINE_CAPTURE)
                .unwrap()
                .current_fields,
            ["value"]
        );
        assert_eq!(
            registry
                .definition_for_engine(crate::config::ENGINE_EMBEDDED)
                .unwrap()
                .current_fields,
            [] as [&str; 0]
        );
        assert_eq!(
            crate::engine::EngineDefinition::new().current_fields,
            [] as [&str; 0]
        );
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
