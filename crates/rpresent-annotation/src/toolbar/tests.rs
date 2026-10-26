use super::*;
use crate::{AnnotationCommand, Session};
use rpresent_config::AnnotationConfig;

fn geometry() -> DisplayGeometry {
    DisplayGeometry {
        root: Rect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1080,
        },
        monitors: vec![
            Rect {
                x: 0,
                y: 0,
                width: 1280,
                height: 1080,
            },
            Rect {
                x: 1280,
                y: 0,
                width: 1280,
                height: 1080,
            },
        ],
        pointer: Point { x: 1800, y: 400 },
    }
}

fn editing_session() -> Session {
    let mut session = Session::new(
        &AnnotationConfig::default(),
        &crate::session::default_bindings(),
    )
    .unwrap();
    session.command(AnnotationCommand::On).unwrap();
    session
}

fn contains_rect(outer: &Rect, inner: &Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

/// Every control must sit fully inside the bar or the picker that contains
/// it.
fn assert_controls_fit(layout: &ToolbarLayout) {
    for control in &layout.controls {
        if matches!(
            control.target,
            Target::Color(_)
                | Target::Width(_)
                | Target::TextSize(_)
                | Target::PreviousPage
                | Target::NextPage
        ) {
            let picker = layout.picker.expect("picker controls need a picker rect");
            assert!(
                contains_rect(&picker, &control.bounds),
                "picker control {:?} outside {picker:?}",
                control.target
            );
        } else {
            assert!(
                contains_rect(&layout.bar, &control.bounds),
                "bar control {:?} outside {:?}",
                control.target,
                layout.bar
            );
        }
    }
}

fn picker_targets(layout: &ToolbarLayout) -> Vec<Target> {
    layout
        .controls
        .iter()
        .filter_map(|control| match control.target {
            Target::Picker(kind) => Some(Target::Picker(kind)),
            _ => None,
        })
        .collect()
}

fn selected_tool(layout: &ToolbarLayout) -> Vec<AnnotationTool> {
    layout
        .controls
        .iter()
        .filter_map(|control| match control.target {
            Target::Tool(tool) if control.selected => Some(tool),
            _ => None,
        })
        .collect()
}

#[test]
fn narrow_monitor_wraps_groups_and_keeps_controls_inside() {
    let session = editing_session();
    let geometry = DisplayGeometry {
        root: Rect {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        },
        monitors: vec![Rect {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        }],
        pointer: Point { x: 320, y: 240 },
    };
    let mut toolbar = Toolbar::new(geometry);
    assert!(toolbar.sync(&session.toolbar_values()).is_some());
    let layout = toolbar.layout().unwrap();
    assert!(
        layout.bar.height > ROW_PX + 2 * PADDING_PX as u32,
        "expected wrapped rows"
    );
    assert!(layout.bar.width <= 640);
    assert_controls_fit(layout);
}

#[test]
fn empty_monitor_list_falls_back_to_root_bounds() {
    let session = editing_session();
    let geometry = DisplayGeometry {
        root: Rect {
            x: 0,
            y: 0,
            width: 1024,
            height: 768,
        },
        monitors: vec![Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }],
        pointer: Point { x: 500, y: 400 },
    };
    let mut toolbar = Toolbar::new(geometry);
    toolbar.sync(&session.toolbar_values());
    let bar = toolbar.layout().unwrap().bar;
    assert!(bar.x >= 0 && bar.right() <= 1024);
    assert!(bar.y >= 0 && bar.bottom() <= 768);
    assert_eq!(bar.y, 12);
}

fn offset_geometry(pointer: Point) -> DisplayGeometry {
    DisplayGeometry {
        root: Rect {
            x: 0,
            y: 0,
            width: 1800,
            height: 900,
        },
        monitors: vec![
            Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
            Rect {
                x: 901,
                y: 200,
                width: 800,
                height: 600,
            },
        ],
        pointer,
    }
}

#[test]
fn offset_monitors_take_the_toolbar_from_the_pointer_monitor() {
    let session = editing_session();
    let mut toolbar = Toolbar::new(offset_geometry(Point { x: 1000, y: 300 }));
    toolbar.sync(&session.toolbar_values());
    let bar = toolbar.layout().unwrap().bar;
    assert!(bar.x >= 901 && bar.right() <= 1701);
    assert!(bar.y >= 200 && bar.bottom() <= 800);
}

