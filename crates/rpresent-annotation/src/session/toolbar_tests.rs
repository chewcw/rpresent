use super::*;
use crate::toolbar::{DisplayGeometry, PickerKind, Target};

fn ready() -> Session {
    let mut session = Session::new(
        &AnnotationConfig::default(),
        &crate::session::default_bindings(),
    )
    .unwrap();
    session.update_display(DisplayGeometry {
        root: Rect {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        },
        monitors: Vec::new(),
        pointer: Point { x: 600, y: 300 },
    });
    session.command(AnnotationCommand::On).unwrap();
    session.ownership(Some(1));
    session
}

fn center(session: &Session, target: Target) -> Point {
    let bounds = session
        .toolbar_layout()
        .unwrap()
        .controls
        .iter()
        .find(|control| control.target == target)
        .unwrap()
        .bounds;
    Point {
        x: bounds.x + bounds.width as i32 / 2,
        y: bounds.y + bounds.height as i32 / 2,
    }
}

fn click(session: &mut Session, at: Point, epoch: u64) {
    session.pointer(
        epoch,
        PointerInput::Down {
            root: at,
            time: Duration::ZERO,
        },
    );
    session.pointer(
        epoch,
        PointerInput::Up {
            root: at,
            time: Duration::from_millis(10),
        },
    );
}

/// A canvas point clear of the bar and any open picker.
fn canvas_point(session: &Session) -> Point {
    let layout = session.toolbar_layout().unwrap();
    let mut bottom = layout.bar.bottom();
    if let Some(picker) = layout.picker {
        bottom = bottom.max(picker.bottom());
    }
    Point {
        x: 640,
        y: bottom + 50,
    }
}

fn escape(session: &mut Session, epoch: u64) {
    session.key(
        epoch,
        KeyInput {
            key: Key::Esc,
            text: None,
            modifiers: Modifiers::default(),
            shortcut: None,
        },
    );
}

fn drag_line(session: &mut Session, from: Point, to: Point) {
    session.pointer(
        1,
        PointerInput::Down {
            root: from,
            time: Duration::ZERO,
        },
    );
    session.pointer(
        1,
        PointerInput::Move {
            root: to,
            time: Duration::from_millis(10),
        },
    );
    session.pointer(
        1,
        PointerInput::Up {
            root: to,
            time: Duration::from_millis(20),
        },
    );
}

fn start_draft(session: &mut Session) {
    session
        .command(AnnotationCommand::Tool(AnnotationTool::Text))
        .unwrap();
    let at = canvas_point(session);
    click(session, at, 1);
    assert!(
        session.draft_id().is_some(),
        "expected an active text draft"
    );
}

fn type_char(session: &mut Session, character: char) {
    session.key(
        1,
        KeyInput {
            key: Key::Char(character),
            text: Some(character),
            modifiers: Modifiers::default(),
            shortcut: None,
        },
    );
}

#[test]
fn toolbar_tool_button_uses_command_without_drawing() {
    let mut session = ready();
    let at = center(&session, Target::Tool(AnnotationTool::Line));
    click(&mut session, at, 1);
    assert_eq!(session.tool(), AnnotationTool::Line);
    assert!(session.document().objects().is_empty());
    assert!(!session.document().can_undo());
}

#[test]
fn tool_press_released_on_the_canvas_never_activates() {
    let mut session = ready();
    let at = center(&session, Target::Tool(AnnotationTool::Line));
    session.pointer(
        1,
        PointerInput::Down {
            root: at,
            time: Duration::ZERO,
        },
    );
    let canvas = canvas_point(&session);
    session.pointer(
        1,
        PointerInput::Up {
            root: canvas,
            time: Duration::from_millis(10),
        },
    );
    assert_eq!(session.tool(), AnnotationTool::Pen);
    assert!(session.document().objects().is_empty());
}

#[test]
fn disabled_actions_consume_presses_without_touching_the_draft() {
    let mut session = ready();
    start_draft(&mut session);
    for target in [Target::Undo, Target::Delete, Target::Clear] {
        let at = center(&session, target);
        click(&mut session, at, 1);
        assert!(
            session.draft_id().is_some(),
            "disabled {target:?} cancelled the draft"
        );
        assert!(session.document().objects().is_empty());
        assert!(!session.document().can_undo());
    }
}

