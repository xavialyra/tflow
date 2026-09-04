use crate::input::Key;
use anyhow::Context;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn remove_disabled_plugins(value: &mut toml::Value, disabled: &BTreeSet<String>) {
    if let Some(plugins) = value.get_mut("plugins").and_then(toml::Value::as_table_mut) {
        for plugin_id in disabled {
            plugins.remove(plugin_id);
        }
    }
}

pub(super) fn normalize_keymap_tables(value: &mut toml::Value) -> anyhow::Result<()> {
    let Some(plugins) = value.get_mut("plugins").and_then(toml::Value::as_table_mut) else {
        return Ok(());
    };
    for (plugin_id, plugin) in plugins {
        let Some(views) = plugin.get_mut("views").and_then(toml::Value::as_table_mut) else {
            continue;
        };
        for (view_name, view) in views {
            let Some(keymap) = view
                .as_table_mut()
                .and_then(|view| view.get_mut("keymap"))
                .and_then(toml::Value::as_table_mut)
            else {
                continue;
            };
            normalize_keymap_table(keymap, &format!("view {plugin_id}:{view_name}"))?;
        }
    }
    Ok(())
}

pub(super) fn normalize_view_keymaps(
    value: &mut toml::Value,
    plugin_id: &str,
) -> anyhow::Result<()> {
    let views = value
        .as_table_mut()
        .context("plugin views must be a table")?;
    for (view_name, view) in views {
        let Some(keymap) = view
            .as_table_mut()
            .and_then(|view| view.get_mut("keymap"))
            .and_then(toml::Value::as_table_mut)
        else {
            continue;
        };
        normalize_keymap_table(keymap, &format!("view {plugin_id}:{view_name}"))?;
    }
    Ok(())
}

fn normalize_keymap_table(
    table: &mut toml::map::Map<String, toml::Value>,
    label: &str,
) -> anyhow::Result<()> {
    let entries = std::mem::replace(table, toml::map::Map::new());
    let mut normalized = toml::map::Map::new();
    let mut source_by_key = BTreeMap::new();
    for (source, value) in entries {
        let key = Key::parse_binding(&source)
            .with_context(|| format!("{label} keymap binding {:?}", source))?
            .binding_name()
            .with_context(|| {
                format!("{label} keymap binding {:?} has no canonical name", source)
            })?;
        if let Some(previous) = source_by_key.insert(key.clone(), source.clone()) {
            anyhow::bail!(
                "{label} keymap bindings {:?} and {:?} normalize to the same key {:?}",
                previous,
                source,
                key
            );
        }
        normalized.insert(key, value);
    }
    *table = normalized;
    Ok(())
}

pub(super) fn inject_builtin_commands_value(config: &mut Value) {
    let Some(root) = config.as_object_mut() else {
        return;
    };
    let commands = root
        .entry("commands".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(commands) = commands.as_object_mut() else {
        return;
    };
    let bindings = commands
        .entry("bindings".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(bindings) = bindings.as_object_mut() else {
        return;
    };
    let commands = bindings
        .entry("commands".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(commands) = commands.as_object_mut() else {
        return;
    };
    commands
        .entry("key".to_string())
        .or_insert_with(|| Value::String("ctrl+k".to_string()));
    commands
        .entry("label".to_string())
        .or_insert_with(|| Value::String("Commands".to_string()));
    commands
        .entry("visibility".to_string())
        .or_insert_with(|| Value::String("overflow".to_string()));
    commands
        .entry("type".to_string())
        .or_insert_with(|| Value::String("call".to_string()));
    commands.entry("payload".to_string()).or_insert_with(|| {
        serde_json::json!({
            "target": "selectors:commands",
            "query": {"commands": "{{ page.commands }}"},
            "presentation": {
                "mode": "popup",
                "width": 72,
                "height": 16
            },
            "engine": {
                "show_input": false,
                "show_divider": false,
                "show_prefix": true
            },
            "then": {
                "type": "invoke",
                "payload": {"command": "{{ result.output.value }}"}
            }
        })
    });
}

pub(super) fn normalize_engine_configs(config: &mut Value) {
    let Some(plugins) = config.get_mut("plugins").and_then(Value::as_object_mut) else {
        return;
    };
    for plugin in plugins.values_mut() {
        let Some(views) = plugin.get_mut("views").and_then(Value::as_object_mut) else {
            continue;
        };
        for view in views.values_mut() {
            let Some(view) = view.as_object_mut() else {
                continue;
            };
            let Some(engine) = view.get("engine").cloned() else {
                continue;
            };
            let Some(engine) = engine.as_object() else {
                continue;
            };
            let Some(engine_type) = engine.get("type").cloned() else {
                continue;
            };
            let Some(config) = engine.get("config").and_then(Value::as_object) else {
                continue;
            };
            view.insert("type".to_string(), engine_type);
            for (key, value) in config {
                view.insert(key.clone(), value.clone());
            }
        }
    }
}
