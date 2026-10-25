use super::*;
use crate::toolbar::{DisplayGeometry, PickerKind, Target};
use crate::{AnnotationCommand, AnnotationTool, PointerInput};
use rpresent_config::AnnotationConfig;
use std::time::Duration;

fn geometry(width: u32, height: u32) -> DisplayGeometry {
    DisplayGeometry {
        root: Rect { x: 0, y: 0, width, height },
        monitors: Vec::new(),
        pointer: crate::Point { x: width as i32 / 2, y: height as i32 / 2 },
    }
}

fn base_scene(config: &AnnotationConfig, width: u32, height: u32) -> Session {
    let mut session = Session::new(config).unwrap();
    session.text_system = crate::TextSystem::with_font(include_bytes!(
        "../../tests/fixtures/DejaVuSans.ttf"
    ));
    session.command(AnnotationCommand::On).unwrap();
    session.ownership(Some(1));
    session
}

fn scene() -> Session {
    scene_with(&AnnotationConfig::default(), 1280, 720)
}

fn scene_with(config: &AnnotationConfig, width: u32, height: u32) -> Session {
    let mut session = Session::new(config).unwrap();
    session.text_system = crate::TextSystem::with_font(include_bytes!(
        "../../tests/fixtures/DejaVuSans.ttf"
    ));
    session.update_display(geometry(width, height));
    session.command(AnnotationCommand::On).unwrap();
    session.ownership(Some(1));
    session
}

fn parity(session: &mut Session, partial: &mut Renderer, full: &mut Renderer, label: &str) {
    if let Some(damage) = session.take_damage() {
        partial.redraw(session, damage).unwrap();
    }
    let bounds = full.full_bounds();
    full.redraw(session, bounds).unwrap();
    assert_eq!(partial.pixels(), full.pixels(), "{label}");
}

fn control_center(session: &Session, target: Target) -> crate::Point {
    let layout = session.toolbar_layout().unwrap();
    let bounds = layout
        .controls
        .iter()
        .find(|control| control.target == target)
        .unwrap_or_else(|| panic!("missing control {target:?}"))
        .bounds;
    crate::Point {
        x: bounds.x + bounds.width as i32 / 2,
        y: bounds.y + bounds.height as i32 / 2,
    }
}

fn click(session: &mut Session, at: crate::Point, millis: u64) {
    session.pointer(1, PointerInput::Down { root: at, time: Duration::from_millis(millis) });
    session.pointer(1, PointerInput::Up { root: at, time: Duration::from_millis(millis + 10) });
}

fn press(session: &mut Session, keysym: u32) {
    session.key(
        1,
        crate::KeyInput {
            keysym,
            text: None,
            modifiers: crate::Modifiers::default(),
        },
    );
}

fn drag(session: &mut Session, from: crate::Point, to: crate::Point, millis: u64) {
    session.pointer(1, PointerInput::Down { root: from, time: Duration::from_millis(millis) });
    session.pointer(1, PointerInput::Move { root: to, time: Duration::from_millis(millis + 5) });
    session.pointer(1, PointerInput::Up { root: to, time: Duration::from_millis(millis + 10) });
}

fn below_bar(session: &Session) -> crate::Point {
    let layout = session.toolbar_layout().unwrap();
    let mut bottom = layout.bar.bottom();
    if let Some(picker) = layout.picker {
        bottom = bottom.max(picker.bottom());
    }
    crate::Point { x: 640, y: bottom + 50 }
}

#[test]
fn empty_editing_scene_paints_toolbar_without_document_objects() {
    let mut session = scene();
    let mut renderer = Renderer::new(1280, 720).unwrap();
    let bounds = renderer.full_bounds();
    renderer.redraw(&mut session, bounds).unwrap();
    let bar = session.toolbar_layout().unwrap().bar;
    let offset = ((bar.y as usize * 1280) + bar.x as usize) * 4;
    assert_ne!(renderer.pixels()[offset + 3], 0);
    assert!(session.document().objects().is_empty());
    assert!(!session.status().has_content);
}

