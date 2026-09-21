use crate::input::Key;
use anyhow::Context;
use std::collections::BTreeMap;

pub(super) fn normalize_view_keymaps(
    value: &mut toml::Value,
    workflow_id: &str,
) -> anyhow::Result<()> {
    let views = value
        .as_table_mut()
        .context("workflow views must be a table")?;
    for (view_name, view) in views {
        let Some(keymap) = view
            .as_table_mut()
            .and_then(|view| view.get_mut("keymap"))
            .and_then(toml::Value::as_table_mut)
        else {
            continue;
        };
        normalize_keymap_table(keymap, &format!("view {workflow_id}:{view_name}"))?;
    }
    Ok(())
}

fn normalize_keymap_table(
    table: &mut toml::map::Map<String, toml::Value>,
    label: &str,
) -> anyhow::Result<()> {
    let mode_val = table.remove("mode");
    let entries = std::mem::replace(table, toml::map::Map::new());
    let mut normalized = toml::map::Map::new();
    if let Some(mode) = mode_val {
        let mode_str = mode
            .as_str()
            .with_context(|| format!("{label} keymap mode must be a string"))?;
        if mode_str != "static" && mode_str != "item" {
            anyhow::bail!("{label} keymap mode must be \"static\" or \"item\", got {:?}", mode_str);
        }
        normalized.insert("mode".to_string(), mode);
    }

    let mut source_by_key = BTreeMap::new();
    for (source, value) in entries {
        let key = if source.is_empty() {
            String::new()
        } else {
            Key::parse_binding(&source)
                .with_context(|| format!("{label} keymap binding {:?}", source))?
                .binding_name()
                .with_context(|| {
                    format!("{label} keymap binding {:?} has no canonical name", source)
                })?
        };
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
