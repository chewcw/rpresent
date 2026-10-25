use super::*;

#[test]
#[ignore = "requires RPRESENT_NATIVE_TEST=1 and a private composited X11 display"]
fn native_toolbar_keeps_one_click_through_window() {
    assert_eq!(std::env::var("RPRESENT_NATIVE_TEST").as_deref(), Ok("1"));
    let mut backend = X11AnnotationBackend::new().unwrap();
    let window = backend.window_id();
    let geometry = backend.geometry().unwrap();
    let mut session = crate::Session::new(&rpresent_config::AnnotationConfig::default()).unwrap();
    session.update_display(geometry);
    session.command(crate::AnnotationCommand::On).unwrap();
    session.ownership(Some(1));
    let (width, height) = backend.size();
    let mut renderer = crate::Renderer::new(width, height).unwrap();
    let whole = renderer.full_bounds();
    renderer.redraw(&mut session, whole).unwrap();
    backend.present(true).unwrap();
    backend.upload(&renderer, whole).unwrap();
    backend.round_trip().unwrap();
    assert_eq!(backend.window_id(), window);

    // The toolbar must be visible as painted window pixels.
    let bar = session.toolbar_layout().unwrap().bar;
    let pixel = backend
        .connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            window,
            (bar.x + 2) as i16,
            (bar.y + 2) as i16,
            1,
            1,
            u32::MAX,
        )
        .unwrap()
        .reply()
        .unwrap();
    let alpha = match backend.byte_order {
        ByteOrder::Bgra => pixel.data[3],
        ByteOrder::Argb => pixel.data[0],
    };
    assert_ne!(alpha, 0, "toolbar bar must be opaque on screen");

    // One click through: the window keeps an empty input shape.
    let input = shape::get_rectangles(&backend.connection, window, SK::INPUT)
        .unwrap()
        .reply()
        .unwrap();
    assert!(input.rectangles.is_empty());
    assert!(session.toolbar_layout().is_some());
    assert!(session.document().objects().is_empty());

    // Hiding and showing reuse the same window identifier.
    backend.present(false).unwrap();
    backend.present(true).unwrap();
    assert_eq!(backend.window_id(), window);
    backend.present(false).unwrap();
}
