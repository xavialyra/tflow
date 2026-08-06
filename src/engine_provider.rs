use crate::discovery::run_bounded_command_with_stdin;
use crate::expression::{MethodResolver, value_to_text};
use crate::projection::DataRef;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::Duration;

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SCRIPT_STDOUT: usize = 1024 * 1024;
const MAX_SCRIPT_STDERR: usize = 64 * 1024;
const MAX_SCRIPT_STDIN: usize = 64 * 1024;

#[derive(Clone, Copy)]
pub struct ProviderContext<'a> {
    pub config: &'a Value,
    pub runtime: &'a Value,
    #[allow(dead_code)]
    pub runtime_revision: u64,
    pub script_root: &'a Path,
}

pub trait DataProvider {
    fn fetch(
        &self,
        target: Option<&str>,
        params: Option<&Value>,
        context: &ProviderContext<'_>,
    ) -> Result<Value>;
}

struct ConfigProvider;
struct RuntimeProvider;
struct ScriptProvider;

impl DataProvider for ConfigProvider {
    fn fetch(
        &self,
        _target: Option<&str>,
        params: Option<&Value>,
        context: &ProviderContext<'_>,
    ) -> Result<Value> {
        reject_params("config", params)?;
        Ok(context.config.clone())
    }
}

impl DataProvider for RuntimeProvider {
    fn fetch(
        &self,
        _target: Option<&str>,
        params: Option<&Value>,
        context: &ProviderContext<'_>,
    ) -> Result<Value> {
        reject_params("runtime", params)?;
        Ok(context.runtime.clone())
    }
}

impl DataProvider for ScriptProvider {
    fn fetch(
        &self,
        target: Option<&str>,
        params: Option<&Value>,
        context: &ProviderContext<'_>,
    ) -> Result<Value> {
        let target = target
            .filter(|target| !target.is_empty())
            .context("script provider requires a target")?;
        run_script(target, context.script_root, params)
    }
}

fn reject_params(provider: &str, params: Option<&Value>) -> Result<()> {
    if params.is_some_and(|value| !value.is_null()) {
        bail!("{} provider does not accept params", provider);
    }
    Ok(())
}

pub struct DataProviderRegistry<'a> {
    context: ProviderContext<'a>,
    providers: BTreeMap<String, Box<dyn DataProvider>>,
}

impl<'a> DataProviderRegistry<'a> {
    pub fn new(
        config: &'a Value,
        runtime: &'a Value,
        runtime_revision: u64,
        script_root: &'a Path,
    ) -> Self {
        let context = ProviderContext {
            config,
            runtime,
            runtime_revision,
            script_root,
        };
        let mut registry = Self {
            context,
            providers: BTreeMap::new(),
        };
        registry.register("config", ConfigProvider);
        registry.register("runtime", RuntimeProvider);
        registry.register("script", ScriptProvider);
        registry
    }

    pub fn register<P>(&mut self, name: impl Into<String>, provider: P)
    where
        P: DataProvider + 'static,
    {
        self.providers.insert(name.into(), Box::new(provider));
    }

    fn fetch(&self, args: Vec<Value>, named_args: BTreeMap<String, Value>) -> Result<Value> {
        if args.len() > 1 {
            bail!("datafetch accepts at most one request argument");
        }
        if let Some(request) = args.into_iter().next() {
            if !named_args.is_empty() {
                bail!("datafetch cannot combine a request object with named arguments");
            }
            return self.fetch_request(request);
        }
        let provider = named_args
            .get("provider")
            .and_then(Value::as_str)
            .context("datafetch requires a string provider")?;
        let target = named_args.get("target").and_then(Value::as_str);
        let matcher = named_args.get("match").and_then(Value::as_str);
        let params = named_args.get("params");
        self.resolve(provider, target, matcher, params)
    }

    fn fetch_request(&self, request: Value) -> Result<Value> {
        let object = request
            .as_object()
            .context("datafetch request must be an object")?;
        let provider = object
            .get("provider")
            .and_then(Value::as_str)
            .context("datafetch request requires a string provider")?;
        let target = object.get("target").and_then(Value::as_str);
        let matcher = object.get("match").and_then(Value::as_str);
        let params = object.get("params");
        self.resolve(provider, target, matcher, params)
    }

    fn resolve(
        &self,
        provider: &str,
        target: Option<&str>,
        matcher: Option<&str>,
        params: Option<&Value>,
    ) -> Result<Value> {
        let request = DataRef {
            provider: provider.to_string(),
            target: target.map(str::to_string),
            matcher: matcher.map(str::to_string),
        };
        request.validate()?;
        let provider = self
            .providers
            .get(provider)
            .with_context(|| format!("unknown data provider {:?}", request.provider))?;
        let source = provider.fetch(target, params, &self.context)?;
        request.project(source)
    }
}

impl MethodResolver for DataProviderRegistry<'_> {
    fn call_method(
        &mut self,
        name: &str,
        args: Vec<Value>,
        named_args: BTreeMap<String, Value>,
    ) -> Result<Value> {
        match name {
            "datafetch" | "datafetch_value" => self.fetch(args, named_args),
            "datafetch_text" => value_to_text(&self.fetch(args, named_args)?).map(Value::String),
            _ => bail!("unknown expression method {:?}", name),
        }
    }
}

fn run_script(target: &str, root: &Path, params: Option<&Value>) -> Result<Value> {
    let path = resolve_script_path(root, target)?;
    let mut process = ProcessCommand::new("sh");
    process.arg(&path).current_dir(root);
    let input = params
        .filter(|value| !value.is_null())
        .map(serde_json::to_vec)
        .transpose()
        .context("could not serialize script provider params")?;
    if input
        .as_ref()
        .is_some_and(|value| value.len() > MAX_SCRIPT_STDIN)
    {
        bail!("script provider params exceeded {} bytes", MAX_SCRIPT_STDIN);
    }
    let output = run_bounded_command_with_stdin(
        process,
        input.as_deref(),
        SCRIPT_TIMEOUT,
        MAX_SCRIPT_STDOUT,
        MAX_SCRIPT_STDERR,
    )
    .with_context(|| format!("could not run script provider {}", path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("script provider exited with status {}", output.status);
        }
        bail!("script provider failed: {}", stderr);
    }
    if output.stdout.is_empty() {
        bail!("script provider produced no JSON output");
    }
    serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "script provider {} did not produce valid JSON",
            path.display()
        )
    })
}

fn resolve_script_path(root: &Path, target: &str) -> Result<PathBuf> {
    let relative = Path::new(target);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "script provider path {:?} must stay below {}",
            target,
            root.display()
        );
    }
    let canonical_root = fs::canonicalize(root)
        .with_context(|| format!("could not resolve provider root {}", root.display()))?;
    let path = root.join(relative);
    let canonical_path = fs::canonicalize(&path)
        .with_context(|| format!("could not read provider script {}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        bail!(
            "script provider path {:?} escapes {}",
            target,
            root.display()
        );
    }
    Ok(canonical_path)
}
