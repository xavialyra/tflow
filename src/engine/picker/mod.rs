mod command;
mod items;
mod keymap;
mod preview;
mod render;
mod runtime;
mod session;
mod tasks;

use self::keymap::PickerKeymap;
use self::session::PickerOptions;
pub(crate) use self::session::PickerView;
use self::tasks::PickerItemsScheduler;
use super::{Engine, EngineValidationContext, ViewContext, ViewInstance, validate_fields};
use crate::config::{
    Config, ConfigSource, Defaults, ENGINE_PICKER, ScriptSourceSpec, View, toml_to_json,
};
use crate::expression::{EvaluationStage, Template, is_dynamic_string};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

pub(crate) use items::Item;

#[derive(Debug, Clone, Copy)]
pub(super) enum PendingAction {
    Activate(crate::input::Key),
}

pub(crate) struct PickerEngine;

impl Engine for PickerEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_PICKER
    }

    fn validate_config(&self, context: EngineValidationContext<'_>) -> Result<()> {
        let name = context.view_ref;
        let view = context.view;
        validate_fields(name, view, &["show_prefix", "layout", "preview"])?;
        validate_picker_bool(view.engine_field("show_prefix"), "show_prefix")?;
        validate_picker_table(view.engine_field("layout"), "layout")?;
        validate_picker_table(view.engine_field("preview"), "preview")?;
        if let Some(items) = view.selected_items() {
            validate_items_source_config(items, context.script_root).with_context(|| {
                format!("view {:?} has invalid items source configuration", name)
            })?;
        }
        Ok(())
    }

    fn supports_data_sources(&self) -> bool {
        true
    }

    fn validate_relations(&self, config: &Config) -> Result<()> {
        for (view_ref, view) in config.iter_views() {
            if config.engine(view_ref)? != self.engine_type() {
                continue;
            }
            let feeds = view.selected_feeds();
            if feeds.is_empty() {
                continue;
            }
            if view.selected_items().is_some() {
                bail!("feeds view {:?} cannot define items", view_ref);
            }
            let mut seen_feeds = BTreeSet::new();
            for feed in feeds {
                let feed_ref = &feed.view;
                if !seen_feeds.insert(feed_ref.clone()) {
                    bail!(
                        "view {:?} lists feed {:?} more than once",
                        view_ref,
                        feed_ref
                    );
                }
                let feed_view = config.view(feed_ref).with_context(|| {
                    format!("view {:?} references missing feed {:?}", view_ref, feed_ref)
                })?;
                if config.engine(feed_ref)? != ENGINE_PICKER {
                    bail!(
                        "view {:?} feed {:?} does not use the picker engine",
                        view_ref,
                        feed_ref
                    );
                }
                if feed_view.is_feeds_page() {
                    bail!(
                        "view {:?} cannot use feeds view {:?} as a feed",
                        view_ref,
                        feed_ref
                    );
                }
                if feed_view.selected_items().is_none() {
                    bail!("view {:?} feed {:?} must define items", view_ref, feed_ref);
                }
            }
        }
        Ok(())
    }

    fn validate_defaults(&self, defaults: &Defaults) -> Result<()> {
        let bindings = defaults
            .picker
            .bindings
            .as_ref()
            .map(toml_to_json)
            .transpose()?;
        PickerKeymap::validate_values(bindings.as_ref(), None).context("picker bindings")
    }

    fn validate_keymap(&self, name: &str, view: &View) -> Result<()> {
        let keymap = view.keymap.as_ref().map(toml_to_json).transpose()?;
        PickerKeymap::validate_values(None, keymap.as_ref())
            .with_context(|| format!("view {:?} picker keymap", name))
    }

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>> {
        let default_bindings = context.config.get(
            ConfigSource::Root,
            &context.evaluation,
            EvaluationStage::Operation,
            &["defaults", "picker", "bindings"],
        )?;
        let source = ConfigSource::View(context.state.view_ref());
        let get_view_field = |path: &[&str]| {
            context.config.get(
                source,
                &context.evaluation,
                EvaluationStage::Operation,
                path,
            )
        };
        let view_keymap = get_view_field(&["keymap"])?;
        let show_prefix = get_view_field(&["show_prefix"])?;
        let layout = get_view_field(&["layout"])?;
        let preview = get_view_field(&["preview"])?;
        let keymap = PickerKeymap::from_values(default_bindings, view_keymap)?;
        let options = PickerOptions {
            show_prefix: parse_bool(show_prefix, "show_prefix", false)?,
            preview: self::preview::parse(layout, preview)?,
        };
        let picker = PickerView::new(
            &context.request.view_ref,
            PickerItemsScheduler::new(context.tasks.clone(), &context.request.view_ref),
            Arc::new(context.config.clone()),
            keymap,
            options,
        );
        Ok(Box::new(picker))
    }
}

fn validate_items_source_config(value: &toml::Value, root: Option<&Path>) -> Result<()> {
    match value {
        toml::Value::Array(_) => Ok(()),
        toml::Value::String(source) => {
            let template = Template::parse(source)?;
            if template.is_complete_path() {
                Ok(())
            } else {
                bail!("items must be an array, complete dynamic path, or script source object")
            }
        }
        toml::Value::Table(_) => {
            let spec = ScriptSourceSpec::parse(value)
                .context("items must be an array or a script source object")?;
            spec.validate_picker_source()?;
            if spec
                .file_value()
                .is_some_and(|file| !is_dynamic_string(file))
            {
                let root = root.context("script items source has no plugin root")?;
                spec.validate_target(root)?;
            }
            Ok(())
        }
        _ => bail!("items must be an array, complete dynamic path, or script source object"),
    }
}

fn validate_picker_bool(value: Option<&toml::Value>, name: &str) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_bool() || is_complete_dynamic_path(value)? {
        return Ok(());
    }
    bail!(
        "picker field {:?} must be a boolean or complete dynamic path",
        name
    )
}

fn validate_picker_table(value: Option<&toml::Value>, name: &str) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_table() || is_complete_dynamic_path(value)? {
        return Ok(());
    }
    bail!(
        "picker field {:?} must be a table or complete dynamic path",
        name
    )
}

fn is_complete_dynamic_path(value: &toml::Value) -> Result<bool> {
    let Some(source) = value.as_str() else {
        return Ok(false);
    };
    Ok(Template::parse(source)?.is_complete_path())
}

fn parse_bool(value: Option<Value>, name: &str, default: bool) -> Result<bool> {
    value
        .map(|value| {
            value
                .as_bool()
                .with_context(|| format!("picker field {:?} must evaluate to a boolean", name))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_runtime_fields_allow_dynamic_values_for_post_resolution_validation() {
        let view: View = toml::from_str(
            r#"
            [engine]
            type = "picker"
            [engine.config]
            show_prefix = "{{ page.query.show_prefix }}"
            layout = "{{ page.query.layout }}"
            preview = "{{ page.query.preview }}"
            "#,
        )
        .unwrap();
        PickerEngine
            .validate_config(EngineValidationContext {
                view_ref: "core:dynamic",
                view: &view,
                script_root: None,
            })
            .unwrap();
    }
}