#[test]
fn pointer_in_a_gap_uses_the_nearest_monitor_and_ties_use_reported_order() {
    let session = editing_session();

    // Closer to the second monitor's edge.
    let mut toolbar = Toolbar::new(offset_geometry(Point { x: 870, y: 300 }));
    toolbar.sync(&session.toolbar_values());
    let bar = toolbar.layout().unwrap().bar;
    assert!(
        bar.x >= 901 && bar.right() <= 1701,
        "expected the second monitor"
    );

    // Equal distance to both monitors: the first reported monitor wins.
    let mut toolbar = Toolbar::new(offset_geometry(Point { x: 850, y: 300 }));
    toolbar.sync(&session.toolbar_values());
    let bar = toolbar.layout().unwrap().bar;
    assert!(
        bar.x >= 0 && bar.right() <= 800,
        "expected the first monitor"
    );
}

#[test]
fn width_picker_pages_cover_every_configured_value_in_order() {
    let config = AnnotationConfig {
        stroke_widths_px: (1..=200).collect(),
        stroke_width_px: 1,
        ..Default::default()
    };
    let mut session = Session::new(&config, &crate::session::default_bindings()).unwrap();
    session.command(AnnotationCommand::On).unwrap();

    let geometry = DisplayGeometry {
        root: Rect {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        },
        monitors: Vec::new(),
        pointer: Point { x: 320, y: 240 },
    };
    let values = session.toolbar_values();
    let origin = Point { x: 0, y: 12 };
    let pages = layout::picker_page_count(&geometry, origin, &values, PickerKind::Width);
    assert!(pages > 1, "200 values must page");

    let mut seen = Vec::new();
    for page in 0..pages {
        let built = layout::build_layout(&geometry, origin, &values, Some(PickerKind::Width), page);
        assert_controls_fit(&built);
        assert!(
            built
                .controls
                .iter()
                .any(|control| control.target == Target::PreviousPage)
        );
        assert!(
            built
                .controls
                .iter()
                .any(|control| control.target == Target::NextPage)
        );
        for control in &built.controls {
            if let Target::Width(width) = control.target {
                seen.push(width);
            }
        }
    }
    let expected: Vec<u32> = (1..=200).collect();
    assert_eq!(
        seen, expected,
        "every configured width appears once, in order"
    );
}

#[test]
fn single_page_picker_shows_no_page_controls() {
    let session = editing_session();
    let geometry = DisplayGeometry {
        root: Rect {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        },
        monitors: Vec::new(),
        pointer: Point { x: 640, y: 360 },
    };
    let mut toolbar = Toolbar::new(geometry);
    toolbar.picker = Some(PickerKind::Width);
    assert!(toolbar.sync(&session.toolbar_values()).is_some());
    let layout = toolbar.layout().unwrap();
    assert!(layout.picker.is_some());
    assert!(
        !layout
            .controls
            .iter()
            .any(|control| matches!(control.target, Target::PreviousPage | Target::NextPage))
    );
    assert_controls_fit(layout);
}

#[test]
fn visibility_changes_return_damage_but_keep_the_remembered_origin() {
    let mut session = Session::new(
        &AnnotationConfig::default(),
        &crate::session::default_bindings(),
    )
    .unwrap();
    let mut toolbar = Toolbar::new(geometry());

    assert!(toolbar.sync(&session.toolbar_values()).is_none());

    session.command(AnnotationCommand::On).unwrap();
    assert!(toolbar.sync(&session.toolbar_values()).is_some());
    let origin = toolbar.origin.expect("first activation sets the origin");

    session.command(AnnotationCommand::Off).unwrap();
    assert!(toolbar.sync(&session.toolbar_values()).is_some());
    assert!(toolbar.layout().is_none());
    assert_eq!(toolbar.origin, Some(origin));

    session.command(AnnotationCommand::On).unwrap();
    assert!(toolbar.sync(&session.toolbar_values()).is_some());
    assert_eq!(toolbar.origin, Some(origin));
    let bar = toolbar.layout().unwrap().bar;
    assert_eq!(Point { x: bar.x, y: bar.y }, origin);
    assert_controls_fit(toolbar.layout().unwrap());
}

