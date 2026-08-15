mod methods;
mod path;
mod script;

pub(crate) use methods::ExpressionMethods;
#[cfg(test)]
pub(crate) use path::apply_path;

use anyhow::{Result, bail};
use serde_json::{Map, Number, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(Value),
    Ref {
        namespace: String,
        path: String,
    },
    Call {
        name: String,
        args: Vec<Expr>,
        named_args: BTreeMap<String, Expr>,
    },
    Array(Vec<Expr>),
    Object(BTreeMap<String, Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TemplatePart {
    Text(String),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    parts: Vec<TemplatePart>,
}

pub trait ReferenceResolver {
    fn resolve_reference(&self, namespace: &str, path: &str) -> Result<Value>;
}

pub trait MethodResolver {
    fn call_method(
        &mut self,
        name: &str,
        args: Vec<Value>,
        named_args: BTreeMap<String, Value>,
    ) -> Result<Value>;
}

pub struct EvalContext<'a> {
    pub references: &'a dyn ReferenceResolver,
    pub methods: &'a mut dyn MethodResolver,
}

pub struct TreeReferences<'a> {
    pub config: &'a Value,
    pub this: &'a Value,
    pub runtime: &'a Value,
    pub input: &'a Value,
    pub request: Option<&'a Value>,
    pub returned: Option<&'a Value>,
}

impl ReferenceResolver for TreeReferences<'_> {
    fn resolve_reference(&self, namespace: &str, path: &str) -> Result<Value> {
        let root = match namespace {
            "config" => self.config,
            "this" => self.this,
            "runtime" => self.runtime,
            "input" => self.input,
            "request" => self
                .request
                .ok_or_else(|| anyhow::anyhow!("request references are not available here"))?,
            "return" => self
                .returned
                .ok_or_else(|| anyhow::anyhow!("return references are not available here"))?,
            _ => bail!("unknown reference namespace {:?}", namespace),
        };
        lookup_path(root, path)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("reference {:?}:{:?} was not found", namespace, path))
    }
}

impl Template {
    pub fn parse(source: &str) -> Result<Self> {
        let mut parts = Vec::new();
        let mut cursor = 0;
        while let Some(open) = find_unescaped_open(source, cursor) {
            if open > cursor {
                parts.push(TemplatePart::Text(source[cursor..open].to_string()));
            }
            let close = find_closing_delimiter(source, open)
                .ok_or_else(|| anyhow::anyhow!("unclosed expression starting at byte {open}"))?;
            let expression = parse_expression(&source[open + 2..close])?;
            parts.push(TemplatePart::Expr(expression));
            cursor = close + 2;
        }
        if cursor < source.len() {
            parts.push(TemplatePart::Text(source[cursor..].to_string()));
        }
        if parts.is_empty() {
            parts.push(TemplatePart::Text(String::new()));
        }
        Ok(Self { parts })
    }

    pub fn is_complete_expression(&self) -> bool {
        matches!(self.parts.as_slice(), [TemplatePart::Expr(_)])
    }

    #[cfg(test)]
    pub fn evaluate_value(&self, context: &mut EvalContext<'_>) -> Result<Value> {
        if let [TemplatePart::Expr(expression)] = self.parts.as_slice() {
            return evaluate_expression(expression, context, &mut BTreeSet::new());
        }
        self.evaluate_text(context).map(Value::String)
    }

    pub fn evaluate_text(&self, context: &mut EvalContext<'_>) -> Result<String> {
        let mut result = String::new();
        for part in &self.parts {
            match part {
                TemplatePart::Text(text) => result.push_str(text),
                TemplatePart::Expr(expression) => {
                    let value = evaluate_expression(expression, context, &mut BTreeSet::new())?;
                    result.push_str(&value_to_text(&value)?);
                }
            }
        }
        Ok(result)
    }
}

