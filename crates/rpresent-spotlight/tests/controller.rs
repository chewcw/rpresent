use rpresent_spotlight::{SpotlightCommand, SpotlightController, SpotlightState};

#[test]
fn toggle_flips_between_on_and_off() {
    let mut controller = SpotlightController::new(false);

    assert_eq!(controller.state(), SpotlightState::Off);
    assert!(controller.apply(SpotlightCommand::Toggle));
    assert_eq!(controller.state(), SpotlightState::On);
    assert!(controller.apply(SpotlightCommand::Toggle));
    assert_eq!(controller.state(), SpotlightState::Off);
}

#[test]
fn on_and_off_are_idempotent() {
    let mut controller = SpotlightController::new(true);

    assert!(!controller.apply(SpotlightCommand::On));
    assert_eq!(controller.state(), SpotlightState::On);
    assert!(controller.apply(SpotlightCommand::Off));
    assert_eq!(controller.state(), SpotlightState::Off);
    assert!(!controller.apply(SpotlightCommand::Off));
    assert_eq!(controller.state(), SpotlightState::Off);
}
