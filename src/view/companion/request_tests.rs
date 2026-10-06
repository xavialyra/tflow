use crate::view::*;
use serde_json::json;

struct Routes {
    auto_open: bool,
    invalid: bool,
}

impl RouteCatalog for Routes {
    fn resolve(&self, target: &str) -> Option<ViewLocation> {
        match target {
            "main" | "side" | "broken" => Some(ViewLocation::new(target)),
            "alias" => Some(ViewLocation::new("side")),
            _ => None,
        }
    }

    fn query_schema(&self, _: &str) -> Option<QuerySchema> {
        Some(QuerySchema { id: "query".into() })
    }

    fn default_companion(&self, target: &str) -> Option<String> {
        (target == "main" && self.auto_open).then(|| "side".into())
    }

    fn default_query(&self, target: &str) -> anyhow::Result<ParsedQuery> {
        Ok(ParsedQuery::new(target, "query", json!({"limit": 10})))
    }

    fn validate_query(&self, query: &ParsedQuery) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.invalid && query.values == json!({"limit": 10}),
            "invalid defaults"
        );
        Ok(())
    }
}

struct Factory;
struct TestView;

impl ViewFactory for Factory {
    fn create(
        &self,
        request: &NavigationRequest,
        _: ViewInstanceId,
        _: &ViewServices<'_>,
    ) -> anyhow::Result<Box<dyn View>> {
        anyhow::ensure!(request.target != "broken", "factory rejected target");
        if request.target == "main" {
            assert_eq!(request.execution_class, crate::task::TaskExecutionClass::Serial);
        } else {
            assert_eq!(request.execution_class, crate::task::TaskExecutionClass::Background);
        }
        Ok(Box::new(TestView))
    }
}

impl View for TestView {
    fn command_snapshot(&self) -> ViewCommandSnapshot {
        ViewCommandSnapshot {
            engine_type: "test".into(),
            parameters: json!({"limit": 10}),
            raw_input: "primary input".into(),
            runtime: Value::Null,
            publication: None,
            revision: 0,
        }
    }
    fn event(&mut self, _: ViewEvent, _: &ViewContext) -> anyhow::Result<ViewDecision> {
        Ok(ViewDecision::Stay)
    }
    fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> anyhow::Result<RenderResult> {
        Ok(RenderResult::default())
    }
}

fn router(auto_open: bool) -> Router {
    let mut router = Router::new(
        Box::new(Routes {
            auto_open,
            invalid: false,
        }),
        Box::new(Factory),
    );
    router
        .push(NavigationRequest::new(
            "main",
            ParsedQuery::new("main", "query", json!({"limit": 10})),
        ))
        .unwrap();
    router
}

#[test]
fn companion_mount_and_navigation_share_canonical_validated_defaults() {
    let mut router = router(false);
    router
        .toggle_companion(None, "alias", Some(json!({"limit": 10})))
        .unwrap();
    let mount = router.active_companion().unwrap();
    assert_eq!(mount.instance.context.location.target, "side");
    assert_eq!(mount.instance.context.query.values, json!({"limit": 10}));
    let request = router
        .companion_navigation_request(&mount.target, mount.last_query.clone())
        .unwrap();
    assert_eq!(request.target, "side");
    assert_eq!(request.query, mount.instance.context.query);
    assert_eq!(request.execution_class, crate::task::TaskExecutionClass::Serial);
    assert!(mount.last_data.is_none(), "explicit query must not inherit source data");
}

#[test]
fn companion_auto_open_and_toggle_resolve_initial_binding_before_mount() {
    for auto_open in [false, true] {
        let mut router = router(auto_open);
        if !auto_open {
            router.toggle_companion(None, "side", None).unwrap();
        }
        let mount = router.active_companion().unwrap();
        assert_eq!(mount.target, "side");
        assert_eq!(mount.instance.context.location.target, "side");
    }
}

#[test]
fn companion_explicit_data_does_not_inherit_configured_live_binding() {
    let mut router = router(false);
    router
        .toggle_companion(None, "side", Some(json!({"limit": 10})))
        .unwrap();
    let mount = router.active_companion().unwrap();
    assert_eq!(mount.last_query, Some(json!({"limit": 10})));
}

#[test]
fn companion_failed_replacement_keeps_existing_mount() {
    let mut router = router(true);
    let original = router.active_companion().unwrap().instance.id;
    assert!(router.toggle_companion(None, "broken", None).is_err());
    assert_eq!(router.active_companion().unwrap().instance.id, original);
    assert!(router.toggle_companion(None, "missing", None).is_err());
    assert_eq!(router.active_companion().unwrap().instance.id, original);
    router.routes = Box::new(Routes {
        auto_open: false,
        invalid: true,
    });
    assert!(router.toggle_companion(None, "broken", None).is_err());
    assert_eq!(router.active_companion().unwrap().instance.id, original);
}

#[test]
fn companion_alias_toggle_addresses_the_same_canonical_target() {
    let mut router = router(true);
    router.toggle_companion(None, "alias", None).unwrap();
    assert!(router.active_companion().is_none());
}

#[test]
fn companion_auto_open_failure_is_observable_without_rejecting_primary() {
    struct BadDefaults;
    impl RouteCatalog for BadDefaults {
        fn resolve(&self, target: &str) -> Option<ViewLocation> {
            Some(ViewLocation::new(target))
        }
        fn query_schema(&self, _: &str) -> Option<QuerySchema> {
            Some(QuerySchema { id: "query".into() })
        }
        fn default_companion(&self, target: &str) -> Option<String> {
            (target == "main").then(|| "side".into())
        }
        fn default_query(&self, _: &str) -> anyhow::Result<ParsedQuery> {
            anyhow::bail!("default query failed")
        }
    }
    let mut router = Router::new(Box::new(BadDefaults), Box::new(Factory));
    router
        .push(NavigationRequest::new(
            "main",
            ParsedQuery::new("main", "query", Value::Null),
        ))
        .unwrap();
    assert_eq!(router.stack().len(), 1);
    assert!(router.active_companion().is_none());
    assert!(router.take_error().is_some());
}
