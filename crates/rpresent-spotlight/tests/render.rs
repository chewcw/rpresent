use rpresent_config::{SpotShape, SpotlightConfig};
use rpresent_spotlight::{CursorPosition, build_projecteur_frame, build_projecteur_frame_rect};

// Surface: 400×300, cursor at centre (200,150), spot_size_pct=40
//   radius = floor(300 * 40 / 100) = 120 px
//   border inner_r = 120 * (1 - 10/100) = 108 px
//   dot_size_px=8 → dot radius 4 px

const W: u16 = 400;
const H: u16 = 300;
const CURSOR: CursorPosition = CursorPosition { x: 200, y: 150 };

fn base_config() -> SpotlightConfig {
    SpotlightConfig {
        spot_size_pct: 40,
        ..SpotlightConfig::default()
    }
}

/// Distance from cursor to pixel (px, py).
fn dist(px: i32, py: i32) -> f32 {
    let dx = (px - CURSOR.x as i32) as f32;
    let dy = (py - CURSOR.y as i32) as f32;
    (dx * dx + dy * dy).sqrt()
}

// ─────────────────────────────── Circle ──────────────────────────────────

#[test]
fn circle_exterior_alpha_equals_shade_opacity() {
    let config = base_config(); // show_spot_shade=true, shade_opacity=0.6
    let frame = build_projecteur_frame((W, H), CURSOR, &config);
    // Pixel (0, 0): ~283 px from cursor — well outside radius (120)
    let alpha = frame.bgra_at(0, 0)[3];
    let expected = (0.6_f32 * 255.0).round() as u8; // 153
    assert_eq!(
        alpha, expected,
        "exterior pixel alpha should equal shade_opacity×255 = {expected}, got {alpha}"
    );
}

#[test]
fn circle_interior_is_transparent() {
    let mut config = base_config();
    config.show_center_dot = false; // exclude dot so we see the pure hole
    let frame = build_projecteur_frame((W, H), CURSOR, &config);
    // Pixel 30 px above cursor: distance=30 < inner_r(108) → inside hole and inside inner shape
    let alpha = frame.bgra_at(200, 120)[3];
    assert_eq!(
        alpha, 0,
        "pixel inside the spotlight hole should be transparent"
    );
}

#[test]
fn circle_border_ring_has_border_color() {
    let mut config = base_config();
    config.show_spot_shade = false; // simplify: shade off, ring still drawn
    // Pixel 114 px above cursor: between inner_r(108) and outer_r(120) → solidly in ring
    assert!(
        dist(200, 36) > 108.0 && dist(200, 36) < 120.0,
        "test pixel must be in ring"
    );
    let frame = build_projecteur_frame((W, H), CURSOR, &config);
    let bgra = frame.bgra_at(200, 36);
    let expected_alpha = (0.8_f32 * 255.0).round() as u8; // 204
    assert_eq!(
        bgra[3], expected_alpha,
        "border ring alpha should equal border_opacity×255 = {expected_alpha}, got {}",
        bgra[3]
    );
    // border_color="#ffffff" white at 80% — tiny-skia stores premultiplied values.
    // All channels (including colour) equal alpha when colour is white.
    assert_eq!(
        bgra[0], expected_alpha,
        "border B channel (premultiplied) should equal alpha"
    );
    assert_eq!(
        bgra[1], expected_alpha,
        "border G channel (premultiplied) should equal alpha"
    );
    assert_eq!(
        bgra[2], expected_alpha,
        "border R channel (premultiplied) should equal alpha"
    );
}

#[test]
fn circle_no_shade_exterior_is_transparent() {
    let mut config = base_config();
    config.show_spot_shade = false;
    config.show_border = false;
    config.show_center_dot = false;
    let frame = build_projecteur_frame((W, H), CURSOR, &config);
    let alpha = frame.bgra_at(0, 0)[3];
    assert_eq!(
        alpha, 0,
        "exterior pixel should be transparent when shade is off"
    );
}

#[test]
fn circle_center_dot_is_visible() {
    let mut config = base_config();
    config.show_spot_shade = false;
    config.show_border = false;
    // dot_size_px=8 → radius 4 px; pixel at exact cursor should be covered
    let frame = build_projecteur_frame((W, H), CURSOR, &config);
    let alpha = frame.bgra_at(200, 150)[3];
    assert!(alpha > 0, "centre dot pixel should be non-transparent");
}