pub fn validate_json_value(value: &Value) -> Result<()> {
    match value {
        Value::String(source) if source.contains("{{") => {
            Template::parse(source)?;
        }
        Value::Array(values) => {
            for value in values {
                validate_json_value(value)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_json_value(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn evaluate_json_value(value: &Value, context: &mut EvalContext<'_>) -> Result<Value> {
    evaluate_json_value_with_stack(value, context, &mut BTreeSet::new())
}

#[allow(dead_code)]
pub fn evaluate_argv(values: &[Template], context: &mut EvalContext<'_>) -> Result<Vec<String>> {
    values
        .iter()
        .map(|value| value.evaluate_text(context))
        .collect()
}

fn evaluate_json_value_with_stack(
    value: &Value,
    context: &mut EvalContext<'_>,
    reference_stack: &mut BTreeSet<String>,
) -> Result<Value> {
    match value {
        Value::String(source) if source.contains("{{") => {
            let template = Template::parse(source)?;
            evaluate_template_with_stack(&template, context, reference_stack)
        }
        Value::Array(values) => values
            .iter()
            .map(|value| evaluate_json_value_with_stack(value, context, reference_stack))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.clone(),
                    evaluate_json_value_with_stack(value, context, reference_stack)?,
                ))
            })
            .collect::<Result<Map<_, _>>>()
            .map(Value::Object),
        value => Ok(value.clone()),
    }
}

fn evaluate_template_with_stack(
    template: &Template,
    context: &mut EvalContext<'_>,
    reference_stack: &mut BTreeSet<String>,
) -> Result<Value> {
    if let [TemplatePart::Expr(expression)] = template.parts.as_slice() {
        return evaluate_expression(expression, context, reference_stack);
    }
    let mut result = String::new();
    for part in &template.parts {
        match part {
            TemplatePart::Text(text) => result.push_str(text),
            TemplatePart::Expr(expression) => {
                let value = evaluate_expression(expression, context, reference_stack)?;
                result.push_str(&value_to_text(&value)?);
            }
        }
    }
    Ok(Value::String(result))
}

fn evaluate_expression(
    expression: &Expr,
    context: &mut EvalContext<'_>,
    reference_stack: &mut BTreeSet<String>,
) -> Result<Value> {
    match expression {
        Expr::Literal(value) => Ok(value.clone()),
        Expr::Ref { namespace, path } => {
            let key = format!("{namespace}:{path}");
            if namespace == "config" && !reference_stack.insert(key.clone()) {
                bail!("cyclic expression reference involving {key:?}");
            }
            let value = context.references.resolve_reference(namespace, path)?;
            let result = if namespace == "config" {
                evaluate_json_value_with_stack(&value, context, reference_stack)
            } else {
                Ok(value)
            };
            if namespace == "config" {
                reference_stack.remove(&key);
            }
            result
        }
        Expr::Call {
            name,
            args,
            named_args,
        } => {
            let args = args
                .iter()
                .map(|argument| evaluate_expression(argument, context, reference_stack))
                .collect::<Result<Vec<_>>>()?;
            let named_args = named_args
                .iter()
                .map(|(key, argument)| {
                    Ok((
                        key.clone(),
                        evaluate_expression(argument, context, reference_stack)?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            context.methods.call_method(name, args, named_args)
        }
        Expr::Array(values) => values
            .iter()
            .map(|value| evaluate_expression(value, context, reference_stack))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Expr::Object(values) => values
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.clone(),
                    evaluate_expression(value, context, reference_stack)?,
                ))
            })
            .collect::<Result<Map<_, _>>>()
            .map(Value::Object),
    }
}

fn lookup_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() || path == "$" {
        return Some(root);
    }
    let mut value = root;
    for component in path.split('.') {
        if component.is_empty() {
            return None;
        }
        value = value.get(component)?;
    }
    Some(value)
}

pub fn value_to_text(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Null => Ok("null".to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Array(_) | Value::Object(_) => {
            bail!("cannot interpolate an array or object into a string")
        }
    }
}

fn find_unescaped_open(source: &str, from: usize) -> Option<usize> {
    let mut cursor = from;
    while let Some(relative) = source[cursor..].find("{{") {
        let position = cursor + relative;
        let mut backslashes = 0;
        let mut preceding = position;
        while preceding > 0 && source.as_bytes()[preceding - 1] == b'\\' {
            backslashes += 1;
            preceding -= 1;
        }
        if backslashes % 2 == 0 {
            return Some(position);
        }
        cursor = position + 2;
    }
    None
}

