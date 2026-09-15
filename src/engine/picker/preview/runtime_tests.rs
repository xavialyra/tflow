use super::*;
use crate::task::{MountTaskLease, MountTaskStarter, TaskRuntime};
use serde_json::json;
use std::time::{Duration, Instant};

fn runtime() -> (TaskRuntime, MountTaskStarter) {
    let tasks = TaskRuntime::new();
    let starter =
        MountTaskStarter::from_lease(&tasks, MountTaskLease::new(crate::input::ViewMountId(912)));
    (tasks, starter)
}
fn source(script: &str) -> PreviewSource {
    parse_source(
        json!({"producer":"script","handler":{"script":script}}),
        None,
    )
    .unwrap()
}
fn request(source: PreviewSource, metadata: Value) -> PreviewRequest {
    let item = Item {
        text: "same".into(),
        display: super::super::ItemDisplayInput::Plain("same".into()).into(),
        value: Some("same".into()),
        metadata,
        source_view: "feed:main".into(),
    };
    let request = crate::protocol::preview_request(
        &json!({"mode":"owner"}),
        &json!({"stdin":{"path":"launch"}}),
        &json!({"input":"query", "item": item_value(&item)}),
    );
    PreviewRequest {
        identity: request.to_string(),
        owner: "feed:main".into(),
        request,
        root: None,
        source,
    }
}
fn preview() -> PickerPreview {
    let mut preview = PickerPreview::new(parse(0.35, 24, None).unwrap());
    preview.set_visible(true);
    preview
}
fn ready(preview: &mut PickerPreview) {
    preview.due = Some(Instant::now());
}
fn collect(preview: &mut PickerPreview, starter: &MountTaskStarter) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while preview.script_task.is_some() && Instant::now() < deadline {
        preview.start(starter);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(preview.script_task.is_none(), "preview did not complete");
}
const ECHO: &str = "#!/usr/bin/env python3\nimport json,sys\nr=json.load(sys.stdin)\nassert r['version']==1 and r['entrypoint']=='picker-preview'\nc=r['context']\nassert c['parameters']['mode']=='owner' and c['input']['stdin']['path']=='launch'\nassert c['engine']['state']['input']=='query'\nprint(json.dumps({'version':1,'preview':c['engine']['state']['item']['metadata']['body']}))\n";

#[test]
fn preview_debounces_and_reloads_same_value_metadata_with_managed_reap() {
    let (tasks, starter) = runtime();
    let mut preview = preview();
    let first = request(source(ECHO), json!({"body":"first"}));
    preview.prepare(Some(first));
    assert!(preview.script_task.is_none());
    assert!(preview.start(&starter).is_none());
    ready(&mut preview);
    let first_generation = preview.start(&starter).unwrap();
    collect(&mut preview, &starter);
    assert!(matches!(preview.document, Some(document::Document::Text(ref s)) if s == "first"));
    preview.prepare(Some(request(source(ECHO), json!({"body":"changed"}))));
    assert!(matches!(preview.render_state().document, Some(document::Document::Text(ref s)) if s == "first"));
    // Verify grace period expiration reveals loading state when exceeding grace window
    preview.grace_due = Some(Instant::now() - Duration::from_millis(1));
    assert!(preview.render_state().document.is_none());
    assert_eq!(
        preview.render_state().status.as_deref(),
        Some("Loading preview…")
    );
    ready(&mut preview);
    assert_ne!(preview.start(&starter).unwrap(), first_generation);
    collect(&mut preview, &starter);
    assert!(matches!(preview.document, Some(document::Document::Text(ref s)) if s == "changed"));
    tasks.shutdown_and_wait();
    let metrics = tasks.metrics_snapshot();
    let completed = metrics
        .recent_terminal
        .iter()
        .filter(|sample| sample.tags.task_class == "preview")
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 2);
    assert!(
        completed
            .iter()
            .all(|sample| sample.process_reaped_at.is_some())
    );
}

#[test]
fn preview_selection_hidden_reshow_and_deactivation_cancel_and_reject_stale_results() {
    let (tasks, starter) = runtime();
    let mut preview = preview();
    let slow = source("#!/bin/sh\nsleep 5\nprintf '%s' '{\"version\":1,\"preview\":\"stale\"}'\n");
    preview.prepare(Some(request(slow, json!({"body":"slow"}))));
    ready(&mut preview);
    preview.start(&starter).unwrap();
    preview.prepare(Some(request(source(ECHO), json!({"body":"latest"}))));
    assert!(preview.script_task.is_none());
    ready(&mut preview);
    preview.start(&starter).unwrap();
    collect(&mut preview, &starter);
    assert!(matches!(preview.document, Some(document::Document::Text(ref s)) if s == "latest"));
    preview.set_visible(false);
    assert!(preview.document.is_none() && preview.selection.is_none());
    preview.set_visible(true);
    preview.prepare(Some(request(source(ECHO), json!({"body":"latest"}))));
    ready(&mut preview);
    preview.start(&starter).unwrap();
    preview.prepare(None);
    assert!(preview.script_task.is_none() && preview.selection.is_none());
    preview.prepare(Some(request(source(ECHO), json!({"body":"restored"}))));
    ready(&mut preview);
    preview.start(&starter).unwrap();
    collect(&mut preview, &starter);
    preview.deactivate();
    assert!(
        preview.document.is_none() && preview.script_task.is_none() && preview.prepared.is_none()
    );
    assert!(preview.start(&starter).is_none());
    tasks.shutdown_and_wait();
}