// ─────────────────────────────── Square ──────────────────────────────────

#[test]
fn square_exterior_has_shade_and_interior_is_transparent() {
    let mut config = base_config();
    config.shape = SpotShape::Square {
        corner_radius_pct: 20,
    };
    config.show_center_dot = false;
    config.show_border = false;
    let frame = build_projecteur_frame((W, H), CURSOR, &config);

    let exterior = frame.bgra_at(0, 0)[3];
    let expected = (0.6_f32 * 255.0).round() as u8;
    assert_eq!(
        exterior, expected,
        "square: exterior alpha should be shade_opacity×255"
    );

    // Centre of the square hole:
    let interior = frame.bgra_at(200, 150)[3];
    assert_eq!(
        interior, 0,
        "square: pixel at cursor centre should be transparent"
    );
}

// ─────────────────────────────── Star ────────────────────────────────────

#[test]
fn star_exterior_has_shade() {
    let mut config = base_config();
    config.shape = SpotShape::Star {
        points: 5,
        inner_radius_pct: 50,
    };
    config.show_border = false;
    config.show_center_dot = false;
    let frame = build_projecteur_frame((W, H), CURSOR, &config);
    let alpha = frame.bgra_at(0, 0)[3];
    let expected = (0.6_f32 * 255.0).round() as u8;
    assert_eq!(
        alpha, expected,
        "star: exterior alpha should be shade_opacity×255"
    );
}

// ─────────────────────────────── Ngon ────────────────────────────────────

#[test]
fn ngon_exterior_has_shade() {
    let mut config = base_config();
    config.shape = SpotShape::Ngon { sides: 6 };
    config.show_border = false;
    config.show_center_dot = false;
    let frame = build_projecteur_frame((W, H), CURSOR, &config);
    let alpha = frame.bgra_at(0, 0)[3];
    let expected = (0.6_f32 * 255.0).round() as u8;
    assert_eq!(
        alpha, expected,
        "ngon: exterior alpha should be shade_opacity×255"
    );
}

// ─────────────────────────────── Frame metadata ──────────────────────────

#[test]
fn frame_dimensions_match_surface_size() {
    let config = base_config();
    let frame = build_projecteur_frame((W, H), CURSOR, &config);
    assert_eq!(frame.width, W);
    assert_eq!(frame.height, H);
    assert_eq!(frame.pixels.len(), W as usize * H as usize * 4);
}

// ─────────────────────────── Damage rectangle ────────────────────────────

#[test]
fn damage_rect_frame_matches_the_same_region_of_the_full_frame() {
    let mut config = base_config();
    config.show_center_dot = false;
    let full = build_projecteur_frame((W, H), CURSOR, &config);
    let patch = build_projecteur_frame_rect((W, H), (70, 20, 260, 260), CURSOR, &config);

    assert_eq!((patch.width, patch.height), (260, 260));
    // Patch origin (70,20) puts the cursor at local (130,130); the hole must
    // still be transparent there.
    assert_eq!(patch.bgra_at(130, 130)[3], 0);

    for y in 0..260u16 {
        for x in 0..260u16 {
            assert_eq!(
                patch.bgra_at(x, y),
                full.bgra_at(70 + x, 20 + y),
                "patch pixel ({x}, {y}) differs from the full frame"
            );
        }
    }
}

#[test]
fn damage_rect_outside_the_spotlight_is_filled_with_shade() {
    let config = base_config();
    let patch = build_projecteur_frame_rect((W, H), (0, 0, 50, 50), CURSOR, &config);
    let expected = (0.6_f32 * 255.0).round() as u8;

    for y in 0..50u16 {
        for x in 0..50u16 {
            assert_eq!(
                patch.bgra_at(x, y)[3],
                expected,
                "patch pixel ({x}, {y}) must be repainted with the shade colour"
            );
        }
    }
}

#[test]
fn damage_rect_outside_the_spotlight_is_transparent_when_the_shade_is_off() {
    let mut config = base_config();
    config.show_spot_shade = false;
    config.show_border = false;
    let patch = build_projecteur_frame_rect((W, H), (0, 0, 50, 50), CURSOR, &config);

    for y in 0..50u16 {
        for x in 0..50u16 {
            assert_eq!(patch.bgra_at(x, y), [0, 0, 0, 0]);
        }
    }
}