fn find_closing_delimiter(source: &str, open: usize) -> Option<usize> {
    let mut cursor = open + 2;
    let mut nested = 0;
    let mut quote = None;
    let mut escaped = false;
    while cursor < source.len() {
        let remaining = &source[cursor..];
        let character = remaining.chars().next()?;
        let width = character.len_utf8();
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' && active_quote == '"' {
                escaped = true;
            } else if character == active_quote {
                quote = None;
            }
            cursor += width;
            continue;
        }
        if character == '"' || character == '\'' {
            quote = Some(character);
            cursor += width;
            continue;
        }
        if remaining.starts_with("{{") {
            nested += 1;
            cursor += 2;
        } else if remaining.starts_with("}}") {
            if nested == 0 {
                return Some(cursor);
            }
            nested -= 1;
            cursor += 2;
        } else {
            cursor += width;
        }
    }
    None
}

struct Parser<'a> {
    source: &'a str,
    position: usize,
}

fn parse_expression(source: &str) -> Result<Expr> {
    let mut parser = Parser {
        source,
        position: 0,
    };
    let expression = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.position != source.len() {
        bail!(
            "unexpected expression input at byte {}: {:?}",
            parser.position,
            &source[parser.position..]
        );
    }
    Ok(expression)
}

impl Parser<'_> {
    fn parse_value(&mut self) -> Result<Expr> {
        self.skip_whitespace();
        if self.starts_with("{{") {
            return self.parse_nested_expression();
        }
        match self.peek_character() {
            Some('"') | Some('\'') => self
                .parse_string()
                .map(|value| Expr::Literal(Value::String(value))),
            Some('[') => self.parse_array(),
            Some('{') => self.parse_object(),
            Some(character) if character == '-' || character.is_ascii_digit() => {
                self.parse_number().map(Expr::Literal)
            }
            Some(character) if is_identifier_start(character) => self.parse_identifier_value(),
            Some(character) => bail!("unexpected expression character {:?}", character),
            None => bail!("expression is empty"),
        }
    }

    fn parse_identifier_value(&mut self) -> Result<Expr> {
        let name = self.parse_identifier()?;
        self.skip_whitespace();
        if self.consume_character('(') {
            return self.parse_call(name);
        }
        if self.consume_character(':') {
            let path = self.parse_path()?;
            return Ok(Expr::Ref {
                namespace: name,
                path,
            });
        }
        match name.as_str() {
            "true" => Ok(Expr::Literal(Value::Bool(true))),
            "false" => Ok(Expr::Literal(Value::Bool(false))),
            "null" => Ok(Expr::Literal(Value::Null)),
            _ => bail!("bare identifier {:?} is not a reference or method", name),
        }
    }

    fn parse_call(&mut self, name: String) -> Result<Expr> {
        let mut args = Vec::new();
        let mut named_args = BTreeMap::new();
        self.skip_whitespace();
        if self.consume_character(')') {
            return Ok(Expr::Call {
                name,
                args,
                named_args,
            });
        }
        loop {
            self.skip_whitespace();
            let save = self.position;
            let named = if self.peek_character().is_some_and(is_identifier_start) {
                let identifier = self.parse_identifier()?;
                self.skip_whitespace();
                if self.consume_character('=') {
                    Some(identifier)
                } else {
                    self.position = save;
                    None
                }
            } else {
                None
            };
            let value = self.parse_value()?;
            if let Some(identifier) = named {
                if named_args.insert(identifier.clone(), value).is_some() {
                    bail!("duplicate named argument {:?}", identifier);
                }
            } else {
                args.push(value);
            }
            self.skip_whitespace();
            if self.consume_character(')') {
                break;
            }
            if !self.consume_character(',') {
                bail!("expected ',' or ')' in method call {:?}", name);
            }
        }
        Ok(Expr::Call {
            name,
            args,
            named_args,
        })
    }

    fn parse_array(&mut self) -> Result<Expr> {
        self.expect_character('[')?;
        let mut values = Vec::new();
        self.skip_whitespace();
        if self.consume_character(']') {
            return Ok(Expr::Array(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_whitespace();
            if self.consume_character(']') {
                break;
            }
            if !self.consume_character(',') {
                bail!("expected ',' or ']' in array expression");
            }
        }
        Ok(Expr::Array(values))
    }

    fn parse_object(&mut self) -> Result<Expr> {
        self.expect_character('{')?;
        let mut values = BTreeMap::new();
        self.skip_whitespace();
        if self.consume_character('}') {
            return Ok(Expr::Object(values));
        }
        loop {
            self.skip_whitespace();
            let key = match self.peek_character() {
                Some('"') | Some('\'') => self.parse_string()?,
                Some(character) if is_identifier_start(character) => self.parse_identifier()?,
                _ => bail!(
                    "object expression keys must be identifiers or strings at byte {} near {:?}",
                    self.position,
                    &self.source[self.position..]
                ),
            };
            self.skip_whitespace();
            self.expect_character('=')?;
            let value = self.parse_value()?;
            if values.insert(key.clone(), value).is_some() {
                bail!("duplicate object expression key {:?}", key);
            }
            self.skip_whitespace();
            if self.consume_character('}') {
                break;
            }
            if !self.consume_character(',') {
                bail!("expected ',' or '}}' in object expression");
            }
        }
        Ok(Expr::Object(values))
    }

    fn parse_nested_expression(&mut self) -> Result<Expr> {
        let open = self.position;
        let close = find_closing_delimiter(self.source, open)
            .ok_or_else(|| anyhow::anyhow!("unclosed nested expression at byte {open}"))?;
        let expression = parse_expression(&self.source[open + 2..close])?;
        self.position = close + 2;
        Ok(expression)
    }

    fn parse_string(&mut self) -> Result<String> {
        let quote = self
            .consume_any_character()
            .ok_or_else(|| anyhow::anyhow!("expected string"))?;
        let mut value = String::new();
        loop {
            let character = self
                .consume_any_character()
                .ok_or_else(|| anyhow::anyhow!("unterminated string expression"))?;
            if character == quote {
                return Ok(value);
            }
            if character == '\\' && quote == '"' {
                let escaped = self
                    .consume_any_character()
                    .ok_or_else(|| anyhow::anyhow!("unterminated string escape"))?;
                let decoded = match escaped {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    '"' => '"',
                    '\\' => '\\',
                    other => bail!("unsupported string escape \\{other}"),
                };
                value.push(decoded);
            } else {
                value.push(character);
            }
        }
    }

    fn parse_number(&mut self) -> Result<Value> {
        let start = self.position;
        while let Some(character) = self.peek_character() {
            if character.is_ascii_digit() || matches!(character, '-' | '+' | '.' | 'e' | 'E') {
                self.position += character.len_utf8();
            } else {
                break;
            }
        }
        let token = &self.source[start..self.position];
        if token.contains('.') || token.contains('e') || token.contains('E') {
            let value = token
                .parse::<f64>()
                .map_err(|_| anyhow::anyhow!("invalid numeric literal {:?}", token))?;
            let number = Number::from_f64(value)
                .ok_or_else(|| anyhow::anyhow!("invalid numeric literal {:?}", token))?;
            Ok(Value::Number(number))
        } else if let Ok(value) = token.parse::<i64>() {
            Ok(Value::Number(Number::from(value)))
        } else if let Ok(value) = token.parse::<u64>() {
            Ok(Value::Number(Number::from(value)))
        } else {
            bail!("invalid numeric literal {:?}", token)
        }
    }

    fn parse_identifier(&mut self) -> Result<String> {
        let start = self.position;
        let first = self
            .peek_character()
            .ok_or_else(|| anyhow::anyhow!("expected identifier"))?;
        if !is_identifier_start(first) {
            bail!("expected identifier at byte {}", self.position);
        }
        self.position += first.len_utf8();
        while let Some(character) = self.peek_character() {
            if is_identifier_continue(character) {
                self.position += character.len_utf8();
            } else {
                break;
            }
        }
        Ok(self.source[start..self.position].to_string())
    }

    fn parse_path(&mut self) -> Result<String> {
        let start = self.position;
        while let Some(character) = self.peek_character() {
            if character.is_whitespace() || matches!(character, ',' | ')' | ']' | '}' | '=') {
                break;
            }
            self.position += character.len_utf8();
        }
        Ok(self.source[start..self.position].to_string())
    }

    fn skip_whitespace(&mut self) {
        while self.peek_character().is_some_and(char::is_whitespace) {
            let width = self.peek_character().expect("character exists").len_utf8();
            self.position += width;
        }
    }

    fn starts_with(&self, value: &str) -> bool {
        self.source[self.position..].starts_with(value)
    }

    fn peek_character(&self) -> Option<char> {
        self.source[self.position..].chars().next()
    }

    fn consume_any_character(&mut self) -> Option<char> {
        let character = self.peek_character()?;
        self.position += character.len_utf8();
        Some(character)
    }

    fn consume_character(&mut self, expected: char) -> bool {
        if self.peek_character() == Some(expected) {
            self.position += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn expect_character(&mut self, expected: char) -> Result<()> {
        if self.consume_character(expected) {
            Ok(())
        } else {
            bail!("expected {:?} at byte {}", expected, self.position)
        }
    }
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character.is_ascii_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || character.is_ascii_digit() || character == '-'
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct TestMethods;

    impl MethodResolver for TestMethods {
        fn call_method(
            &mut self,
            name: &str,
            args: Vec<Value>,
            _named_args: BTreeMap<String, Value>,
        ) -> Result<Value> {
            match name {
                "echo" => Ok(args.into_iter().next().unwrap_or(Value::Null)),
                _ => bail!("unknown test method {name:?}"),
            }
        }
    }

    fn context<'a>(
        config: &'a Value,
        runtime: &'a Value,
        methods: &'a mut TestMethods,
    ) -> EvalContext<'a> {
        let empty = Box::leak(Box::new(Value::Null));
        let references = Box::leak(Box::new(TreeReferences {
            config,
            this: empty,
            runtime,
            input: empty,
            request: None,
            returned: None,
        }));
        EvalContext {
            references,
            methods,
        }
    }

    #[test]
    fn parses_references_and_calls() {
        let template = Template::parse("{{ echo(runtime:provider_name) }}").unwrap();
        assert_eq!(
            &template.parts,
            &[TemplatePart::Expr(Expr::Call {
                name: "echo".to_string(),
                args: vec![Expr::Ref {
                    namespace: "runtime".to_string(),
                    path: "provider_name".to_string(),
                }],
                named_args: BTreeMap::new(),
            })]
        );
    }

    #[test]
    fn nested_placeholder_is_syntactic_sugar_for_an_expression_argument() {
        let direct = Template::parse("{{ echo(runtime:provider_name) }}").unwrap();
        let nested = Template::parse("{{ echo({{ runtime:provider_name }}) }}").unwrap();
        assert_eq!(direct, nested);
    }

    #[test]
    fn exact_placeholder_preserves_value_type() {
        let config = serde_json::json!({"command": {"script": "open.sh"}});
        let runtime = Value::Null;
        let mut methods = TestMethods;
        let mut evaluation = context(&config, &runtime, &mut methods);
        assert_eq!(
            Template::parse("{{ config:command }}")
                .unwrap()
                .evaluate_value(&mut evaluation)
                .unwrap(),
            serde_json::json!({"script": "open.sh"})
        );
    }

    #[test]
    fn this_references_resolve_the_explicit_view_instance() {
        let config = Value::Null;
        let this = serde_json::json!({"query": {"text": "hello"}});
        let runtime = Value::Null;
        let input = Value::Null;
        let references = TreeReferences {
            config: &config,
            this: &this,
            runtime: &runtime,
            input: &input,
            request: None,
            returned: None,
        };
        let mut methods = TestMethods;
        let mut evaluation = EvalContext {
            references: &references,
            methods: &mut methods,
        };
        assert_eq!(
            Template::parse("{{ this:query.text }}")
                .unwrap()
                .evaluate_value(&mut evaluation)
                .unwrap(),
            Value::String("hello".to_string())
        );
    }

    #[test]
    fn input_references_support_values_and_whole_roots() {
        let config = Value::Null;
        let runtime = Value::Null;
        let input = serde_json::json!({"stdin": {"path": "/tmp/input"}});
        let references = TreeReferences {
            config: &config,
            this: &Value::Null,
            runtime: &runtime,
            input: &input,
            request: None,
            returned: None,
        };
        let mut methods = TestMethods;
        let mut evaluation = EvalContext {
            references: &references,
            methods: &mut methods,
        };

        assert_eq!(
            Template::parse("{{ input: }}")
                .unwrap()
                .evaluate_value(&mut evaluation)
                .unwrap(),
            input
        );
    }

    #[test]
    fn mixed_templates_only_accept_scalar_values() {
        let config = Value::Null;
        let runtime = serde_json::json!({"selected": "Firefox"});
        let mut methods = TestMethods;
        let mut evaluation = context(&config, &runtime, &mut methods);
        assert_eq!(
            Template::parse("run {{ runtime:selected }}")
                .unwrap()
                .evaluate_text(&mut evaluation)
                .unwrap(),
            "run Firefox"
        );

        let runtime = serde_json::json!({"selected": {"label": "Firefox"}});
        let mut methods = TestMethods;
        let mut evaluation = context(&config, &runtime, &mut methods);
        let error = Template::parse("run {{ runtime:selected }}")
            .unwrap()
            .evaluate_text(&mut evaluation)
            .expect_err("objects cannot be interpolated into text");
        assert!(error.to_string().contains("cannot interpolate"));
    }

    #[test]
    fn recursive_config_references_are_evaluated() {
        let config = serde_json::json!({
            "first": "{{ config:second }}",
            "second": "ready"
        });
        let runtime = Value::Null;
        let mut methods = TestMethods;
        let mut evaluation = context(&config, &runtime, &mut methods);
        assert_eq!(
            Template::parse("{{ config:first }}")
                .unwrap()
                .evaluate_value(&mut evaluation)
                .unwrap(),
            Value::String("ready".to_string())
        );
    }

    #[test]
    fn recursive_config_references_detect_cycles() {
        let config = serde_json::json!({
            "first": "{{ config:second }}",
            "second": "{{ config:first }}"
        });
        let runtime = Value::Null;
        let mut methods = TestMethods;
        let mut evaluation = context(&config, &runtime, &mut methods);
        let error = Template::parse("{{ config:first }}")
            .unwrap()
            .evaluate_value(&mut evaluation)
            .expect_err("cyclic references must fail");
        assert!(error.to_string().contains("cyclic"));
    }

    #[test]
    fn calls_receive_evaluated_arguments() {
        let config = Value::Null;
        let runtime = serde_json::json!({"provider_name": "aa"});
        let mut methods = TestMethods;
        let mut evaluation = context(&config, &runtime, &mut methods);
        assert_eq!(
            Template::parse("{{ echo(runtime:provider_name) }}")
                .unwrap()
                .evaluate_value(&mut evaluation)
                .unwrap(),
            Value::String("aa".to_string())
        );
    }

    #[test]
    fn evaluate_json_tree_preserves_plain_values_and_resolves_templates() {
        let config = serde_json::json!({"script": "open.sh"});
        let runtime = serde_json::json!({"selected": "Firefox"});
        let mut methods = TestMethods;
        let mut evaluation = context(&config, &runtime, &mut methods);
        let tree = serde_json::json!({
            "run": "{{ config:script }} --item {{ runtime:selected }}",
            "literal": 1
        });
        assert_eq!(
            evaluate_json_value(&tree, &mut evaluation).unwrap(),
            serde_json::json!({"run": "open.sh --item Firefox", "literal": 1})
        );
    }
}
