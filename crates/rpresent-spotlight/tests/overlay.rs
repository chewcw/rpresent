use rpresent_config::SpotlightConfig;
use rpresent_spotlight::{SpotlightAppearance, spotlight_alpha};

#[test]
fn alpha_is_full_inside_the_radius_and_fades_linearly() {
    assert_eq!(spotlight_alpha(0.0, 200.0, 40.0), 1.0);
    assert_eq!(spotlight_alpha(200.0, 200.0, 40.0), 1.0);
    assert_eq!(spotlight_alpha(220.0, 200.0, 40.0), 0.5);
    assert_eq!(spotlight_alpha(240.0, 200.0, 40.0), 0.0);
    assert_eq!(spotlight_alpha(260.0, 200.0, 40.0), 0.0);
}

#[test]
fn overlay_defaults_match_the_mvp() {
    let appearance = SpotlightAppearance::default();

    assert_eq!(appearance.radius_px, 240);
    assert_eq!(appearance.softness_px, 80);
    assert!((appearance.dim_alpha - 0.72).abs() < f32::EPSILON);
}

#[test]
fn appearance_can_be_built_from_config() {
    let appearance = SpotlightAppearance::from(SpotlightConfig {
        enabled_on_start: false,
        radius_px: 300,
        softness_px: 90,
        dim_alpha: 0.8,
        ..SpotlightConfig::default()
    });

    assert_eq!(appearance.radius_px, 300);
    assert_eq!(appearance.softness_px, 90);
    assert!((appearance.dim_alpha - 0.8).abs() < f32::EPSILON);
}