#[test]
fn moving_the_toolbar_repaints_the_artwork_underneath() {
    // Committed line across the future toolbar area, created before any
    // geometry exists.
    let mut session = base_scene(&AnnotationConfig::default(), 1280, 720);
    session.command(AnnotationCommand::Tool(AnnotationTool::Line)).unwrap();
    drag(
        &mut session,
        crate::Point { x: 200, y: 40 },
        crate::Point { x: 1100, y: 40 },
        0,
    );
    assert_eq!(session.document().objects().len(), 1);
    assert!(session.toolbar_layout().is_none(), "no geometry yet");

    let mut partial = Renderer::new(1280, 720).unwrap();
    let whole = partial.full_bounds();
    partial.redraw(&mut session, whole).unwrap();
    session.take_damage();
    let mut full = Renderer::new(1280, 720).unwrap();

    // The toolbar appears over the line: partial damage must equal a full
    // frame.
    session.update_display(geometry(1280, 720));
    parity(&mut session, &mut partial, &mut full, "toolbar appears over artwork");
    let old = session.toolbar_layout().unwrap().bar;
    let offset = ((old.y as usize * 1280) + old.x as usize) * 4;
    assert_ne!(partial.pixels()[offset + 3], 0, "toolbar must cover the artwork");

    // Drag the toolbar away; the old position must show the original line.
    let handle = control_center(&session, Target::Handle);
    let moved = crate::Point { x: handle.x, y: 300 };
    drag(&mut session, handle, moved, 100);
    parity(&mut session, &mut partial, &mut full, "toolbar moved");
    let new = session.toolbar_layout().unwrap().bar;
    assert_ne!(new, old);

    let probe_x = old.x + old.width as i32 / 2;
    let line_offset = ((40usize * 1280) + probe_x as usize) * 4;
    let pixel = &partial.pixels()[line_offset..line_offset + 4];
    assert_eq!(pixel[3], 255, "old bar position must be opaque again");
    assert!(
        pixel[0] > 200 && pixel[1] < 80,
        "expected the red line under the old bar position, got {pixel:?}"
    );
}

#[test]
fn every_toolbar_transition_keeps_partial_frames_identical_to_full_frames() {
    let mut config = AnnotationConfig::default();
    config.text_sizes_px = (16..=46).collect();
    config.text_size_px = 24;
    let mut session = scene_with(&config, 1280, 720);
    let mut partial = Renderer::new(1280, 720).unwrap();
    let whole = partial.full_bounds();
    partial.redraw(&mut session, whole).unwrap();
    session.take_damage();
    let mut full = Renderer::new(1280, 720).unwrap();

    // Committed artwork below the bar.
    session.command(AnnotationCommand::Tool(AnnotationTool::Line)).unwrap();
    parity(&mut session, &mut partial, &mut full, "tool change");
    let from = below_bar(&session);
    drag(
        &mut session,
        from,
        crate::Point { x: from.x + 80, y: from.y + 10 },
        0,
    );
    parity(&mut session, &mut partial, &mut full, "line drawn");

    // Tool changes.
    for tool in [
        AnnotationTool::Pen,
        AnnotationTool::Eraser,
        AnnotationTool::Select,
        AnnotationTool::Line,
        AnnotationTool::Pen,
    ] {
        session.command(AnnotationCommand::Tool(tool)).unwrap();
        parity(&mut session, &mut partial, &mut full, "tool");
    }

    // Picker: open, replace, close.
    let at = control_center(&session, Target::Picker(PickerKind::Color));
    click(&mut session, at, 100);
    assert!(session.toolbar_layout().unwrap().picker.is_some());
    parity(&mut session, &mut partial, &mut full, "picker opened");
    let at = control_center(&session, Target::Picker(PickerKind::Width));
    click(&mut session, at, 200);
    parity(&mut session, &mut partial, &mut full, "picker replaced");
    press(&mut session, 0xff1b);
    parity(&mut session, &mut partial, &mut full, "picker closed");

    // Selection change.
    session.command(AnnotationCommand::Tool(AnnotationTool::Select)).unwrap();
    parity(&mut session, &mut partial, &mut full, "select tool");
    click(&mut session, crate::Point { x: from.x + 40, y: from.y + 5 }, 300);
    assert!(session.selection().is_some());
    parity(&mut session, &mut partial, &mut full, "selected");

    // Preset change through the picker while selected.
    session.command(AnnotationCommand::Tool(AnnotationTool::Pen)).unwrap();
    parity(&mut session, &mut partial, &mut full, "pen tool");
    let at = control_center(&session, Target::Picker(PickerKind::Color));
    click(&mut session, at, 400);
    parity(&mut session, &mut partial, &mut full, "color picker opened");
    let at = control_center(&session, Target::Color([255, 255, 0]));
    click(&mut session, at, 500);
    assert_eq!(session.color(), [255, 255, 0]);
    parity(&mut session, &mut partial, &mut full, "color changed");

    // History.
    session.command(AnnotationCommand::Undo).unwrap();
    parity(&mut session, &mut partial, &mut full, "undo");
    session.command(AnnotationCommand::Redo).unwrap();
    parity(&mut session, &mut partial, &mut full, "redo");
    session.command(AnnotationCommand::Clear).unwrap();
    parity(&mut session, &mut partial, &mut full, "clear");
    session.command(AnnotationCommand::Undo).unwrap();
    parity(&mut session, &mut partial, &mut full, "undo clear");

    session.command(AnnotationCommand::Clear).unwrap();
    parity(&mut session, &mut partial, &mut full, "cleared for draft");

    // A text draft underneath the open picker.
    session.command(AnnotationCommand::Tool(AnnotationTool::Text)).unwrap();
    parity(&mut session, &mut partial, &mut full, "text tool");
    let draft_at = crate::Point { x: 640, y: 130 };
    click(&mut session, draft_at, 600);
    assert!(session.draft_id().is_some());
    parity(&mut session, &mut partial, &mut full, "draft open");
    session.key(
        1,
        crate::KeyInput {
            keysym: 'a' as u32,
            text: Some('a'),
            modifiers: crate::Modifiers::default(),
        },
    );
    parity(&mut session, &mut partial, &mut full, "draft typed");
    let at = control_center(&session, Target::Picker(PickerKind::TextSize));
    click(&mut session, at, 700);
    assert!(session.toolbar_layout().unwrap().picker.is_some());
    parity(&mut session, &mut partial, &mut full, "picker over draft");
    press(&mut session, 0xff1b);
    parity(&mut session, &mut partial, &mut full, "picker closed over draft");
    press(&mut session, 0xff0d);
    parity(&mut session, &mut partial, &mut full, "draft committed");

    // Exit and reopen: the toolbar disappears and returns.
    session.command(AnnotationCommand::Off).unwrap();
    assert!(session.toolbar_layout().is_none());
    parity(&mut session, &mut partial, &mut full, "exit");
    session.command(AnnotationCommand::On).unwrap();
    parity(&mut session, &mut partial, &mut full, "reopened");
}

