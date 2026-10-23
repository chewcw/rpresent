use rpresent_spotlight::spotlight_alpha;

#[test]
fn alpha_is_full_inside_the_radius_and_fades_linearly() {
    assert_eq!(spotlight_alpha(0.0, 200.0, 40.0), 1.0);
    assert_eq!(spotlight_alpha(200.0, 200.0, 40.0), 1.0);
    assert_eq!(spotlight_alpha(220.0, 200.0, 40.0), 0.5);
    assert_eq!(spotlight_alpha(240.0, 200.0, 40.0), 0.0);
    assert_eq!(spotlight_alpha(260.0, 200.0, 40.0), 0.0);
}
