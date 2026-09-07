use anyhow::{Context, Result, bail};
use serde_json::Value;

pub(crate) fn apply_pointer(root: &Value, pointer: &str, value: Value) -> Result<Value> {
    let mut updated = root.clone();
    set_pointer(&mut updated, pointer, value)?;
    Ok(updated)
}

fn set_pointer(root: &mut Value, pointer: &str, value: Value) -> Result<()> {
    if pointer.is_empty() {
        *root = value;
        return Ok(());
    }
    let (parent_pointer, key) = pointer
        .rsplit_once('/')
        .context("runtime JSON Pointer must identify a value")?;
    let parent = if parent_pointer.is_empty() {
        root
    } else {
        root.pointer_mut(parent_pointer)
            .context("runtime JSON Pointer parent does not exist")?
    };
    let key = decode_json_pointer_token(key)?;
    match parent {
        Value::Object(object) => {
            object.insert(key, value);
        }
        Value::Array(array) => {
            let index = key
                .parse::<usize>()
                .with_context(|| format!("runtime array index {:?} is invalid", key))?;
            let slot = array
                .get_mut(index)
                .context("runtime JSON Pointer array index does not exist")?;
            *slot = value;
        }
        _ => bail!("runtime JSON Pointer parent is not a container"),
    }
    Ok(())
}

fn decode_json_pointer_token(token: &str) -> Result<String> {
    let mut decoded = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(character) = chars.next() {
        if character != '~' {
            decoded.push(character);
            continue;
        }
        match chars.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            Some(other) => bail!("invalid JSON Pointer escape ~{}", other),
            None => bail!("unterminated JSON Pointer escape"),
        }
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_pointer_updates_nested_values() {
        let root = serde_json::json!({"view": {"current": {"input": "old"}}});
        let updated =
            apply_pointer(&root, "/view/current/input", serde_json::json!("new")).unwrap();
        assert_eq!(updated["view"]["current"]["input"], "new");
    }
}