#[test]
fn canvas_stroke_completes_across_the_toolbar() {
    let mut session = ready();
    session
        .command(AnnotationCommand::Tool(AnnotationTool::Line))
        .unwrap();
    let from = canvas_point(&session);
    let release = center(&session, Target::Handle);
    session.pointer(
        1,
        PointerInput::Down {
            root: from,
            time: Duration::ZERO,
        },
    );
    session.pointer(
        1,
        PointerInput::Move {
            root: release,
            time: Duration::from_millis(10),
        },
    );
    session.pointer(
        1,
        PointerInput::Up {
            root: release,
            time: Duration::from_millis(20),
        },
    );
    assert_eq!(session.document().objects().len(), 1);
    let object = &session.document().objects()[0];
    match &object.data {
        ObjectData::Line { end, .. } => {
            let endpoint = Point {
                x: object.origin.x + end.x,
                y: object.origin.y + end.y,
            };
            assert_eq!(
                endpoint, release,
                "line must complete at the release position"
            );
        }
        other => panic!("unexpected object {other:?}"),
    }
}

#[test]
fn handle_drag_moves_the_toolbar_without_touching_document_or_history() {
    let mut session = ready();
    let before = session.toolbar_layout().unwrap().bar;
    let handle = center(&session, Target::Handle);
    let to = canvas_point(&session);
    session.pointer(
        1,
        PointerInput::Down {
            root: handle,
            time: Duration::ZERO,
        },
    );
    session.pointer(
        1,
        PointerInput::Move {
            root: to,
            time: Duration::from_millis(10),
        },
    );
    session.pointer(
        1,
        PointerInput::Up {
            root: to,
            time: Duration::from_millis(20),
        },
    );
    let after = session.toolbar_layout().unwrap().bar;
    assert_ne!(after, before, "toolbar must move with the drag");
    assert!(after.x >= 0 && after.right() <= 1280 && after.y >= 0 && after.bottom() <= 720);
    assert!(session.document().objects().is_empty());
    assert!(!session.document().can_undo());
}

#[test]
fn outside_click_closes_an_open_picker_and_leaves_the_draft_unchanged() {
    let mut session = ready();
    start_draft(&mut session);
    let draft = session.draft_id();
    let at = center(&session, Target::Picker(PickerKind::Color));
    click(&mut session, at, 1);
    assert!(session.toolbar_layout().unwrap().picker.is_some());
    let canvas = canvas_point(&session);
    click(&mut session, canvas, 1);
    assert!(session.toolbar_layout().unwrap().picker.is_none());
    assert_eq!(session.draft_id(), draft);
    assert!(session.document().objects().is_empty());
}

#[test]
fn outside_click_closes_an_open_picker_and_keeps_the_selection() {
    let mut session = ready();
    session
        .command(AnnotationCommand::Tool(AnnotationTool::Line))
        .unwrap();
    let from = canvas_point(&session);
    let to = Point {
        x: from.x + 100,
        y: from.y,
    };
    drag_line(&mut session, from, to);
    session
        .command(AnnotationCommand::Tool(AnnotationTool::Select))
        .unwrap();
    click(
        &mut session,
        Point {
            x: from.x + 50,
            y: from.y,
        },
        1,
    );
    assert!(session.selection().is_some());
    session
        .command(AnnotationCommand::Tool(AnnotationTool::Text))
        .unwrap();
    let at = center(&session, Target::Picker(PickerKind::Color));
    click(&mut session, at, 1);
    assert!(session.toolbar_layout().unwrap().picker.is_some());
    let canvas = canvas_point(&session);
    click(&mut session, canvas, 1);
    assert!(session.toolbar_layout().unwrap().picker.is_none());
    assert!(session.selection().is_some());
}

