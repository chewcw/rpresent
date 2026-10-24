use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use rpresent_config::{Config, ConfigService};

use crate::{ZoomCommand, ZoomRequest, channel, run_overlay};

#[test]
#[ignore = "requires a private X11 display with GLX"]
fn native_activation_publishes_only_after_a_real_window_is_ready() {
    let config = Arc::new(ConfigService::new(
        PathBuf::from("native-test.toml"),
        Config::default(),
    ));
    let (handle, inbox) = channel().expect("zoom transport");
    let runtime = thread::spawn(move || run_overlay(config, inbox));

    handle
        .send(ZoomRequest::Command(ZoomCommand::On))
        .expect("activate zoom");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.is_active() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !handle.is_active() {
        drop(handle);
        let result = runtime.join().expect("native runtime thread");
        panic!("native zoom activation did not complete: {result:?}");
    }

    handle
        .send(ZoomRequest::Command(ZoomCommand::Off))
        .expect("close zoom");
    drop(handle);
    runtime
        .join()
        .expect("native runtime thread")
        .expect("native runtime");
}

#[test]
#[ignore = "requires a private X11 display with GLX"]
fn native_capture_and_glx_preserve_row_colors() {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ChangeGCAux, ConnectionExt as _, CreateGCAux, Rectangle};

    use super::capture_frame;
    use crate::glx::GlxRenderer;
    use crate::session::MonitorRect;

    let (connection, screen_num) = x11rb::connect(None).unwrap();
    let screen = &connection.setup().roots[screen_num];
    let root_visual = screen
        .allowed_depths
        .iter()
        .flat_map(|depth| &depth.visuals)
        .find(|visual| visual.visual_id == screen.root_visual)
        .unwrap();
    assert_eq!(
        screen.root_depth, 24,
        "run with the runner's 24-bit Xvfb screen"
    );
    assert_eq!(
        (
            root_visual.red_mask,
            root_visual.green_mask,
            root_visual.blue_mask
        ),
        (0xff0000, 0x00ff00, 0x0000ff),
    );

    for width in [7_u16, 1920_u16] {
        let height = 3_u16;
        let pixmap = connection.generate_id().unwrap();
        let gc = connection.generate_id().unwrap();
        connection
            .create_pixmap(screen.root_depth, pixmap, screen.root, width, height)
            .unwrap()
            .check()
            .unwrap();
        connection
            .create_gc(gc, pixmap, &CreateGCAux::new())
            .unwrap()
            .check()
            .unwrap();

        let rows = [0xff0000_u32, 0x00ff00, 0x0000ff];
        let markers = [0xffff00_u32, 0xffffff, 0xff00ff];
        let mut expected = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let color = if x == width - 1 {
                    markers[usize::from(y)]
                } else {
                    rows[usize::from(y)]
                };
                connection
                    .change_gc(gc, &ChangeGCAux::new().foreground(color))
                    .unwrap();
                connection
                    .poly_fill_rectangle(
                        pixmap,
                        gc,
                        &[Rectangle {
                            x: x as i16,
                            y: y as i16,
                            width: 1,
                            height: 1,
                        }],
                    )
                    .unwrap();
                expected.extend_from_slice(&[
                    color as u8,
                    (color >> 8) as u8,
                    (color >> 16) as u8,
                    255,
                ]);
            }
        }

        let frame = capture_frame(
            &connection,
            screen_num,
            pixmap,
            MonitorRect {
                x: 0,
                y: 0,
                width: u32::from(width),
                height: u32::from(height),
            },
        )
        .unwrap();
        connection.free_gc(gc).unwrap().check().unwrap();
        connection.free_pixmap(pixmap).unwrap().check().unwrap();
        assert_eq!(frame.bgra, expected, "capture pixels at width {width}");

        let mut renderer = unsafe { GlxRenderer::new(screen_num as i32).unwrap() };
        let window = renderer
            .create_window(
                screen.root.into(),
                0,
                0,
                u32::from(width),
                u32::from(height),
                screen.black_pixel,
            )
            .unwrap();
        renderer.attach(window).unwrap();
        renderer.upload_frame(&frame).unwrap();
        renderer.map_window();
        renderer
            .draw(1.0, (0.0, 0.0), (u32::from(width), u32::from(height)))
            .unwrap();
        renderer.wait_complete();

        let (rendered, visual_id) =
            x11rb::image::Image::get(&connection, window, 0, 0, width, height).unwrap();
        let visual = screen
            .allowed_depths
            .iter()
            .flat_map(|depth| &depth.visuals)
            .find(|visual| visual.visual_id == visual_id)
            .unwrap();
        assert_eq!(
            (visual.red_mask, visual.green_mask, visual.blue_mask),
            (0xff0000, 0x00ff00, 0x0000ff),
        );
        for y in 0..height {
            for x in 0..width {
                let color = if x == width - 1 {
                    markers[usize::from(y)]
                } else {
                    rows[usize::from(y)]
                };
                assert_eq!(
                    rendered.get_pixel(x, y) & 0x00ffffff,
                    color,
                    "rendered pixel ({x}, {y}) at width {width}",
                );
            }
        }
    }
}
