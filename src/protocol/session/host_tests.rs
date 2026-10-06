use super::*;
use crate::input::{EditorSnapshot, InputEvent, Key};
use crate::view::{InputEdit, MapRouteCatalog, View, ViewContext, ViewFactory, ViewServices};
use ratatui::{
    Terminal,
    backend::{Backend, TestBackend},
};
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
struct Geometry {
    resizes: Vec<(ViewInstanceId, TerminalSize)>,
    renders: Vec<(ViewInstanceId, Rect)>,
}

struct EditableFactory(Rc<RefCell<Geometry>>);
struct EditableView {
    id: ViewInstanceId,
    input: EditorSnapshot,
    geometry: Rc<RefCell<Geometry>>,
    omnibar: bool,
}

impl ViewFactory for EditableFactory {
    fn create(
        &self,
        request: &NavigationRequest,
        id: ViewInstanceId,
        _: &ViewServices<'_>,
    ) -> Result<Box<dyn View>> {
        let seed = request.input.clone().unwrap_or(crate::view::ViewInputSeed {
            text: String::new(),
            cursor: 0,
        });
        Ok(Box::new(EditableView {
            id,
            input: crate::input::EditorBuffer::from_raw(seed.text, seed.cursor).snapshot(),
            geometry: self.0.clone(),
            omnibar: request.target != "details",
        }))
    }
}

impl View for EditableView {
    fn preferred_top_inset(&self) -> u16 {
        1
    }
    fn input_mode(&self) -> crate::ui::chrome::InputPresentationMode {
        if self.omnibar {
            crate::ui::chrome::InputPresentationMode::Omnibar { show_cursor: true }
        } else {
            crate::ui::chrome::InputPresentationMode::Hidden
        }
    }
    fn input_divider(&self) -> bool {
        true
    }
    fn initial_input(&self) -> EditorSnapshot {
        self.input.clone()
    }
    fn on_host_input_changed(
        &mut self,
        input: &EditorSnapshot,
        _: &ViewContext,
    ) -> Result<ViewDecision> {
        self.input = input.clone();
        Ok(ViewDecision::Invalidate)
    }
    fn command_snapshot(&self) -> crate::view::ViewCommandSnapshot {
        crate::view::ViewCommandSnapshot {
            engine_type: "test".into(),
            parameters: serde_json::Value::Null,
            raw_input: self.input.raw.clone(),
            runtime: serde_json::Value::Null,
            publication: None,
            revision: self.input.revision,
        }
    }
    fn engine_commands(&self, _: &ViewContext) -> Vec<crate::command::CommandEntry> {
        [
            (Key::Enter, "push"),
            (Key::Escape, "back"),
            (Key::Alt('w'), "word"),
            (Key::Alt('d'), "delete"),
            (Key::Alt('u'), "clear"),
            (Key::Alt('f'), "failed"),
        ]
        .into_iter()
        .map(|(key, id)| {
            crate::command::CommandEntry::for_event(id, None, Some(key), BindingLayer::Engine)
        })
        .collect()
    }
    fn on_command(&mut self, id: &str, _: &ViewContext) -> Result<ViewDecision> {
        Ok(match id {
            "push" => ViewDecision::Transition(crate::view::TransitionRequest::Push(
                NavigationRequest::new(
                    "child",
                    crate::view::ParsedQuery::new("child", "query", serde_json::Value::Null),
                )
                .with_input(self.input.raw.clone(), 0)?,
            )),
            "failed" => ViewDecision::Transition(crate::view::TransitionRequest::Push(
                NavigationRequest::new(
                    "missing",
                    crate::view::ParsedQuery::new("missing", "query", serde_json::Value::Null),
                ),
            )),
            "back" => ViewDecision::Close,
            "word" => ViewDecision::EditInput(InputEdit::DeleteWord),
            "delete" => ViewDecision::EditInput(InputEdit::Key(Key::Backspace)),
            "clear" => ViewDecision::EditInput(InputEdit::Clear),
            _ => ViewDecision::Stay,
        })
    }
    fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
        if let ViewEvent::Resize(size) = event {
            self.geometry.borrow_mut().resizes.push((self.id, size));
        }
        Ok(ViewDecision::Stay)
    }
    fn render(&self, _: &mut Frame, area: Rect, _: &RenderContext) -> Result<RenderResult> {
        self.geometry.borrow_mut().renders.push((self.id, area));
        Ok(RenderResult::default())
    }
}

struct NoEffects;
impl crate::view::EffectExecutor for NoEffects {
    fn execute(
        &mut self,
        _: crate::view::EffectRequest,
        _: &ViewContext,
    ) -> Result<crate::view::EffectResult> {
        Ok(crate::view::EffectResult::Complete)
    }
}

fn session_with_input(text: &str, cursor: usize) -> (ProtocolSession, Rc<RefCell<Geometry>>) {
    let geometry = Rc::new(RefCell::new(Geometry::default()));
    let mut routes = MapRouteCatalog::default();
    for target in ["root", "child", "details"] {
        routes.insert(target, target);
    }
    let router = Router::new(
        Box::new(routes),
        Box::new(EditableFactory(geometry.clone())),
    );
    let mut session = ProtocolSession::new(router, Box::new(NoEffects));
    session
        .start_root(
            NavigationRequest::new(
                "root",
                crate::view::ParsedQuery::new("root", "query", serde_json::Value::Null),
            )
            .with_input(text, cursor)
            .unwrap(),
        )
        .unwrap();
    session
        .resize(TerminalSize {
            width: 80,
            height: 24,
        })
        .unwrap();
    (session, geometry)
}

