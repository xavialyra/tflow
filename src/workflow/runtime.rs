use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeStore {
    value: Value,
    revision: u64,
}

impl Default for RuntimeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeStore {
    pub(crate) fn new() -> Self {
        Self {
            value: Value::Object(serde_json::Map::new()),
            revision: 0,
        }
    }

    pub(crate) fn snapshot(&self) -> &Value {
        &self.value
    }

    #[cfg(test)]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn replace(&mut self, value: Value) -> u64 {
        self.value = value;
        self.revision = self.revision.wrapping_add(1);
        self.revision
    }

    #[cfg(test)]
    pub(crate) fn set(&mut self, pointer: &str, value: Value) -> Result<u64> {
        self.set_many([(pointer, value)])
    }

    pub(crate) fn set_many<'a>(
        &mut self,
        updates: impl IntoIterator<Item = (&'a str, Value)>,
    ) -> Result<u64> {
        let mut value = self.value.clone();
        let mut changed = false;
        for (pointer, update) in updates {
            set_pointer(&mut value, pointer, update)?;
            changed = true;
        }
        if !changed {
            return Ok(self.revision);
        }
        self.value = value;
        self.revision = self.revision.wrapping_add(1);
        Ok(self.revision)
    }
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
    fn set_many_publishes_one_atomic_revision() {
        let mut store = RuntimeStore::new();
        store.replace(serde_json::json!({
            "view": {"current": {"input": "old", "query": "old"}}
        }));
        let revision = store.revision();

        assert_eq!(
            store
                .set_many([
                    ("/view/current/input", serde_json::json!("new")),
                    ("/view/current/query", serde_json::json!("new")),
                ])
                .unwrap(),
            revision + 1
        );
        assert_eq!(store.snapshot()["view"]["current"]["input"], "new");
        assert_eq!(store.snapshot()["view"]["current"]["query"], "new");

        let committed = store.snapshot().clone();
        let revision = store.revision();
        assert!(
            store
                .set_many([
                    ("/view/current/input", serde_json::json!("partial")),
                    ("/missing/value", serde_json::json!(true)),
                ])
                .is_err()
        );
        assert_eq!(store.snapshot(), &committed);
        assert_eq!(store.revision(), revision);
    }

    #[test]
    fn tracks_revisions_and_json_pointer_updates() {
        let mut store = RuntimeStore::new();
        assert_eq!(store.revision(), 0);
        assert_eq!(
            store.replace(serde_json::json!({"view": {"current": {}}})),
            1
        );
        assert_eq!(
            store
                .set("/view/current/items", serde_json::json!([1, 2]))
                .unwrap(),
            2
        );
        assert_eq!(
            store.snapshot(),
            &serde_json::json!({"view": {"current": {"items": [1, 2]}}})
        );
        assert_eq!(
            store
                .set("/view/current/items/0", serde_json::json!(3))
                .unwrap(),
            3
        );
        assert_eq!(store.snapshot()["view"]["current"]["items"][0], 3);
    }
}