#[test]
fn preview_protocol_schema_output_and_process_failures_are_renderable_errors() {
    let (tasks, starter) = runtime();
    for script in [
        "#!/bin/sh\nprintf '%s' '{\"version\":2,\"preview\":\"bad\"}'",
        "#!/bin/sh\nprintf '%s' '{\"version\":1}'",
        "#!/bin/sh\nprintf '%s' '{\"version\":1,\"preview\":null,\"operation\":{}}'",
        "#!/bin/sh\nprintf '%s' '{\"version\":1,\"preview\":{\"type\":\"paragraph\",\"text\":\"bad\",\"action\":\"exit\"}}'",
        "#!/bin/sh\nprintf '%s' '{\"version\":1,\"preview\":null}{}'",
        "#!/bin/sh\necho broken >&2\nexit 9",
        "#!/usr/bin/env python3\nprint('x' * (1024 * 1024 + 1))",
    ] {
        let mut preview = preview();
        preview.prepare(Some(request(source(script), json!({}))));
        ready(&mut preview);
        preview.start(&starter).unwrap();
        collect(&mut preview, &starter);
        assert!(
            preview.error && preview.status.is_some(),
            "accepted script {script}"
        );
    }
    let mut empty = preview();
    empty.prepare(Some(request(
        source("#!/bin/sh\nprintf '%s' '{\"version\":1,\"preview\":null}'"),
        json!({}),
    )));
    ready(&mut empty);
    empty.start(&starter).unwrap();
    collect(&mut empty, &starter);
    assert_eq!(empty.status.as_deref(), Some("(no preview)"));
    assert!(!empty.error);
    tasks.shutdown_and_wait();
}

#[test]
fn declared_document_images_start_only_with_authority_and_resolve_owner_root() {
    let (tasks, starter) = runtime();
    let root =
        std::env::temp_dir().join(format!("tlaunch-preview-document-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    image::DynamicImage::new_rgb8(2, 2)
        .save(root.join("art.png"))
        .unwrap();
    let doc = document::parse(json!({"type":"layout","direction":"vertical","children":["caption",{"type":"image","path":"art.png"}]})).unwrap();
    let mut request = request(PreviewSource::Declared(doc), json!({}));
    request.root = Some(root.clone());
    let mut preview = preview();
    preview.prepare(Some(request));
    assert!(preview.task.is_none());
    preview.start(&starter);
    assert!(preview.task.is_some());
    for _ in 0..100 {
        preview.start(&starter);
        if preview.task.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(
        preview.images[0],
        PreviewImageState {
            image: Some(_),
            error: None
        }
    ));
    preview.deactivate();
    tasks.shutdown_and_wait();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn preview_fixture_script_roundtrip_decodes_its_workflow_relative_image() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/preview/workflows/library");
    let source = parse_source(
        json!({"producer":"script","handler":{"file":"scripts/preview.py"}}),
        Some(&root),
    )
    .unwrap();
    let mut request = request(
        source,
        json!({"summary":"Fixture roundtrip", "image":"art.png"}),
    );
    request.root = Some(root);
    request.request["context"]["parameters"]["owner"] = json!("library");
    let (tasks, starter) = runtime();
    let mut preview = preview();
    preview.prepare(Some(request));
    ready(&mut preview);
    preview.start(&starter).unwrap();
    collect(&mut preview, &starter);
    assert!(preview.document.is_some() && !preview.error);
    for _ in 0..100 {
        preview.start(&starter);
        if preview.task.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(
        preview.images[0],
        PreviewImageState {
            image: Some(_),
            error: None
        }
    ));
    preview.deactivate();
    tasks.shutdown_and_wait();
}

#[test]
fn preview_scroll_up_responds_immediately_after_repeated_scroll_down_and_resize() {
    let (tasks, starter) = runtime();
    let mut preview = preview();
    let document = document::parse(json!(
        (0..10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
    .unwrap();
    preview.prepare(Some(request(PreviewSource::Declared(document), json!({}))));
    preview.start(&starter);
    preview.set_content_size(Some((40, 3)));
    for _ in 0..100 {
        preview.scroll(3);
    }
    assert_eq!(preview.scroll, 7);
    preview.scroll(-3);
    assert_eq!(preview.scroll, 4);
    preview.set_content_size(Some((40, 8)));
    assert_eq!(preview.scroll, 2);
    preview.set_content_size(Some((40, 3)));
    assert_eq!(preview.scroll, 2);
    preview.scroll(3);
    assert_eq!(preview.scroll, 5);
    preview.set_content_size(Some((40, 8)));
    assert_eq!(preview.scroll, 2);
    preview.scroll(-3);
    assert_eq!(preview.scroll, 0);
    preview.deactivate();
    tasks.shutdown_and_wait();
}
