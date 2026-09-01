use crate::config::Config;
use crate::input::EditorBuffer;
use crate::parameter::ParameterState;
use crate::runtime::RuntimeStore;
use anyhow::Result;
use serde_json::{Value, json};

pub(super) fn publish_active_input(
    runtime: &mut RuntimeStore,
    input: &EditorBuffer,
    parameter_input: &str,
    state: &ParameterState,
) -> Result<()> {
    runtime.set_many([
        ("/view/current/state_revision", json!(state.revision())),
        ("/view/current/input", json!(parameter_input)),
        ("/view/current/raw_input", json!(input.raw)),
        ("/view/current/query", json!(parameter_input)),
        ("/view/current/cursor", json!(input.cursor)),
        ("/view/current/buffer_revision", json!(input.revision)),
        (
            "/session/input",
            json!({"raw": input.raw, "params": parameter_input, "cursor": input.cursor, "revision": input.revision}),
        ),
    ])?;
    Ok(())
}

pub(super) fn publish_location(
    runtime: &mut RuntimeStore,
    config: &Config,
    view_ref: &str,
    input: &EditorBuffer,
    parameter_input: &str,
    state: &ParameterState,
) -> Result<()> {
    let commands = crate::command::collect_page_owner_commands(config, view_ref, None)?
        .into_values()
        .collect::<Vec<_>>();
    let view = json!({
        "current": {
            "ref": view_ref,
            "state_revision": state.revision(),
            "input": parameter_input,
            "raw_input": input.raw,
            "query": parameter_input,
            "cursor": input.cursor,
            "buffer_revision": input.revision,
            "selected_item": Value::Null,
            "items": [],
            "command": commands,
            "command_owner": view_ref,
        }
    });
    let input = json!({
        "raw": input.raw,
        "params": parameter_input,
        "cursor": input.cursor,
        "revision": input.revision,
    });
    if runtime.snapshot().get("session").is_some() {
        runtime.set_many([("/view", view), ("/session/input", input)])?;
    } else {
        runtime.set_many([("/view", view), ("/session", json!({"input": input}))])?;
    }
    Ok(())
}

pub(super) fn publish_view_catalog(runtime: &mut RuntimeStore, config: &Config) -> Result<()> {
    let views = config
        .iter_views()
        .map(|(view_ref, view)| {
            json!({
                "label": view.alias.as_deref().unwrap_or(view_ref),
                "value": view_ref,
                "metadata": {
                    "alias": view.alias,
                    "engine": view.selected_engine_type(),
                    "plugin": view_ref.split_once(':').map(|(plugin, _)| plugin).unwrap_or(view_ref),
                },
            })
        })
        .collect::<Vec<_>>();
    runtime.set_many([("/session/views", json!(views))])?;
    Ok(())
}
