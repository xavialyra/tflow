#[cfg(test)]
use super::api::EngineRegistration;
use super::api::{
    EngineValidationContext, InputBindingFactoryContext, MountRuntimeData, RendererFactoryContext,
    RuntimeFactoryContext, ViewFactory,
};
use crate::config::{Config, Defaults, EngineConfigValidator, View};
use crate::expression::validate_json_value;
use anyhow::{Context, Result, bail};
#[cfg(test)]
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) struct EngineRegistry {
    #[cfg(test)]
    overrides: BTreeMap<&'static str, EngineRegistration>,
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
        Self {
            #[cfg(test)]
            overrides: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn register(&mut self, registration: EngineRegistration) {
        self.overrides
            .insert(registration.definition.kind, registration);
    }

    #[cfg(test)]
    fn override_for(&self, engine_type: &str) -> Option<&EngineRegistration> {
        self.overrides.get(engine_type)
    }

    pub(crate) fn definition_for_engine(
        &self,
        engine_type: &str,
    ) -> Option<crate::engine::EngineDefinition> {
        #[cfg(test)]
        if let Some(registration) = self.override_for(engine_type) {
            return Some(registration.definition.clone());
        }
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
        #[cfg(test)]
        if self.override_for(view.selected_engine_type()).is_some() {
            return Ok(());
        }
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

impl ViewFactory for EngineRegistry {
    fn definition(
        &self,
        config: &Config,
        view_ref: &str,
    ) -> Result<crate::engine::EngineDefinition> {
        let engine_type = config.engine(view_ref)?;
        self.definition_for_engine(engine_type)
            .with_context(|| format!("unsupported view engine {:?}", engine_type))
    }

    fn create_mount_data(
        &self,
        config: &Config,
        identity: &crate::engine::ViewIdentity,
        task_lease: crate::task::MountTaskLease,
    ) -> Result<Option<MountRuntimeData>> {
        #[cfg(test)]
        if self.override_for(&identity.engine_type).is_some() {
            return Ok(None);
        }
        match identity.engine_type.as_str() {
            crate::config::ENGINE_PICKER => {
                super::picker::mount_data(config, &identity.view_ref, task_lease).map(Some)
            }
            crate::config::ENGINE_CAPTURE | crate::config::ENGINE_EMBEDDED => Ok(None),
            engine_type => bail!("unsupported view engine {:?}", engine_type),
        }
    }

    fn create_view(
        &self,
        context: RuntimeFactoryContext,
    ) -> Result<Box<dyn crate::engine::EngineRuntime>> {
        #[cfg(test)]
        if let Some(registration) = self.override_for(&context.identity.engine_type) {
            return (registration.create_runtime)(context);
        }
        match context.identity.engine_type.as_str() {
            crate::config::ENGINE_PICKER => super::picker::create_view(context),
            crate::config::ENGINE_CAPTURE => super::capture::create_view(context),
            crate::config::ENGINE_EMBEDDED => super::embedded::create_view(context),
            engine_type => bail!("unsupported view engine {:?}", engine_type),
        }
    }

    fn create_renderer(
        &self,
        context: RendererFactoryContext,
    ) -> Result<Box<dyn crate::engine::ViewRenderer>> {
        #[cfg(test)]
        if let Some(registration) = self.override_for(&context.identity.engine_type) {
            return (registration.create_renderer)(context);
        }
        match context.identity.engine_type.as_str() {
            crate::config::ENGINE_PICKER => super::picker::create_renderer(context),
            crate::config::ENGINE_CAPTURE => super::capture::create_renderer(context),
            crate::config::ENGINE_EMBEDDED => super::embedded::create_renderer(context),
            engine_type => bail!("unsupported view engine {:?}", engine_type),
        }
    }

    fn create_input_bindings(
        &self,
        context: InputBindingFactoryContext,
    ) -> Result<Vec<crate::command::InputActionBinding>> {
        #[cfg(test)]
        if let Some(registration) = self.override_for(&context.identity.engine_type) {
            return (registration.create_bindings)(context);
        }
        match context.identity.engine_type.as_str() {
            crate::config::ENGINE_PICKER => super::picker::create_input_bindings(context),
            crate::config::ENGINE_CAPTURE => super::capture::create_input_bindings(context),
            crate::config::ENGINE_EMBEDDED => super::embedded::create_input_bindings(context),
            engine_type => bail!("unsupported view engine {:?}", engine_type),
        }
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
    fn accepts_custom_engine_registrations() {
        struct TestView;

        impl crate::engine::EngineRuntime for TestView {
            fn tick_mode(&self) -> crate::engine::EngineTickMode {
                crate::engine::EngineTickMode::Prepared
            }

            fn render_model(&self) -> crate::engine::RenderModel {
                crate::engine::RenderModel::new("test", ())
            }
        }

        let registration = EngineRegistration::new(
            crate::engine::EngineDefinition::new("test", "test"),
            |_context| Ok(Box::new(TestView)),
        );
        let mut registry = EngineRegistry::new();
        registry.register(registration);
        assert!(registry.override_for("test").is_some());
        assert_eq!(registry.definition_for_engine("test").unwrap().kind, "test");
    }

    #[test]
    fn replacement_registration_delegates_input_binding_creation() {
        struct TestView;

        impl crate::engine::EngineRuntime for TestView {
            fn tick_mode(&self) -> crate::engine::EngineTickMode {
                crate::engine::EngineTickMode::Prepared
            }

            fn render_model(&self) -> crate::engine::RenderModel {
                crate::engine::RenderModel::new("test", ())
            }
        }

        let registration = EngineRegistration::new(
            crate::engine::EngineDefinition::new(crate::config::ENGINE_PICKER, "custom-picker"),
            |_context| Ok(Box::new(TestView)),
        )
        .with_input_binding_factory(|_context| {
            Ok(vec![crate::command::InputActionBinding {
                key: crate::input::Key::Escape,
                action: crate::command::ResolvedInputAction::Engine(crate::engine::ActionId::new(
                    "custom.cancel",
                )),
                label: Some("Cancel".to_string()),
                enabled: true,
            }])
        });

        let view_ref = "apps:main";
        let mut registry = EngineRegistry::new();
        registry.register(registration);

        let bindings = registry
            .create_input_bindings(InputBindingFactoryContext {
                identity: crate::engine::ViewIdentity::new(view_ref, crate::config::ENGINE_PICKER),
                bindings: crate::engine::EvaluatedBindingConfig::default(),
            })
            .unwrap();

        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].key, crate::input::Key::Escape);
        assert!(matches!(
            &bindings[0].action,
            crate::command::ResolvedInputAction::Engine(id) if id.as_str() == "custom.cancel"
        ));
        assert_eq!(
            registry
                .definition_for_engine(crate::config::ENGINE_PICKER)
                .unwrap()
                .renderer,
            "custom-picker"
        );
    }

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
            crate::engine::EngineDefinition::new("custom", "test").current_fields,
            [] as [&str; 0]
        );
    }

    #[test]
    fn engine_definitions_own_input_focus_policy() {
        let registry = EngineRegistry::new();
        assert_eq!(
            registry
                .definition_for_engine(crate::config::ENGINE_PICKER)
                .unwrap()
                .input
                .focus,
            crate::engine::InputFocus::Focused
        );
        for engine_type in [
            crate::config::ENGINE_CAPTURE,
            crate::config::ENGINE_EMBEDDED,
        ] {
            assert_eq!(
                registry
                    .definition_for_engine(engine_type)
                    .unwrap()
                    .input
                    .focus,
                crate::engine::InputFocus::Unfocused
            );
        }
    }

    #[test]
    fn engine_definitions_declare_terminal_eof_policy() {
        let registry = EngineRegistry::new();
        assert_eq!(
            registry
                .definition_for_engine(crate::config::ENGINE_PICKER)
                .unwrap()
                .mount_policy
                .terminal_eof,
            crate::engine::TerminalEofPolicy::Exit
        );
        assert_eq!(
            registry
                .definition_for_engine(crate::config::ENGINE_CAPTURE)
                .unwrap()
                .mount_policy
                .terminal_eof,
            crate::engine::TerminalEofPolicy::Exit
        );
        assert_eq!(
            registry
                .definition_for_engine(crate::config::ENGINE_EMBEDDED)
                .unwrap()
                .mount_policy
                .terminal_eof,
            crate::engine::TerminalEofPolicy::Close
        );
        assert_eq!(
            crate::engine::EngineDefinition::new("custom", "test")
                .mount_policy
                .terminal_eof,
            crate::engine::TerminalEofPolicy::Exit
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
