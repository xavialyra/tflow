use crate::lifecycle::CancellationToken;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};

const MAX_TEMPLATE_BYTES: usize = 1024 * 1024;
const MAX_EXPRESSION_BYTES: usize = 256 * 1024;
const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 1_000_000;
const MAX_PATH_SEGMENTS: usize = 128;
const MAX_COLLECTION_ELEMENTS: usize = 100_000;
const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;
const MAX_COMPILED_TEMPLATES: usize = 4096;
const MAX_IDENTIFIER_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Namespace {
    View,
    Page,
    Selection,
    Input,
    Result,
    Session,
}

impl Namespace {
    const ALL: [Self; 6] = [
        Self::Input,
        Self::Page,
        Self::Selection,
        Self::Session,
        Self::View,
        Self::Result,
    ];

    pub(crate) fn parse(source: &str) -> Option<Self> {
        match source {
            "view" => Some(Self::View),
            "page" => Some(Self::Page),
            "selection" => Some(Self::Selection),
            "input" => Some(Self::Input),
            "result" => Some(Self::Result),
            "session" => Some(Self::Session),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Page => "page",
            Self::Selection => "selection",
            Self::Input => "input",
            Self::Result => "result",
            Self::Session => "session",
        }
    }
}

/// The earliest lifecycle stage at which a configuration consumer can resolve
/// a value. Later stages inherit the capabilities of earlier stages.
///
/// This is a static contract for a consumer, not a second evaluator. The
/// runtime snapshot remains responsible for supplying the actual scope data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvaluationStage {
    /// Configuration topology is being bound; no dynamic scope exists yet.
    Bootstrap,
    /// Immutable invocation input has been captured.
    Invocation,
    /// A View and active session operation have been captured.
    Operation,
    /// A return continuation or result handler additionally has `result`.
    Return,
}

impl EvaluationStage {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::Invocation => "invocation",
            Self::Operation => "operation",
            Self::Return => "return",
        }
    }

    pub(crate) const fn allows(self, namespace: Namespace) -> bool {
        match self {
            Self::Bootstrap => false,
            Self::Invocation => matches!(namespace, Namespace::Input),
            Self::Operation => {
                Self::Invocation.allows(namespace)
                    || matches!(
                        namespace,
                        Namespace::Page
                            | Namespace::Selection
                            | Namespace::Session
                            | Namespace::View
                    )
            }
            Self::Return => {
                Self::Operation.allows(namespace) || matches!(namespace, Namespace::Result)
            }
        }
    }

    fn available_names(self) -> String {
        let names = Namespace::ALL
            .into_iter()
            .filter(|namespace| self.allows(*namespace))
            .map(Namespace::name)
            .collect::<Vec<_>>();
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    }
}