fn key(session: &mut ProtocolSession, key: Key) {
    session
        .input(InputEvent::Key {
            key,
            raw: Vec::new(),
        })
        .unwrap();
}

fn cursor(session: &mut ProtocolSession) -> Position {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    terminal.backend_mut().get_cursor_position().unwrap()
}

#[test]
fn host_edits_and_renders_the_same_utf8_insertion_point() {
    let (mut session, _) = session_with_input("a中bc", "a中".len());
    assert_eq!(cursor(&mut session), Position::new(4, 1));
    key(&mut session, Key::Left);
    key(&mut session, Key::Char('X'));
    session
        .input(InputEvent::Paste {
            text: Some("YZ".into()),
            raw: b"YZ".to_vec(),
        })
        .unwrap();
    assert_eq!(session.router.active().unwrap().input.raw(), "aXYZ中bc");
    assert_eq!(session.router.active().unwrap().input.cursor(), 4);
    assert_eq!(cursor(&mut session), Position::new(5, 1));
    key(&mut session, Key::Delete);
    key(&mut session, Key::Alt('d'));
    assert_eq!(session.router.active().unwrap().input.raw(), "aXYbc");
    assert_eq!(cursor(&mut session), Position::new(4, 1));
    key(&mut session, Key::Alt('w'));
    assert_eq!(
        session
            .router
            .active()
            .unwrap()
            .command_snapshot()
            .raw_input,
        "bc"
    );
    key(&mut session, Key::Alt('u'));
    assert_eq!(session.router.active().unwrap().input.cursor(), 0);
    assert!(session.router.active().unwrap().input.is_empty());
}

#[test]
fn host_restores_parent_cursor_even_when_child_text_is_identical() {
    let (mut session, _) = session_with_input("same", 2);
    let parent = session.router.active().unwrap().id;
    key(&mut session, Key::Enter);
    assert_ne!(session.router.active().unwrap().id, parent);
    assert_eq!(session.router.active().unwrap().input.cursor(), 0);
    key(&mut session, Key::End);
    key(&mut session, Key::Escape);
    assert_eq!(session.router.active().unwrap().id, parent);
    assert_eq!(session.router.active().unwrap().input.cursor(), 2);
    assert_eq!(cursor(&mut session), Position::new(3, 1));
    assert!(
        session
            .input(InputEvent::Key {
                key: Key::Alt('f'),
                raw: Vec::new()
            })
            .is_err()
    );
    assert_eq!(session.router.active().unwrap().input.cursor(), 2);
    assert_eq!(session.router.active().unwrap().input.raw(), "same");
}

#[test]
fn runtime_resize_matches_render_after_toggle_and_under_popup() {
    let (mut session, geometry) = session_with_input("", 0);
    let primary = session.router.active().unwrap().id;
    session
        .router
        .toggle_companion(None, "details", None, None, None)
        .unwrap();
    let companion = session.router.active_companion().unwrap().instance.id;
    session.tick().unwrap();
    cursor(&mut session);
    let state = geometry.borrow();
    for (id, width) in [(primary, 39), (companion, 38)] {
        assert_eq!(
            state
                .resizes
                .iter()
                .rfind(|(owner, _)| *owner == id)
                .unwrap()
                .1,
            TerminalSize { width, height: 20 }
        );
        let area = state
            .renders
            .iter()
            .rfind(|(owner, _)| *owner == id)
            .unwrap()
            .1;
        assert_eq!((area.width, area.height), (width, 20));
    }
    drop(state);
    let mut request = NavigationRequest::new(
        "child",
        crate::view::ParsedQuery::new("child", "query", serde_json::Value::Null),
    );
    request.presentation.mode = crate::workflow::config::ViewPresentationMode::Popup;
    request.presentation.width = Some(20.into());
    request.presentation.height = Some(8.into());
    session.router.push(request).unwrap();
    session.tick().unwrap();
    cursor(&mut session);
    let popup = session.router.active().unwrap().id;
    let state = geometry.borrow();
    assert_eq!(
        state
            .resizes
            .iter()
            .rfind(|(owner, _)| *owner == popup)
            .unwrap()
            .1,
        TerminalSize {
            width: 18,
            height: 4
        }
    );
    assert_eq!(
        state
            .resizes
            .iter()
            .rfind(|(owner, _)| *owner == companion)
            .unwrap()
            .1,
        TerminalSize {
            width: 38,
            height: 20
        }
    );
    drop(state);
    key(&mut session, Key::Escape);
    session
        .router
        .toggle_companion(None, "details", None, None, None)
        .unwrap();
    session.tick().unwrap();
    assert_eq!(
        geometry
            .borrow()
            .resizes
            .iter()
            .rfind(|(owner, _)| *owner == primary)
            .unwrap()
            .1,
        TerminalSize {
            width: 78,
            height: 20
        }
    );
}
