use serde_json::Value;

/// Snapshot of the active context available for projection into dynamic
/// parameters, arguments, and companion slots.
#[derive(Debug, Clone, Default)]
pub(crate) struct ContextSource<'a> {
    pub(crate) selection: Option<&'a Value>,
    pub(crate) input: Option<&'a str>,
    pub(crate) query: Option<&'a Value>,
}

impl<'a> ContextSource<'a> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_selection(mut self, selection: Option<&'a Value>) -> Self {
        self.selection = selection;
        self
    }

    pub(crate) fn with_input(mut self, input: Option<&'a str>) -> Self {
        self.input = input;
        self
    }

    pub(crate) fn with_query(mut self, query: Option<&'a Value>) -> Self {
        self.query = query;
        self
    }

    /// Resolves a single path expression like `selection`, `selection.value`, `input`, `query.id`.
    pub(crate) fn resolve_path(&self, path: &str) -> Option<Value> {
        let (root, rest) = match path.split_once('.') {
            Some((root, rest)) => (root, Some(rest)),
            None => (path, None),
        };

        let root_val = match root {
            "selection" | "item" => self.selection?,
            "input" => return Some(Value::String(self.input.unwrap_or_default().to_string())),
            "query" | "parameters" => self.query?,
            _ => return None,
        };

        match rest {
            Some(sub_path) => get_nested_value(root_val, sub_path).cloned(),
            None => Some(root_val.clone()),
        }
    }
}

/// Recursively evaluates dynamic projection tokens in a JSON Value template.
///
/// Rules:
/// 1. Exact token match (`"$selection"` or `"$selection.value"`):
///    Preserves the underlying JSON type (Object, Number, Bool, String).
/// 2. Embedded string interpolation (`"prefix-$selection.id-suffix"`):
///    Replaces each `$<token>` with its string representation.
/// 3. Arrays and Objects:
///    Recursively evaluates all nested children.
pub(crate) fn project_value(template: &Value, ctx: &ContextSource<'_>) -> Value {
    match template {
        Value::String(s) => project_string(s, ctx),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| project_value(item, ctx)).collect())
        }
        Value::Object(map) => {
            let mut projected = serde_json::Map::new();
            for (key, val) in map {
                projected.insert(key.clone(), project_value(val, ctx));
            }
            Value::Object(projected)
        }
        literal => literal.clone(),
    }
}

/// Evaluates a string value, checking first for exact token replacement, then interpolation.
fn project_string(s: &str, ctx: &ContextSource<'_>) -> Value {
    // 1. Exact match check (e.g. "$selection", "$selection.val")
    if let Some(token) = s.strip_prefix('$')
        && !token.is_empty()
        && is_valid_token_path(token)
    {
        return ctx.resolve_path(token).unwrap_or(Value::Null);
    }

    // 2. String interpolation check
    if !s.contains('$') {
        return Value::String(s.to_string());
    }

    let mut result = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();

    while let Some((i, ch)) = chars.next() {
        if ch == '$' {
            // Find end of token path (alphanumeric, underscore, dot)
            let mut end = i + 1;
            while let Some(&(next_idx, next_ch)) = chars.peek() {
                if next_ch.is_alphanumeric() || next_ch == '_' || next_ch == '.' {
                    end = next_idx + next_ch.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }

            let token = &s[i + 1..end];
            if is_valid_token_path(token) {
                if let Some(val) = ctx.resolve_path(token) {
                    append_value_as_str(&mut result, &val);
                }
            } else {
                result.push('$');
                result.push_str(token);
            }
        } else {
            result.push(ch);
        }
    }

    Value::String(result)
}

fn is_valid_token_path(token: &str) -> bool {
    if token.is_empty() || token.starts_with('.') || token.ends_with('.') {
        return false;
    }
    let (root, _) = match token.split_once('.') {
        Some((r, rest)) => (r, rest),
        None => (token, ""),
    };
    matches!(
        root,
        "selection" | "item" | "input" | "query" | "parameters"
    )
}

fn get_nested_value<'a>(val: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = val;
    for part in path.split('.') {
        if part.is_empty() {
            return None;
        }
        match current {
            Value::Object(map) => {
                current = map.get(part).or_else(|| {
                    if part == "display" {
                        map.get("text")
                    } else if part == "text" {
                        map.get("display")
                    } else {
                        None
                    }
                })?;
            }
            Value::Array(arr) => {
                let idx: usize = part.parse().ok()?;
                current = arr.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn append_value_as_str(buf: &mut String, val: &Value) {
    match val {
        Value::String(s) => buf.push_str(s),
        Value::Number(n) => buf.push_str(&n.to_string()),
        Value::Bool(b) => buf.push_str(&b.to_string()),
        Value::Null => {}
        other => buf.push_str(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_token_preserves_primitive_types() {
        let selection = json!({
            "id": 123,
            "active": true,
            "title": "demo item",
            "metadata": {
                "tag": "prod"
            }
        });
        let ctx = ContextSource::new()
            .with_selection(Some(&selection))
            .with_input(Some("search-keyword"));

        // Exact number
        assert_eq!(project_value(&json!("$selection.id"), &ctx), json!(123));
        // Exact boolean
        assert_eq!(
            project_value(&json!("$selection.active"), &ctx),
            json!(true)
        );
        // Exact string
        assert_eq!(
            project_value(&json!("$selection.title"), &ctx),
            json!("demo item")
        );
        // Exact nested
        assert_eq!(
            project_value(&json!("$selection.metadata.tag"), &ctx),
            json!("prod")
        );
        // Exact full object
        assert_eq!(project_value(&json!("$selection"), &ctx), selection);
        // Exact input
        assert_eq!(
            project_value(&json!("$input"), &ctx),
            json!("search-keyword")
        );
    }

    #[test]
    fn string_interpolation_formats_into_text() {
        let selection = json!({
            "service": "postgres",
            "port": 5432
        });
        let ctx = ContextSource::new()
            .with_selection(Some(&selection))
            .with_input(Some("staging"));

        let template = json!("connect to $selection.service:$selection.port on $input");
        assert_eq!(
            project_value(&template, &ctx),
            json!("connect to postgres:5432 on staging")
        );
    }

    #[test]
    fn recursive_objects_and_arrays_are_projected() {
        let selection = json!({
            "name": "myapp",
            "version": "1.0.0"
        });
        let ctx = ContextSource::new().with_selection(Some(&selection));

        let template = json!({
            "app_name": "$selection.name",
            "tags": ["latest", "$selection.version"],
            "config": {
                "tag": "release-$selection.version"
            }
        });

        assert_eq!(
            project_value(&template, &ctx),
            json!({
                "app_name": "myapp",
                "tags": ["latest", "1.0.0"],
                "config": {
                    "tag": "release-1.0.0"
                }
            })
        );
    }

    #[test]
    fn missing_tokens_resolve_gracefully() {
        let ctx = ContextSource::new();
        // Exact missing token yields Null
        assert_eq!(
            project_value(&json!("$selection.missing"), &ctx),
            Value::Null
        );
        // Interpolated missing token resolves to empty string
        assert_eq!(
            project_value(&json!("prefix-$selection.missing-suffix"), &ctx),
            json!("prefix--suffix")
        );
    }

    #[test]
    fn literal_strings_without_tokens_are_untouched() {
        let ctx = ContextSource::new().with_input(Some("test"));
        assert_eq!(
            project_value(&json!("hello world"), &ctx),
            json!("hello world")
        );
        assert_eq!(
            project_value(&json!("price: $100"), &ctx),
            json!("price: $100")
        );
    }
}
