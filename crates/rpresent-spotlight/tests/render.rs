use rpresent_config::{SpotShape, SpotlightConfig};
use rpresent_spotlight::{
    CursorPosition, SpotlightFrame, build_projecteur_frame, build_projecteur_frame_rect,
};

// Surface: 400×300, cursor at centre (200,150), radius=120 px
//   border inner_r = 120 * (1 - 10/100) = 108 px
//   dot_size_px=8 → dot radius 4 px

const W: u16 = 400;
const H: u16 = 300;
const CURSOR: CursorPosition = CursorPosition { x: 200, y: 150 };
const RADIUS: u32 = 120;

fn base_config() -> SpotlightConfig {
    SpotlightConfig {
        softness_px: 0,
        ..SpotlightConfig::default()
    }
}

#[test]
fn software_frame_uses_the_supplied_live_radius() {
    let config = SpotlightConfig {
        softness_px: 0,
        show_border: false,
        show_center_dot: false,
        ..Default::default()
    };
    let small = build_projecteur_frame((W, H), CURSOR, &config, 40);
    let large = build_projecteur_frame((W, H), CURSOR, &config, 80);
    assert!(small.bgra_at(260, 150)[3] > 0);
    assert_eq!(large.bgra_at(260, 150)[3], 0);
}

#[test]
fn softness_fades_outward_from_the_shape_boundary() {
    let config = SpotlightConfig {
        shade_opacity: 1.0,
        softness_px: 20,
        show_border: false,
        show_center_dot: false,
        ..Default::default()
    };
    let frame = build_projecteur_frame((W, H), CURSOR, &config, 40);
    assert_eq!(frame.bgra_at(220, 150)[3], 0);
    let midpoint = frame.bgra_at(250, 150)[3];
    assert!((120..=136).contains(&midpoint));
    assert_eq!(frame.bgra_at(280, 150)[3], 255);
}

#[test]
fn softness_values_cover_hard_edges_and_finite_fades() {
    for softness_px in [0, 1, 20, 80] {
        let config = SpotlightConfig {
            shade_opacity: 1.0,
            softness_px,
            show_border: false,
            show_center_dot: false,
            ..Default::default()
        };
        let frame = build_projecteur_frame((W, H), CURSOR, &config, 40);
        assert_eq!(frame.bgra_at(200, 150)[3], 0);
        assert_eq!(frame.bgra_at(399, 299)[3], 255);
    }
}

#[test]
fn softness_follows_square_and_polygon_boundaries() {
    let mut square = SpotlightConfig {
        shade_opacity: 1.0,
        softness_px: 20,
        show_border: false,
        show_center_dot: false,
        shape: SpotShape::Square {
            corner_radius_pct: 0,
        },
        ..Default::default()
    };
    let square_frame = build_projecteur_frame((W, H), CURSOR, &square, 40);
    assert_eq!(square_frame.bgra_at(240, 150)[3], 0);
    assert!((120..=136).contains(&square_frame.bgra_at(250, 150)[3]));
    assert_eq!(square_frame.bgra_at(260, 150)[3], 255);

    square.shape = SpotShape::Ngon { sides: 4 };
    let polygon_frame = build_projecteur_frame((W, H), CURSOR, &square, 40);
    assert_eq!(polygon_frame.bgra_at(200, 110)[3], 0);
    assert!((120..=136).contains(&polygon_frame.bgra_at(200, 100)[3]));
    assert_eq!(polygon_frame.bgra_at(200, 90)[3], 255);

    square.shape = SpotShape::Star {
        points: 5,
        inner_radius_pct: 50,
    };
    let star_frame = build_projecteur_frame((W, H), CURSOR, &square, 40);
    assert_eq!(star_frame.bgra_at(200, 110)[3], 0);
    assert!((120..=136).contains(&star_frame.bgra_at(200, 100)[3]));
    assert_eq!(star_frame.bgra_at(200, 90)[3], 255);
}

#[test]
fn colored_soft_shade_uses_premultiplied_bgra() {
    let config = SpotlightConfig {
        shade_color: "#804020".to_owned(),
        shade_opacity: 0.5,
        softness_px: 0,
        show_border: false,
        show_center_dot: false,
        ..Default::default()
    };
    let frame = build_projecteur_frame((W, H), CURSOR, &config, 20);

    assert_eq!(frame.bgra_at(0, 0), [16, 32, 64, 128]);
}

#[test]
fn soft_patch_matches_the_same_region_of_the_full_frame() {
    let config = SpotlightConfig {
        shade_opacity: 1.0,
        softness_px: 20,
        show_border: true,
        show_center_dot: true,
        ..Default::default()
    };
    let full = build_projecteur_frame((W, H), CURSOR, &config, 40);
    let patch = build_projecteur_frame_rect((130, 80, 160, 160), CURSOR, &config, 40);

    for y in 0..patch.height {
        for x in 0..patch.width {
            assert_eq!(
                patch.bgra_at(x, y),
                full.bgra_at(130 + x, 80 + y),
                "soft patch pixel ({x}, {y}) differs from the full frame"
            );
        }
    }
}

/// Distance from cursor to pixel (px, py).
fn dist(px: i32, py: i32) -> f32 {
    let dx = (px - CURSOR.x as i32) as f32;
    let dy = (py - CURSOR.y as i32) as f32;
    (dx * dx + dy * dy).sqrt()
}

fn paint_bounds(cursor: CursorPosition, radius_px: u32) -> (i16, i16, u16, u16) {
    let radius = radius_px as i32 + 1;
    (
        cursor.x - radius as i16,
        cursor.y - radius as i16,
        (radius * 2) as u16,
        (radius * 2) as u16,
    )
}