#[test]
fn paging_the_picker_keeps_partial_frames_identical() {
    let mut config = AnnotationConfig::default();
    config.stroke_widths_px = (1..=200).collect();
    config.stroke_width_px = 1;
    let mut session = scene_with(&config, 640, 480);
    let mut partial = Renderer::new(640, 480).unwrap();
    let whole = partial.full_bounds();
    partial.redraw(&mut session, whole).unwrap();
    session.take_damage();
    let mut full = Renderer::new(640, 480).unwrap();

    let at = control_center(&session, Target::Picker(PickerKind::Width));
    click(&mut session, at, 0);
    assert!(session.toolbar_layout().unwrap().picker.is_some());
    parity(&mut session, &mut partial, &mut full, "paged picker opened");

    for label in ["next", "next", "previous"] {
        let target = if label == "next" { Target::NextPage } else { Target::PreviousPage };
        let at = control_center(&session, target);
        click(&mut session, at, 100);
        parity(&mut session, &mut partial, &mut full, label);
    }
    press(&mut session, 0xff1b);
    parity(&mut session, &mut partial, &mut full, "paged picker closed");
}

#[test]
fn unchanged_toolbar_state_reports_no_damage() {
    let mut session = scene();
    let mut renderer = Renderer::new(1280, 720).unwrap();
    let whole = renderer.full_bounds();
    renderer.redraw(&mut session, whole).unwrap();
    session.take_damage();

    // A completed draw, then clear its damage.
    session.command(AnnotationCommand::Tool(AnnotationTool::Line)).unwrap();
    let from = below_bar(&session);
    drag(
        &mut session,
        from,
        crate::Point { x: from.x + 40, y: from.y },
        0,
    );
    assert!(session.take_damage().is_some());
    assert!(session.toolbar_layout().is_some());

    // A no-op command and an unchanged geometry refresh report nothing.
    session.command(AnnotationCommand::Tool(AnnotationTool::Line)).unwrap();
    assert!(session.take_damage().is_none(), "no-op command must not damage the toolbar");
    session.update_display(geometry(1280, 720));
    assert!(session.take_damage().is_none(), "unchanged geometry must not damage the toolbar");
}