#[test]
fn escape_closes_an_open_picker_before_the_draft() {
    let mut session = ready();
    start_draft(&mut session);
    let at = center(&session, Target::Picker(PickerKind::Color));
    click(&mut session, at, 1);
    assert!(session.toolbar_layout().unwrap().picker.is_some());
    escape(&mut session, 1);
    assert!(session.toolbar_layout().unwrap().picker.is_none());
    assert!(
        session.draft_id().is_some(),
        "escape must not cancel the draft first"
    );
    assert!(session.status().editing_requested);
}

#[test]
fn escape_restores_a_drag_and_consumes_the_rest_of_the_sequence() {
    let mut session = ready();
    start_draft(&mut session);
    let before = session.toolbar_layout().unwrap().bar;
    let handle = center(&session, Target::Handle);
    session.pointer(
        1,
        PointerInput::Down {
            root: handle,
            time: Duration::ZERO,
        },
    );
    let moved = canvas_point(&session);
    session.pointer(
        1,
        PointerInput::Move {
            root: moved,
            time: Duration::from_millis(10),
        },
    );
    assert_ne!(
        session.toolbar_layout().unwrap().bar,
        before,
        "drag must move the bar"
    );
    escape(&mut session, 1);
    assert_eq!(
        session.toolbar_layout().unwrap().bar,
        before,
        "escape must restore the origin"
    );
    // Leaked events would reach the draft editor and commit the draft.
    let away = Point { x: 100, y: 600 };
    session.pointer(
        1,
        PointerInput::Move {
            root: away,
            time: Duration::from_millis(20),
        },
    );
    session.pointer(
        1,
        PointerInput::Up {
            root: away,
            time: Duration::from_millis(30),
        },
    );
    assert_eq!(session.toolbar_layout().unwrap().bar, before);
    assert!(
        session.draft_id().is_some(),
        "canvas events leaked past the cancelled drag"
    );
    assert!(session.document().objects().is_empty());
}

#[test]
fn ownership_loss_cancels_a_pending_toolbar_press() {
    let mut session = ready();
    let at = center(&session, Target::Tool(AnnotationTool::Line));
    session.pointer(
        1,
        PointerInput::Down {
            root: at,
            time: Duration::ZERO,
        },
    );
    session.ownership(None);
    session.pointer(
        1,
        PointerInput::Up {
            root: at,
            time: Duration::from_millis(10),
        },
    );
    assert_eq!(session.tool(), AnnotationTool::Pen);
    assert!(session.document().objects().is_empty());
}

#[test]
fn input_from_an_obsolete_epoch_changes_nothing() {
    let mut session = ready();
    let before = session.toolbar_layout().unwrap().bar;
    let handle = center(&session, Target::Handle);
    session.pointer(
        99,
        PointerInput::Down {
            root: handle,
            time: Duration::ZERO,
        },
    );
    session.pointer(
        99,
        PointerInput::Move {
            root: Point { x: 50, y: 50 },
            time: Duration::from_millis(10),
        },
    );
    session.pointer(
        99,
        PointerInput::Up {
            root: Point { x: 50, y: 50 },
            time: Duration::from_millis(20),
        },
    );
    assert_eq!(session.toolbar_layout().unwrap().bar, before);
    assert!(session.toolbar_layout().unwrap().picker.is_none());
    assert!(session.document().objects().is_empty());
    assert_eq!(session.tool(), AnnotationTool::Pen);
}

#[test]
fn clear_then_undo_restores_objects_exactly() {
    let mut session = ready();
    session
        .command(AnnotationCommand::Tool(AnnotationTool::Line))
        .unwrap();
    let first = canvas_point(&session);
    drag_line(
        &mut session,
        first,
        Point {
            x: first.x + 80,
            y: first.y + 10,
        },
    );
    let second = Point {
        x: first.x + 40,
        y: first.y + 60,
    };
    drag_line(
        &mut session,
        second,
        Point {
            x: second.x + 80,
            y: second.y + 10,
        },
    );
    let snapshot = session.document().objects().to_vec();
    assert_eq!(snapshot.len(), 2);

    let clear = center(&session, Target::Clear);
    click(&mut session, clear, 1);
    assert!(session.document().objects().is_empty());
    assert!(session.document().can_undo());

    let undo = center(&session, Target::Undo);
    click(&mut session, undo, 1);
    assert_eq!(session.document().objects(), snapshot.as_slice());
    assert!(
        session.document().can_redo(),
        "the cleared state stays redoable"
    );
}