fn union_bounds(first: (i16, i16, u16, u16), second: (i16, i16, u16, u16)) -> (i16, i16, u16, u16) {
    let x0 = i32::from(first.0).min(i32::from(second.0));
    let y0 = i32::from(first.1).min(i32::from(second.1));
    let x1 =
        (i32::from(first.0) + i32::from(first.2)).max(i32::from(second.0) + i32::from(second.2));
    let y1 =
        (i32::from(first.1) + i32::from(first.3)).max(i32::from(second.1) + i32::from(second.3));
    (x0 as i16, y0 as i16, (x1 - x0) as u16, (y1 - y0) as u16)
}

fn apply_patch(base: &mut SpotlightFrame, patch: &SpotlightFrame, origin: (i16, i16)) {
    for y in 0..patch.height {
        for x in 0..patch.width {
            let source = ((y as usize * patch.width as usize) + x as usize) * 4;
            let destination = (((origin.1 as u16 + y) as usize * base.width as usize)
                + origin.0 as usize
                + x as usize)
                * 4;
            base.pixels[destination..destination + 4]
                .copy_from_slice(&patch.pixels[source..source + 4]);
        }
    }
}

// ─────────────────────────────── Circle ──────────────────────────────────

#[test]
fn circle_exterior_alpha_equals_shade_opacity() {
    let config = base_config(); // show_spot_shade=true, shade_opacity=0.6
    let frame = build_projecteur_frame((W, H), CURSOR, &config, RADIUS);
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
    let frame = build_projecteur_frame((W, H), CURSOR, &config, RADIUS);
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
    let frame = build_projecteur_frame((W, H), CURSOR, &config, RADIUS);
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
    let frame = build_projecteur_frame((W, H), CURSOR, &config, RADIUS);
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
    let frame = build_projecteur_frame((W, H), CURSOR, &config, RADIUS);
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
    let frame = build_projecteur_frame((W, H), CURSOR, &config, RADIUS);

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
    let frame = build_projecteur_frame((W, H), CURSOR, &config, RADIUS);
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
    let frame = build_projecteur_frame((W, H), CURSOR, &config, RADIUS);
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
    let frame = build_projecteur_frame((W, H), CURSOR, &config, RADIUS);
    assert_eq!(frame.width, W);
    assert_eq!(frame.height, H);
    assert_eq!(frame.pixels.len(), W as usize * H as usize * 4);
}

// ─────────────────────────── Damage rectangle ────────────────────────────

#[test]
fn damage_rect_frame_matches_the_same_region_of_the_full_frame() {
    let mut config = base_config();
    config.show_center_dot = false;
    let full = build_projecteur_frame((W, H), CURSOR, &config, RADIUS);
    let patch = build_projecteur_frame_rect((70, 20, 260, 260), CURSOR, &config, RADIUS);

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
    let patch = build_projecteur_frame_rect((0, 0, 50, 50), CURSOR, &config, RADIUS);
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
    let patch = build_projecteur_frame_rect((0, 0, 50, 50), CURSOR, &config, RADIUS);

    for y in 0..50u16 {
        for x in 0..50u16 {
            assert_eq!(patch.bgra_at(x, y), [0, 0, 0, 0]);
        }
    }
}

#[test]
fn radius_growth_and_shrink_patches_reconstruct_full_frames() {
    let config = SpotlightConfig {
        softness_px: 0,
        show_border: false,
        show_center_dot: false,
        ..base_config()
    };

    let mut grown = build_projecteur_frame((W, H), CURSOR, &config, 40);
    let growth_rect = union_bounds(paint_bounds(CURSOR, 40), paint_bounds(CURSOR, 80));
    let growth_patch = build_projecteur_frame_rect(growth_rect, CURSOR, &config, 80);
    apply_patch(&mut grown, &growth_patch, (growth_rect.0, growth_rect.1));
    assert_eq!(
        grown,
        build_projecteur_frame((W, H), CURSOR, &config, 80),
        "a radius increase must repaint the union of old and new bounds"
    );

    let mut shrunk = build_projecteur_frame((W, H), CURSOR, &config, 80);
    let shrink_rect = union_bounds(paint_bounds(CURSOR, 80), paint_bounds(CURSOR, 20));
    let shrink_patch = build_projecteur_frame_rect(shrink_rect, CURSOR, &config, 20);
    apply_patch(&mut shrunk, &shrink_patch, (shrink_rect.0, shrink_rect.1));
    assert_eq!(
        shrunk,
        build_projecteur_frame((W, H), CURSOR, &config, 20),
        "a radius decrease must erase the old larger hole"
    );
}

#[test]
fn movement_and_radius_change_reconstructs_the_new_full_frame() {
    let config = SpotlightConfig {
        softness_px: 0,
        show_border: false,
        show_center_dot: false,
        ..base_config()
    };
    let next_cursor = CursorPosition { x: 220, y: 170 };
    let mut patched = build_projecteur_frame((W, H), CURSOR, &config, 40);
    let damage = union_bounds(paint_bounds(CURSOR, 40), paint_bounds(next_cursor, 80));
    let patch = build_projecteur_frame_rect(damage, next_cursor, &config, 80);
    apply_patch(&mut patched, &patch, (damage.0, damage.1));

    assert_eq!(
        patched,
        build_projecteur_frame((W, H), next_cursor, &config, 80),
        "movement and radius changes must be covered by one damage patch"
    );
}
