use super::*;
use crate::config::{Config, EngineDefinition};
use crate::engine::{EngineDriver, EngineHost, SessionEffect};
use crate::expression::{EvalContext, ExpressionMethods, Template, TreeReferences};
use crate::terminal::Terminal;
use anyhow::Result;
use serde_json::Value;
use std::env;
use std::fs;
use std::path::Path;

#[test]
fn registry_accepts_custom_engine_implementations() {
    struct TestEngine;

    struct TestDriver;

    impl EngineDriver for TestDriver {
        fn step(
            &mut self,
            _host: &mut EngineHost<'_>,
            _terminal: &mut Terminal,
        ) -> Result<SessionEffect> {
            Ok(SessionEffect::Continue)
        }

        fn render(&self, _host: &EngineHost<'_>, _terminal: &Terminal) -> Result<()> {
            Ok(())
        }
    }

    impl Engine for TestEngine {
        fn engine_type(&self) -> &'static str {
            "test"
        }

        fn validate_config(&self, _name: &str, _definition: &EngineDefinition) -> Result<()> {
            Ok(())
        }

        fn create_view(
            &self,
            _config: &Config,
            _view_ref: &str,
            _input: &str,
            _log_file: Option<&Path>,
            _runtime: RuntimeHandle,
        ) -> Result<Box<dyn EngineDriver>> {
            Ok(Box::new(TestDriver))
        }

        fn create_command(&self, _execution: CommandExecution) -> Result<Box<dyn EngineDriver>> {
            Ok(Box::new(TestDriver))
        }
    }

    let mut registry = EngineRegistry::new();
    registry.register(Box::new(TestEngine));
    assert!(registry.contains("test"));
}

#[test]
fn runtime_store_tracks_revisions_and_json_pointer_updates() {
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

#[test]
fn expression_path_projects_a_runtime_value() {
    let config = Value::Null;
    let runtime = serde_json::json!({"items": [1, 2]});
    let references = TreeReferences {
        config: &config,
        runtime: &runtime,
    };
    let mut methods = ExpressionMethods::new(Path::new("."));
    let mut context = EvalContext {
        references: &references,
        methods: &mut methods,
    };
    assert_eq!(
        Template::parse(r#"{{ path(runtime:items, "$") }}"#)
            .unwrap()
            .evaluate_value(&mut context)
            .unwrap(),
        serde_json::json!([1, 2])
    );
}

#[test]
fn expression_script_passes_json_params() {
    let root = env::temp_dir().join(format!("tui-launcher-engine-script-{}", std::process::id()));
    fs::remove_dir_all(&root).ok();
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("params.sh"), "cat\n").unwrap();
    let config = Value::Null;
    let runtime = Value::Null;
    let references = TreeReferences {
        config: &config,
        runtime: &runtime,
    };
    let mut methods = ExpressionMethods::new(&root);
    let mut context = EvalContext {
        references: &references,
        methods: &mut methods,
    };
    assert_eq!(
        Template::parse(r#"{{ script("params.sh", {query = "fire"}) }}"#)
            .unwrap()
            .evaluate_value(&mut context)
            .unwrap(),
        serde_json::json!({"query": "fire"})
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn expression_script_paths_cannot_escape_the_root() {
    let config = Value::Null;
    let runtime = Value::Null;
    let references = TreeReferences {
        config: &config,
        runtime: &runtime,
    };
    let mut methods = ExpressionMethods::new(Path::new("."));
    let mut context = EvalContext {
        references: &references,
        methods: &mut methods,
    };
    let error = Template::parse(r#"{{ script("../test.sh") }}"#)
        .unwrap()
        .evaluate_value(&mut context)
        .expect_err("script paths must remain below the root");
    assert!(error.to_string().contains("must stay below"));
}
