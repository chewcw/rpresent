use cosmic_text::Color as TextColor;
use tiny_skia::{Mask, Pixmap};

use crate::render::{blend_rect, draw_border, fill_clipped};
use crate::text::TextSystem;

use super::{GAP_PX, PADDING_PX, ROW_PX, SWATCH_PX, ToolbarControl, ToolbarLayout};

/// Font size for toolbar labels. UI labels never use the configured
/// annotation text-size presets.
const UI_FONT_PX: u32 = 14;
const BAR_BACKGROUND: [u8; 3] = [24, 24, 28];
const BORDER_COLOR: [u8; 3] = [92, 92, 102];
const ACCENT_COLOR: [u8; 3] = [64, 156, 255];
const TEXT_LIGHT: [u8; 3] = [236, 236, 240];
const TEXT_MUTED: [u8; 3] = [132, 132, 140];

/// Paint the toolbar and open picker last, clipped to the damaged region.
/// Purely presentational: it reads the layout and never touches the
/// session.
pub(crate) fn draw_toolbar(
    pixmap: &mut Pixmap,
    mask: &Mask,
    text_system: &mut TextSystem,
    layout: &ToolbarLayout,
    clip: &crate::Rect,
) {
    fill_clipped(pixmap, layout.bar, BAR_BACKGROUND, 255, clip);
    draw_border(pixmap, mask, layout.bar, BORDER_COLOR, clip);
    if let Some(picker) = layout.picker {
        fill_clipped(pixmap, picker, BAR_BACKGROUND, 255, clip);
        draw_border(pixmap, mask, picker, BORDER_COLOR, clip);
    }
    for control in &layout.controls {
        paint_control(pixmap, mask, text_system, control, clip);
    }
}

fn paint_control(
    pixmap: &mut Pixmap,
    mask: &Mask,
    text_system: &mut TextSystem,
    control: &ToolbarControl,
    clip: &crate::Rect,
) {
    if control.selected {
        draw_border(pixmap, mask, control.bounds, ACCENT_COLOR, clip);
    }

    let mut label_x = control.bounds.x + PADDING_PX;
    if let Some(color) = control.swatch {
        let swatch_y = control.bounds.y + ((ROW_PX - SWATCH_PX) as i32) / 2;
        let swatch = crate::Rect {
            x: label_x,
            y: swatch_y,
            width: SWATCH_PX,
            height: SWATCH_PX,
        };
        fill_clipped(pixmap, swatch, color, 255, clip);
        // A swatch border contrasted against its own luminance keeps both
        // black and white swatches visible on the dark bar.
        let luminance = 0.299 * color[0] as f32 + 0.587 * color[1] as f32 + 0.114 * color[2] as f32;
        let edge = if luminance > 140.0 { [0, 0, 0] } else { [255, 255, 255] };
        draw_border(pixmap, mask, swatch, edge, clip);
        label_x += SWATCH_PX as i32 + GAP_PX;
    }

    let text_color = if control.enabled { TEXT_LIGHT } else { TEXT_MUTED };
    let color = TextColor::rgb(text_color[0], text_color[1], text_color[2]);
    let origin_x = label_x;
    let origin_y = control.bounds.y + ((ROW_PX - UI_FONT_PX) as i32) / 2;
    text_system.draw_label(&control.label, UI_FONT_PX, color, |x, y, width, height, red, green, blue, alpha| {
        blend_rect(
            pixmap,
            x + origin_x,
            y + origin_y,
            width,
            height,
            red,
            green,
            blue,
            alpha,
            clip,
        );
    });
}