#[test]
fn exit_hides_the_toolbar_and_keeps_committed_content() {
    let mut session = ready();
    session
        .command(AnnotationCommand::Tool(AnnotationTool::Line))
        .unwrap();
    let from = canvas_point(&session);
    drag_line(
        &mut session,
        from,
        Point {
            x: from.x + 80,
            y: from.y + 40,
        },
    );
    assert_eq!(session.document().objects().len(), 1);

    let exit = center(&session, Target::Exit);
    click(&mut session, exit, 1);
    assert!(session.toolbar_layout().is_none());
    assert!(!session.status().editing_requested);
    assert!(session.status().has_content);
    assert_eq!(session.document().objects().len(), 1);
}

#[test]
fn choosing_a_tool_button_cancels_the_uncommitted_draft() {
    let mut session = ready();
    start_draft(&mut session);
    let at = center(&session, Target::Tool(AnnotationTool::Line));
    click(&mut session, at, 1);
    assert_eq!(session.tool(), AnnotationTool::Line);
    assert!(session.draft_id().is_none());
}

#[test]
fn choosing_a_preset_value_cancels_the_uncommitted_draft() {
    let mut session = ready();
    start_draft(&mut session);
    let open = center(&session, Target::Picker(PickerKind::TextSize));
    click(&mut session, open, 1);
    assert!(session.toolbar_layout().unwrap().picker.is_some());
    let value = center(&session, Target::TextSize(48));
    click(&mut session, value, 1);
    assert_eq!(session.text_size_px(), 48);
    assert!(session.draft_id().is_none());
    assert!(session.toolbar_layout().unwrap().picker.is_none());
}

#[test]
fn dragging_the_handle_with_a_picker_open_issues_no_command() {
    let mut session = ready();
    let open = center(&session, Target::Picker(PickerKind::Color));
    click(&mut session, open, 1);
    assert!(session.toolbar_layout().unwrap().picker.is_some());
    let before = session.toolbar_layout().unwrap().bar;
    let handle = center(&session, Target::Handle);
    let to = canvas_point(&session);
    session.pointer(
        1,
        PointerInput::Down {
            root: handle,
            time: Duration::ZERO,
        },
    );
    assert!(
        session.toolbar_layout().unwrap().picker.is_none(),
        "handle press dismisses the picker"
    );
    session.pointer(
        1,
        PointerInput::Move {
            root: to,
            time: Duration::from_millis(10),
        },
    );
    session.pointer(
        1,
        PointerInput::Up {
            root: to,
            time: Duration::from_millis(20),
        },
    );
    assert_eq!(
        session.tool(),
        AnnotationTool::Pen,
        "dragging must not run a command"
    );
    assert!(session.document().objects().is_empty());
    assert!(!session.document().can_undo());
    assert_ne!(session.toolbar_layout().unwrap().bar, before);
}

#[test]
fn keyboard_ctrl_z_edits_the_draft_not_document_history() {
    let mut session = ready();
    session
        .command(AnnotationCommand::Tool(AnnotationTool::Line))
        .unwrap();
    let from = Point { x: 200, y: 500 };
    drag_line(&mut session, from, Point { x: 300, y: 500 });
    assert_eq!(session.document().objects().len(), 1);

    start_draft(&mut session);
    type_char(&mut session, 'a');
    assert_eq!(session.text_edit.as_ref().unwrap().draft.plain_text(), "a");

    session.key(
        1,
        KeyInput {
            key: Key::Char('z'),
            text: None,
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            shortcut: None,
        },
    );
    assert!(
        session.draft_id().is_some(),
        "ctrl+z must stay local to the draft"
    );
    assert_eq!(session.text_edit.as_ref().unwrap().draft.plain_text(), "");
    assert_eq!(
        session.document().objects().len(),
        1,
        "document history must be untouched"
    );
}
