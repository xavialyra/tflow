use crate::command_runner::run_bounded_command_with_stdin;
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

pub(super) fn evaluate(
    root: &Path,
    args: Vec<Value>,
    named_args: BTreeMap<String, Value>,
) -> Result<Value> {
    if args.len() > 1 {
        bail!("script accepts at most one positional target")
    }
    if let Some(name) = named_args
        .keys()
        .find(|name| *name != "target" && *name != "params")
    {
        bail!("script does not accept named argument {:?}", name)
    }
    if args.len() == 1 && named_args.contains_key("target") {
        bail!("script cannot combine a positional target with target =")
    }

    let target = args
        .into_iter()
        .next()
        .or_else(|| named_args.get("target").cloned())
        .context("script requires a target")?;
    let target = target.as_str().context("script requires a string target")?;
    if target.is_empty() {
        bail!("script requires a non-empty target")
    }
    run_script(target, root, named_args.get("params"))
}

fn run_script(target: &str, root: &Path, params: Option<&Value>) -> Result<Value> {
    let path = resolve_script_path(root, target)?;
    let mut process = ProcessCommand::new("sh");
    process.arg(&path).current_dir(root);
    let input = params
        .filter(|value| !value.is_null())
        .map(serde_json::to_vec)
        .transpose()
        .context("could not serialize script params")?;
    if input
        .as_ref()
        .is_some_and(|value| value.len() > MAX_SCRIPT_STDIN)
    {
        bail!("script params exceeded {} bytes", MAX_SCRIPT_STDIN);
    }
    let output = run_bounded_command_with_stdin(
        process,
        input.as_deref(),
        SCRIPT_TIMEOUT,
        MAX_SCRIPT_STDOUT,
        MAX_SCRIPT_STDERR,
    )
    .with_context(|| format!("could not run script {}", path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("script exited with status {}", output.status);
        }
        bail!("script failed: {}", stderr);
    }
    if output.stdout.is_empty() {
        bail!("script produced no JSON output");
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("script {} did not produce valid JSON", path.display()))
}

fn resolve_script_path(root: &Path, target: &str) -> Result<PathBuf> {
    let relative = Path::new(target);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "script path {:?} must stay below {}",
            target,
            root.display()
        );
    }
    let canonical_root = fs::canonicalize(root)
        .with_context(|| format!("could not resolve script root {}", root.display()))?;
    let path = root.join(relative);
    let canonical_path = fs::canonicalize(&path)
        .with_context(|| format!("could not read script {}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        bail!("script path {:?} escapes {}", target, root.display());
    }
    Ok(canonical_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::{EvalContext, ExpressionMethods, Template, TreeReferences};
    use std::env;

    fn evaluate_expression(source: &str, script_root: &Path) -> Result<Value> {
        let config = Value::Null;
        let runtime = Value::Null;
        let references = TreeReferences {
            config: &config,
            runtime: &runtime,
        };
        let mut methods = ExpressionMethods::new(script_root);
        let mut context = EvalContext {
            references: &references,
            methods: &mut methods,
        };
        Template::parse(source)?.evaluate_value(&mut context)
    }

    #[test]
    fn script_receives_json_params() {
        let root = env::temp_dir().join(format!(
            "tui-launcher-expression-script-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("params.sh"), "cat\n").unwrap();
        assert_eq!(
            evaluate_expression(
                r#"{{ script("params.sh", params = {query = "fire"}) }}"#,
                &root,
            )
            .unwrap(),
            serde_json::json!({"query": "fire"})
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn script_paths_cannot_escape_the_root() {
        let error = evaluate_expression(r#"{{ script("../test.sh") }}"#, Path::new("."))
            .expect_err("script paths must remain below the root");
        assert!(error.to_string().contains("must stay below"));
    }
}
