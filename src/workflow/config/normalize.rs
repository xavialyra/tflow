use crate::input::Key;
use anyhow::Context;
use std::collections::BTreeMap;

pub(super) fn normalize_view_bindings(
    value: &mut toml::Value,
    workflow_id: &str,
) -> anyhow::Result<()> {
    let views = value
        .as_table_mut()
        .context("workflow views must be a table")?;
    for (view_name, view) in views {
        let Some(view_table) = view.as_table_mut() else {
            continue;
        };
        let label = format!("view {workflow_id}:{view_name}");
        if let Some(unbind) = view_table.get_mut("unbind") {
            normalize_unbind(unbind, &label)?;
        }
        if let Some(bindings) = view_table
            .get_mut("bindings")
            .and_then(toml::Value::as_table_mut)
        {
            normalize_bindings_table(bindings, &label)?;
        }
    }
    Ok(())
}

/// Validates the shape of `[views.<name>.unbind]` and canonicalizes its
/// physical keys so two spellings of the same key cannot both appear.
fn normalize_unbind(value: &mut toml::Value, label: &str) -> anyhow::Result<()> {
    let table = value
        .as_table_mut()
        .with_context(|| format!("{label} unbind must be a table"))?;
    for field in ["keys", "commands", "layers"] {
        let Some(values) = table.get(field) else {
            continue;
        };
        let array = values
            .as_array()
            .with_context(|| format!("{label} unbind.{field} must be an array of strings"))?;
        if array.iter().any(|value| value.as_str().is_none()) {
            anyhow::bail!("{label} unbind.{field} must contain only strings");
        }
    }

    let Some(keys) = table.get_mut("keys").and_then(toml::Value::as_array_mut) else {
        return Ok(());
    };
    let mut source_by_key = BTreeMap::new();
    for entry in keys.iter_mut() {
        let source = entry
            .as_str()
            .expect("unbind.keys was validated as strings")
            .to_string();
        let canonical = Key::canonical_binding_name(&source)
            .with_context(|| format!("{label} unbind key {source:?}"))?;
        if let Some(previous) = source_by_key.insert(canonical.clone(), source.clone()) {
            anyhow::bail!(
                "{label} unbind.keys {previous:?} and {source:?} normalize to the same key {canonical:?}"
            );
        }
        *entry = toml::Value::String(canonical);
    }
    Ok(())
}

fn normalize_bindings_table(
    table: &mut toml::map::Map<String, toml::Value>,
    label: &str,
) -> anyhow::Result<()> {
    let entries = std::mem::replace(table, toml::map::Map::new());
    let mut normalized = toml::map::Map::new();

    let mut source_by_key = BTreeMap::new();
    for (source, value) in entries {
        let key = if source.is_empty() {
            String::new()
        } else {
            Key::canonical_binding_name(&source)
                .with_context(|| format!("{label} binding {:?}", source))?
        };
        if let Some(previous) = source_by_key.insert(key.clone(), source.clone()) {
            anyhow::bail!(
                "{label} bindings {:?} and {:?} normalize to the same key {:?}",
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