const VIEW_FIELDS: &[&str] = &["ref", "query", "input", "raw_input", "state_revision"];
const PAGE_FIELDS: &[&str] = &[
    "ref",
    "state_revision",
    "input",
    "raw_input",
    "query",
    "items",
    "selected_item",
    "command_owner",
    "commands",
];
const SESSION_FIELDS: &[&str] = &["input", "views"];

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathSegment {
    Member(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Path {
    source: String,
    segments: Vec<PathSegment>,
}

#[derive(Debug, Clone)]
pub enum TemplatePart {
    Text(String),
    Expression(Path),
}

#[derive(Debug, Clone)]
pub struct Template {
    parts: Vec<TemplatePart>,
}

#[derive(Debug, Clone)]
enum RequiredFields {
    All,
    Fields(HashSet<String>),
}

#[derive(Debug, Default)]
pub struct ContextRequirements {
    namespaces: HashMap<Namespace, RequiredFields>,
}

impl ContextRequirements {
    fn add_path(&mut self, path: &Path) {
        let PathSegment::Member(name) = &path.segments[0] else {
            return;
        };
        let namespace =
            Namespace::parse(name).expect("compiled dynamic path has an unsupported namespace");
        let field = match path.segments.get(1) {
            Some(PathSegment::Member(field)) => Some(field),
            Some(PathSegment::Index(_)) | None => None,
        };
        match (self.namespaces.get_mut(&namespace), field) {
            (Some(RequiredFields::All), _) => {}
            (Some(RequiredFields::Fields(fields)), Some(field)) => {
                fields.insert(field.clone());
            }
            (Some(required), None) => *required = RequiredFields::All,
            (None, Some(field)) => {
                self.namespaces.insert(
                    namespace,
                    RequiredFields::Fields(HashSet::from([field.clone()])),
                );
            }
            (None, None) => {
                self.namespaces.insert(namespace, RequiredFields::All);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.namespaces.is_empty()
    }

    pub(crate) fn requires(&self, namespace: Namespace) -> bool {
        self.namespaces.contains_key(&namespace)
    }

    pub(crate) fn requires_field(&self, namespace: Namespace, field: &str) -> bool {
        match self.namespaces.get(&namespace) {
            Some(RequiredFields::All) => true,
            Some(RequiredFields::Fields(fields)) => fields.contains(field),
            None => false,
        }
    }

    /// Reject a template whose required scopes do not exist when `consumer`
    /// runs. This is deliberately namespace-based: schema validation of a
    /// namespace's contents remains separate from scope availability.
    pub(crate) fn validate_stage(&self, stage: EvaluationStage, consumer: &str) -> Result<()> {
        let unavailable = Namespace::ALL
            .into_iter()
            .filter(|namespace| self.requires(*namespace) && !stage.allows(*namespace))
            .map(Namespace::name)
            .collect::<Vec<_>>();
        if unavailable.is_empty() {
            return Ok(());
        }
        bail!(
            "{consumer} is consumed during the {} evaluation stage and cannot reference dynamic namespace{} {}; available namespaces: {}",
            stage.name(),
            if unavailable.len() == 1 { "" } else { "s" },
            unavailable
                .iter()
                .map(|namespace| format!("{namespace:?}"))
                .collect::<Vec<_>>()
                .join(", "),
            stage.available_names(),
        );
    }
}

#[derive(Debug, Default, Clone)]
pub struct TemplateRegistry {
    templates: HashMap<String, Template>,
}

impl TemplateRegistry {
    pub fn compile_json_tree(value: &Value) -> Result<Self> {
        let mut registry = Self::default();
        registry.compile_value(value, 0)?;
        Ok(registry)
    }

    fn compile_value(&mut self, value: &Value, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            bail!("dynamic value exceeded maximum depth of {MAX_DEPTH}");
        }
        match value {
            Value::String(source) if is_dynamic_string(source) => {
                let template = parse_template(source)?;
                if self.templates.len() >= MAX_COMPILED_TEMPLATES
                    && !self.templates.contains_key(source)
                {
                    bail!(
                        "dynamic configuration exceeded maximum compiled template count of {MAX_COMPILED_TEMPLATES}"
                    );
                }
                self.templates.entry(source.clone()).or_insert(template);
            }
            Value::Array(values) => {
                for value in values {
                    self.compile_value(value, depth + 1)?;
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    self.compile_value(value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn get(&self, source: &str) -> Result<&Template> {
        self.templates.get(source).ok_or_else(|| {
            anyhow::anyhow!(
                "dynamic template was not compiled during configuration validation: {source:?}"
            )
        })
    }

    pub fn requirements_for_value(&self, value: &Value) -> Result<ContextRequirements> {
        let mut requirements = ContextRequirements::default();
        self.collect_requirements(value, &mut requirements)?;
        Ok(requirements)
    }

    fn collect_requirements(
        &self,
        value: &Value,
        requirements: &mut ContextRequirements,
    ) -> Result<()> {
        match value {
            Value::String(source) if is_dynamic_string(source) => {
                for part in &self.get(source)?.parts {
                    if let TemplatePart::Expression(path) = part {
                        requirements.add_path(path);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.collect_requirements(value, requirements)?;
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    self.collect_requirements(value, requirements)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn is_dynamic_string(source: &str) -> bool {
    find_unescaped_open(source, 0).is_some()
}

pub struct EvalContext<'a> {
    pub root: &'a Value,
    pub cancellation: Option<&'a CancellationToken>,
    pub templates: Option<&'a TemplateRegistry>,
}

#[derive(Debug, Default)]
pub(crate) struct Budget {
    nodes: usize,
    output_bytes: usize,
}

impl Budget {
    fn visit(&mut self, depth: usize, cancellation: Option<&CancellationToken>) -> Result<()> {
        check_cancelled(cancellation)?;
        if depth > MAX_DEPTH {
            bail!("dynamic value exceeded maximum depth of {MAX_DEPTH}");
        }
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("dynamic value node budget overflow"))?;
        if self.nodes > MAX_NODES {
            bail!("dynamic value exceeded maximum node count of {MAX_NODES}");
        }
        Ok(())
    }

    fn path_segment(&mut self, cancellation: Option<&CancellationToken>) -> Result<()> {
        check_cancelled(cancellation)?;
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("dynamic path budget overflow"))?;
        if self.nodes > MAX_NODES {
            bail!("dynamic value exceeded maximum node count of {MAX_NODES}");
        }
        Ok(())
    }

    fn output(&mut self, bytes: usize, cancellation: Option<&CancellationToken>) -> Result<()> {
        check_cancelled(cancellation)?;
        self.output_bytes = self
            .output_bytes
            .checked_add(bytes)
            .ok_or_else(|| anyhow::anyhow!("dynamic output budget overflow"))?;
        if self.output_bytes > MAX_RESULT_BYTES {
            bail!("dynamic value exceeded maximum output size of {MAX_RESULT_BYTES} bytes");
        }
        Ok(())
    }

    fn serialized<T: Serialize>(
        &mut self,
        value: &T,
        cancellation: Option<&CancellationToken>,
    ) -> Result<()> {
        let (failure, result) = {
            let mut writer = BudgetWriter {
                budget: self,
                cancellation,
                failure: None,
            };
            let result = serde_json::to_writer(&mut writer, value);
            (writer.failure, result)
        };
        match failure {
            Some(BudgetWriterFailure::Cancelled) => {
                bail!("dynamic value evaluation cancelled")
            }
            Some(BudgetWriterFailure::OutputLimit) => {
                bail!("dynamic value exceeded maximum output size of {MAX_RESULT_BYTES} bytes")
            }
            None => result
                .map_err(|error| anyhow::anyhow!("could not serialize dynamic value: {error}")),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum BudgetWriterFailure {
    Cancelled,
    OutputLimit,
}

struct BudgetWriter<'a> {
    budget: &'a mut Budget,
    cancellation: Option<&'a CancellationToken>,
    failure: Option<BudgetWriterFailure>,
}

impl Write for BudgetWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failure.is_some() {
            return Err(io::Error::other("dynamic output budget exhausted"));
        }
        if self
            .cancellation
            .is_some_and(CancellationToken::is_cancelled)
        {
            self.failure = Some(BudgetWriterFailure::Cancelled);
            return Err(io::Error::other("dynamic value evaluation cancelled"));
        }
        let Some(total) = self.budget.output_bytes.checked_add(bytes.len()) else {
            self.failure = Some(BudgetWriterFailure::OutputLimit);
            return Err(io::Error::other("dynamic output budget overflow"));
        };
        if total > MAX_RESULT_BYTES {
            self.failure = Some(BudgetWriterFailure::OutputLimit);
            return Err(io::Error::other("dynamic value output limit exceeded"));
        }
        self.budget.output_bytes = total;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn check_cancelled(cancellation: Option<&CancellationToken>) -> Result<()> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        bail!("dynamic value evaluation cancelled");
    }
    Ok(())
}

impl Template {
    pub fn parse(source: &str) -> Result<Self> {
        parse_template(source)
    }

    pub fn is_complete_path(&self) -> bool {
        matches!(self.parts.as_slice(), [TemplatePart::Expression(_)])
    }

    #[cfg(test)]
    pub fn evaluate_value(&self, context: &EvalContext<'_>) -> Result<Value> {
        let mut budget = Budget::default();
        self.evaluate(&mut budget, context, 0)
    }

    fn evaluate(
        &self,
        budget: &mut Budget,
        context: &EvalContext<'_>,
        depth: usize,
    ) -> Result<Value> {
        budget.visit(depth, context.cancellation)?;
        if let [TemplatePart::Expression(path)] = self.parts.as_slice() {
            return evaluate_path(
                path,
                context.root,
                budget,
                context.cancellation,
                depth + 1,
                true,
            );
        }
        let mut text = String::new();
        for part in &self.parts {
            check_cancelled(context.cancellation)?;
            match part {
                TemplatePart::Text(value) => {
                    append_text(&mut text, value, context.cancellation)?;
                }
                TemplatePart::Expression(path) => {
                    let value = evaluate_path(
                        path,
                        context.root,
                        budget,
                        context.cancellation,
                        depth + 1,
                        false,
                    )?;
                    append_value_to_text(&mut text, &value, context.cancellation)?;
                }
            }
        }
        budget.serialized(&text, context.cancellation)?;
        Ok(Value::String(text))
    }
}

fn compiled_template<'a>(source: &str, context: &'a EvalContext<'_>) -> Result<&'a Template> {
    context
        .templates
        .and_then(|registry| registry.get(source).ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "dynamic template was not compiled during configuration validation: {source:?}"
            )
        })
}

fn parse_template(source: &str) -> Result<Template> {
    if source.len() > MAX_TEMPLATE_BYTES {
        bail!("dynamic template exceeded maximum size of {MAX_TEMPLATE_BYTES} bytes");
    }
    let mut parts = Vec::new();
    let mut cursor = 0;
    while let Some(open) = find_unescaped_open(source, cursor) {
        if open > cursor {
            parts.push(TemplatePart::Text(source[cursor..open].to_string()));
        }
        let close = find_closing_delimiter(source, open)
            .ok_or_else(|| anyhow::anyhow!("unclosed dynamic template at byte {open}"))?;
        let expression = &source[open + 2..close];
        if expression.len() > MAX_EXPRESSION_BYTES {
            bail!("dynamic path exceeded maximum size of {MAX_EXPRESSION_BYTES} bytes");
        }
        parts.push(TemplatePart::Expression(parse_path(expression)?));
        cursor = close + 2;
    }
    if cursor < source.len() {
        parts.push(TemplatePart::Text(source[cursor..].to_string()));
    }
    if parts.is_empty() {
        parts.push(TemplatePart::Text(String::new()));
    }
    Ok(Template { parts })
}

fn parse_path(source: &str) -> Result<Path> {
    let bytes = source.as_bytes();
    let mut parser = PathParser {
        source,
        bytes,
        position: 0,
    };
    parser.whitespace();
    let root_start = parser.position;
    let root = parser.identifier()?;
    if Namespace::parse(&root).is_none() {
        bail!("unknown dynamic namespace {root:?} at byte {root_start}");
    }
    let mut segments = vec![PathSegment::Member(root)];
    parser.whitespace();
    while parser.position < bytes.len() {
        if segments.len() >= MAX_PATH_SEGMENTS {
            bail!(
                "dynamic path exceeded maximum segment count of {MAX_PATH_SEGMENTS} at byte {}",
                parser.position
            );
        }
        match parser.peek() {
            Some(b'.') => {
                parser.position += 1;
                let member = parser.identifier()?;
                segments.push(PathSegment::Member(member));
            }
            Some(b'[') => {
                parser.position += 1;
                parser.whitespace();
                let segment = if parser.peek() == Some(b'\"') {
                    let key = parser.quoted_key()?;
                    PathSegment::Member(key)
                } else {
                    let index = parser.index()?;
                    PathSegment::Index(index)
                };
                parser.whitespace();
                if parser.peek() != Some(b']') {
                    bail!(
                        "expected closing ] for path segment at byte {}",
                        parser.position
                    );
                }
                parser.position += 1;
                segments.push(segment);
            }
            Some(_) => bail!("unexpected path character at byte {}", parser.position),
            None => break,
        }
        parser.whitespace();
    }
    let path = Path {
        source: source.trim().to_string(),
        segments,
    };
    validate_context_schema(&path)?;
    Ok(path)
}

struct PathParser<'a> {
    source: &'a str,
    bytes: &'a [u8],
    position: usize,
}

impl PathParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }
    fn whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.position += 1;
        }
    }
    fn identifier(&mut self) -> Result<String> {
        let start = self.position;
        if !self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        {
            bail!("expected path member at byte {start}");
        }
        self.position += 1;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            self.position += 1;
        }
        if self.position - start > MAX_IDENTIFIER_BYTES {
            bail!("path member exceeded maximum size at byte {start}");
        }
        Ok(self.source[start..self.position].to_string())
    }
    fn quoted_key(&mut self) -> Result<String> {
        let start = self.position;
        self.position += 1;
        let mut escaped = false;
        while let Some(byte) = self.peek() {
            self.position += 1;
            if escaped {
                escaped = false;
                continue;
            }
            match byte {
                b'\\' => escaped = true,
                b'\"' => {
                    return serde_json::from_str(&self.source[start..self.position]).map_err(
                        |error| anyhow::anyhow!("invalid quoted path key at byte {start}: {error}"),
                    );
                }
                byte if byte.is_ascii_control() => bail!(
                    "control character in quoted path key at byte {}",
                    self.position - 1
                ),
                _ => {}
            }
        }
        bail!("unterminated quoted path key at byte {start}")
    }
    fn index(&mut self) -> Result<usize> {
        let start = self.position;
        let begin = self.position;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.position += 1;
        }
        if self.position == begin {
            bail!("expected a non-negative array index at byte {start}");
        }
        self.source[begin..self.position]
            .parse()
            .map_err(|_| anyhow::anyhow!("array index is too large at byte {start}"))
    }
}

fn validate_context_schema(path: &Path) -> Result<()> {
    let PathSegment::Member(namespace) = &path.segments[0] else {
        return Ok(());
    };
    let Some(PathSegment::Member(field)) = path.segments.get(1) else {
        return Ok(());
    };
    let fields = match Namespace::parse(namespace) {
        Some(Namespace::View) => VIEW_FIELDS,
        Some(Namespace::Page) => PAGE_FIELDS,
        Some(Namespace::Session) => SESSION_FIELDS,
        Some(Namespace::Selection | Namespace::Input | Namespace::Result) => return Ok(()),
        None => return Ok(()),
    };
    if !fields.contains(&field.as_str()) {
        bail!(
            "unknown {namespace} field {field:?}; expected one of {}",
            fields.join(", ")
        );
    }
    Ok(())
}

fn evaluate_path(
    path: &Path,
    root: &Value,
    budget: &mut Budget,
    cancellation: Option<&CancellationToken>,
    depth: usize,
    account_output: bool,
) -> Result<Value> {
    let mut value = root;
    for (index, segment) in path.segments.iter().enumerate() {
        budget.path_segment(cancellation)?;
        value = match segment {
            PathSegment::Member(key) => {
                let object = value.as_object().ok_or_else(|| {
                    anyhow::anyhow!(
                        "cannot read member {key:?} from {} while traversing {} at segment {}",
                        value_type(value),
                        path.source,
                        index + 1
                    )
                })?;
                object.get(key).ok_or_else(|| {
                    anyhow::anyhow!(
                        "missing member {key:?} while traversing {} at segment {}",
                        path.source,
                        index + 1
                    )
                })?
            }
            PathSegment::Index(array_index) => {
                let array = value.as_array().ok_or_else(|| {
                    anyhow::anyhow!(
                        "cannot index {} while traversing {} at segment {}",
                        value_type(value),
                        path.source,
                        index + 1
                    )
                })?;
                array.get(*array_index).ok_or_else(|| {
                    anyhow::anyhow!(
                        "array index {array_index} is out of bounds while traversing {} at segment {}",
                        path.source,
                        index + 1
                    )
                })?
            }
        };
    }
    check_cancelled(cancellation)?;
    clone_value(value, budget, cancellation, depth, account_output)
}

fn clone_value(
    value: &Value,
    budget: &mut Budget,
    cancellation: Option<&CancellationToken>,
    depth: usize,
    account_output: bool,
) -> Result<Value> {
    budget.visit(depth, cancellation)?;
    match value {
        Value::Array(values) => {
            if values.len() > MAX_COLLECTION_ELEMENTS {
                bail!(
                    "dynamic value collection exceeded maximum element count of {MAX_COLLECTION_ELEMENTS}"
                );
            }
            if account_output {
                budget.output(2 + values.len().saturating_sub(1), cancellation)?;
            }
            values
                .iter()
                .map(|value| clone_value(value, budget, cancellation, depth + 1, account_output))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array)
        }
        Value::Object(values) => {
            if values.len() > MAX_COLLECTION_ELEMENTS {
                bail!(
                    "dynamic value collection exceeded maximum element count of {MAX_COLLECTION_ELEMENTS}"
                );
            }
            if account_output {
                budget.output(2 + values.len().saturating_sub(1), cancellation)?;
            }
            values
                .iter()
                .map(|(key, value)| {
                    if account_output {
                        budget.serialized(key, cancellation)?;
                        budget.output(1, cancellation)?;
                    }
                    Ok((
                        key.clone(),
                        clone_value(value, budget, cancellation, depth + 1, account_output)?,
                    ))
                })
                .collect::<Result<serde_json::Map<_, _>>>()
                .map(Value::Object)
        }
        value => {
            if account_output {
                budget.serialized(value, cancellation)?;
            }
            Ok(value.clone())
        }
    }
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub fn validate_json_value(value: &Value) -> Result<()> {
    let mut budget = Budget::default();
    validate_json_value_with_budget(value, &mut budget, 0)
}

fn validate_json_value_with_budget(value: &Value, budget: &mut Budget, depth: usize) -> Result<()> {
    budget.visit(depth, None)?;
    match value {
        Value::String(source) if is_dynamic_string(source) => {
            Template::parse(source)?;
        }
        Value::Array(values) => {
            if values.len() > MAX_COLLECTION_ELEMENTS {
                bail!(
                    "dynamic value collection exceeded maximum element count of {MAX_COLLECTION_ELEMENTS}"
                );
            }
            for value in values {
                validate_json_value_with_budget(value, budget, depth + 1)?;
            }
        }
        Value::Object(values) => {
            if values.len() > MAX_COLLECTION_ELEMENTS {
                bail!(
                    "dynamic value collection exceeded maximum element count of {MAX_COLLECTION_ELEMENTS}"
                );
            }
            for value in values.values() {
                validate_json_value_with_budget(value, budget, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
pub fn evaluate_json_value(value: &Value, context: &EvalContext<'_>) -> Result<Value> {
    let mut budget = Budget::default();
    evaluate_json_value_with_budget_at_depth(value, context, &mut budget, 0)
}

pub(crate) fn evaluate_json_value_with_budget(
    value: &Value,
    context: &EvalContext<'_>,
    budget: &mut Budget,
) -> Result<Value> {
    evaluate_json_value_with_budget_at_depth(value, context, budget, 0)
}

pub(crate) fn clone_json_value_with_budget(
    value: &Value,
    cancellation: Option<&CancellationToken>,
    budget: &mut Budget,
) -> Result<Value> {
    clone_value(value, budget, cancellation, 0, true)
}

fn evaluate_json_value_with_budget_at_depth(
    value: &Value,
    context: &EvalContext<'_>,
    budget: &mut Budget,
    depth: usize,
) -> Result<Value> {
    budget.visit(depth, context.cancellation)?;
    match value {
        Value::String(source) if is_dynamic_string(source) => {
            compiled_template(source, context)?.evaluate(budget, context, depth + 1)
        }
        Value::Array(values) => {
            if values.len() > MAX_COLLECTION_ELEMENTS {
                bail!(
                    "dynamic value collection exceeded maximum element count of {MAX_COLLECTION_ELEMENTS}"
                );
            }
            budget.output(2 + values.len().saturating_sub(1), context.cancellation)?;
            values
                .iter()
                .map(|value| {
                    evaluate_json_value_with_budget_at_depth(value, context, budget, depth + 1)
                })
                .collect::<Result<Vec<_>>>()
                .map(Value::Array)
        }
        Value::Object(values) => {
            if values.len() > MAX_COLLECTION_ELEMENTS {
                bail!(
                    "dynamic value collection exceeded maximum element count of {MAX_COLLECTION_ELEMENTS}"
                );
            }
            budget.output(2 + values.len().saturating_sub(1), context.cancellation)?;
            values
                .iter()
                .map(|(key, value)| {
                    budget.serialized(key, context.cancellation)?;
                    budget.output(1, context.cancellation)?;
                    Ok((
                        key.clone(),
                        evaluate_json_value_with_budget_at_depth(
                            value,
                            context,
                            budget,
                            depth + 1,
                        )?,
                    ))
                })
                .collect::<Result<serde_json::Map<_, _>>>()
                .map(Value::Object)
        }
        value => {
            budget.serialized(value, context.cancellation)?;
            Ok(value.clone())
        }
    }
}

fn append_text(
    text: &mut String,
    value: &str,
    cancellation: Option<&CancellationToken>,
) -> Result<()> {
    check_cancelled(cancellation)?;
    let length = text
        .len()
        .checked_add(value.len())
        .ok_or_else(|| anyhow::anyhow!("dynamic value output length overflow"))?;
    if length > MAX_RESULT_BYTES {
        bail!("dynamic value exceeded maximum output size of {MAX_RESULT_BYTES} bytes");
    }
    text.push_str(value);
    Ok(())
}

fn append_value_to_text(
    text: &mut String,
    value: &Value,
    cancellation: Option<&CancellationToken>,
) -> Result<()> {
    match value {
        Value::String(value) => append_text(text, value, cancellation),
        value => {
            let encoded = serde_json::to_string(value)
                .context("could not JSON-encode an interpolated dynamic value")?;
            append_text(text, &encoded, cancellation)
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
    let mut bracket_depth = 0usize;
    let mut quote = false;
    let mut escaped = false;
    while cursor < source.len() {
        let character = source[cursor..].chars().next()?;
        let width = character.len_utf8();
        if quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quote = false;
            }
            cursor += width;
            continue;
        }
        if character == '"' {
            quote = true;
            cursor += width;
            continue;
        }
        if source[cursor..].starts_with("{{") {
            cursor += 2;
            continue;
        }
        if source[cursor..].starts_with("}}") && bracket_depth == 0 {
            return Some(cursor);
        }
        match character {
            '[' => bracket_depth += 1,
            ']' if bracket_depth > 0 => bracket_depth -= 1,
            _ => {}
        }
        cursor += width;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluate(source: &str, root: Value) -> Result<Value> {
        let registry = TemplateRegistry::compile_json_tree(&Value::String(source.to_string()))?;
        Template::parse(source)?.evaluate_value(&EvalContext {
            root: &root,
            cancellation: None,
            templates: Some(&registry),
        })
    }

    #[test]
    fn paths_preserve_types_and_support_keys_indexes_and_unicode() {
        let root = serde_json::json!({
            "page": {
                "items": [{"odd-key": true}],
                "query": {"snow雪": {"quoted\"key": 7}}
            }
        });
        assert_eq!(
            evaluate("{{ page.items[0][\"odd-key\"] }}", root.clone()).unwrap(),
            true
        );
        assert_eq!(
            evaluate(
                "{{ page[\"query\"][\"snow\\u96ea\"][\"quoted\\\"key\"] }}",
                root.clone()
            )
            .unwrap(),
            7
        );
        assert_eq!(evaluate("{{ page }}", root.clone()).unwrap(), root["page"]);
        assert_eq!(
            evaluate(r"literal \{{ page }}", root).unwrap(),
            r"literal \{{ page }}"
        );
    }

    #[test]
    fn mixed_templates_stringify_non_strings_as_compact_json() {
        let root = serde_json::json!({"page": {"query": {
            "text": "Firefox",
            "none": null,
            "enabled": true,
            "count": 3,
            "object": {"x": 1},
            "array": ["one", 2],
        }}});
        assert_eq!(
            evaluate(
                "{{ page.query.text }} {{ page.query.none }}/{{ page.query.enabled }}/{{ page.query.count }}",
                root.clone()
            )
            .unwrap(),
            "Firefox null/true/3"
        );
        assert_eq!(
            evaluate(
                "object={{ page.query.object }}; array={{ page.query.array }}",
                root,
            )
            .unwrap(),
            r#"object={"x":1}; array=["one",2]"#
        );
    }

    #[test]
    fn parser_rejects_non_path_syntax() {
        for source in [
            "{{ 1 }}",
            "{{ page.value == 1 }}",
            "{{ unknown }}",
            "{{ page[\"unterminated] }}",
            "{{ page[-1] }}",
            "{{ page[1.0] }}",
            "{{ page[] }}",
        ] {
            assert!(Template::parse(source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn stages_allow_only_inherited_scope_capabilities() {
        let source = serde_json::json!({
            "invocation": "{{ input }}",
            "operation": "{{ view.query }} {{ page.input }} {{ selection }} {{ session.input }}",
            "returned": "{{ result }}",
        });
        let registry = TemplateRegistry::compile_json_tree(&source).unwrap();

        let invocation = registry
            .requirements_for_value(&source["invocation"])
            .unwrap();
        invocation
            .validate_stage(EvaluationStage::Invocation, "test invocation")
            .unwrap();

        let operation = registry
            .requirements_for_value(&source["operation"])
            .unwrap();
        operation
            .validate_stage(EvaluationStage::Operation, "test operation")
            .unwrap();
        let error = operation
            .validate_stage(EvaluationStage::Invocation, "test invocation")
            .unwrap_err();
        assert!(error.to_string().contains("invocation evaluation stage"));
        assert!(error.to_string().contains("\"view\""));

        let returned = registry
            .requirements_for_value(&source["returned"])
            .unwrap();
        let error = returned
            .validate_stage(EvaluationStage::Operation, "test operation")
            .unwrap_err();
        assert!(error.to_string().contains("\"result\""));
        returned
            .validate_stage(EvaluationStage::Return, "test return")
            .unwrap();
    }

    #[test]
    fn one_compiled_template_resolves_against_each_snapshot() {
        let source = Value::String("{{ page.ref }}".to_string());
        let registry = TemplateRegistry::compile_json_tree(&source).unwrap();
        let template = Template::parse(source.as_str().unwrap()).unwrap();
        let first = serde_json::json!({"page": {"ref": "core:default"}});
        let second = serde_json::json!({"page": {"ref": "apps:main"}});
        let first_value = template
            .evaluate_value(&EvalContext {
                root: &first,
                cancellation: None,
                templates: Some(&registry),
            })
            .unwrap();
        let second_value = template
            .evaluate_value(&EvalContext {
                root: &second,
                cancellation: None,
                templates: Some(&registry),
            })
            .unwrap();
        assert_eq!(first_value, "core:default");
        assert_eq!(second_value, "apps:main");
    }

    #[test]
    fn traversal_errors_distinguish_missing_values_and_wrong_types() {
        let missing = evaluate(
            "{{ page.query.missing }}",
            serde_json::json!({"page": {"query": {}}}),
        )
        .unwrap_err();
        assert!(missing.to_string().contains("missing member"));
        let wrong_type = evaluate(
            "{{ page.query.value.name }}",
            serde_json::json!({"page": {"query": {"value": 1}}}),
        )
        .unwrap_err();
        assert!(wrong_type.to_string().contains("cannot read member"));
    }

    #[test]
    fn recursive_native_values_are_evaluated() {
        let value = serde_json::json!({
            "array": ["{{ page.query.value }}", {"nested": "v={{ page.query.value }}"}],
            "static": true
        });
        validate_json_value(&value).unwrap();
        assert_eq!(
            evaluate_json_value(
                &value,
                &EvalContext {
                    root: &serde_json::json!({"page": {"query": {"value": 4}}}),
                    cancellation: None,
                    templates: Some(&TemplateRegistry::compile_json_tree(&value).unwrap()),
                },
            )
            .unwrap(),
            serde_json::json!({"array": [4, {"nested": "v=4"}], "static": true})
        );
    }

    #[test]
    fn compile_depth_and_path_budgets_are_enforced() {
        let path = format!("{{{{ page.query{} }}}}", ".value".repeat(MAX_PATH_SEGMENTS));
        assert!(Template::parse(&path).is_err());

        let mut nested = Value::Null;
        for _ in 0..=MAX_DEPTH {
            nested = Value::Array(vec![nested]);
        }
        assert!(validate_json_value(&nested).is_err());
    }

    #[test]
    fn output_budget_matches_serialized_result_size() {
        let oversized = "x".repeat(MAX_RESULT_BYTES);
        let error = evaluate(
            "{{ page.query.value }}",
            serde_json::json!({"page": {"query": {"value": oversized}}}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("output size"));
    }

    #[test]
    fn cancellation_is_checked_during_large_traversal() {
        let token = CancellationToken::new();
        let cancel = token.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1));
            cancel.cancel();
        });
        let values = (0..MAX_COLLECTION_ELEMENTS)
            .map(|index| serde_json::json!({"index": index, "padding": "xxxxxxxxxxxxxxxx"}))
            .collect::<Vec<_>>();
        let error = evaluate_json_value(
            &serde_json::json!("{{ page.items }}"),
            &EvalContext {
                root: &serde_json::json!({"page": {"items": values}}),
                cancellation: Some(&token),
                templates: Some(
                    &TemplateRegistry::compile_json_tree(&serde_json::json!("{{ page.items }}"))
                        .unwrap(),
                ),
            },
        )
        .unwrap_err();
        thread.join().unwrap();
        assert!(error.to_string().contains("cancelled"));
    }
}