#[test]
fn tool_changes_swap_applicable_presets_and_highlight() {
    let mut session = Session::new(
        &AnnotationConfig::default(),
        &crate::session::default_bindings(),
    )
    .unwrap();
    session.command(AnnotationCommand::On).unwrap();
    let mut toolbar = Toolbar::new(geometry());
    toolbar.sync(&session.toolbar_values());

    // Default tool is Arrow: color and width apply, Arrow is highlighted.
    assert_eq!(
        picker_targets(toolbar.layout().unwrap()),
        vec![
            Target::Picker(PickerKind::Color),
            Target::Picker(PickerKind::Width)
        ]
    );
    assert_eq!(
        selected_tool(toolbar.layout().unwrap()),
        vec![AnnotationTool::Arrow]
    );

    session
        .command(AnnotationCommand::Tool(AnnotationTool::Text))
        .unwrap();
    assert!(toolbar.sync(&session.toolbar_values()).is_some());
    assert_eq!(
        picker_targets(toolbar.layout().unwrap()),
        vec![
            Target::Picker(PickerKind::Color),
            Target::Picker(PickerKind::TextSize)
        ]
    );
    assert_eq!(
        selected_tool(toolbar.layout().unwrap()),
        vec![AnnotationTool::Text]
    );

    session
        .command(AnnotationCommand::Tool(AnnotationTool::Arrow))
        .unwrap();
    assert!(toolbar.sync(&session.toolbar_values()).is_some());
    assert_eq!(
        picker_targets(toolbar.layout().unwrap()),
        vec![
            Target::Picker(PickerKind::Color),
            Target::Picker(PickerKind::Width)
        ]
    );
    assert_eq!(
        selected_tool(toolbar.layout().unwrap()),
        vec![AnnotationTool::Arrow]
    );
    assert_controls_fit(toolbar.layout().unwrap());

    session
        .command(AnnotationCommand::Tool(AnnotationTool::Rect))
        .unwrap();
    assert!(toolbar.sync(&session.toolbar_values()).is_some());
    assert_eq!(
        picker_targets(toolbar.layout().unwrap()),
        vec![
            Target::Picker(PickerKind::Color),
            Target::Picker(PickerKind::Width)
        ]
    );
    assert_eq!(
        selected_tool(toolbar.layout().unwrap()),
        vec![AnnotationTool::Rect]
    );
    assert_controls_fit(toolbar.layout().unwrap());

    session
        .command(AnnotationCommand::Tool(AnnotationTool::Select))
        .unwrap();
    assert!(toolbar.sync(&session.toolbar_values()).is_some());
    assert!(picker_targets(toolbar.layout().unwrap()).is_empty());
    assert_eq!(
        selected_tool(toolbar.layout().unwrap()),
        vec![AnnotationTool::Select]
    );
    assert_controls_fit(toolbar.layout().unwrap());
}

#[test]
fn toolbar_starts_on_pointer_monitor_without_creating_content() {
    let mut session = Session::new(
        &AnnotationConfig::default(),
        &crate::session::default_bindings(),
    )
    .unwrap();
    session.command(AnnotationCommand::On).unwrap();
    let mut toolbar = Toolbar::new(geometry());
    assert!(toolbar.sync(&session.toolbar_values()).is_some());
    let bar = toolbar.layout().unwrap().bar;
    assert!(bar.x >= 1280);
    assert!(bar.right() <= 2560);
    assert_eq!(bar.y, 12);
    let tools: Vec<_> = toolbar
        .layout()
        .unwrap()
        .controls
        .iter()
        .filter_map(|control| match control.target {
            Target::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();
    assert_eq!(
        tools,
        vec![
            AnnotationTool::Select,
            AnnotationTool::Pen,
            AnnotationTool::Line,
            AnnotationTool::Arrow,
            AnnotationTool::Rect,
            AnnotationTool::Eraser,
            AnnotationTool::Text,
        ]
    );
    assert!(session.document().objects().is_empty());
    assert!(toolbar.sync(&session.toolbar_values()).is_none());
}
