#[cfg(feature = "qml")]
mod ipc_message {
    use rpresent_config::SpotlightConfig;
    use rpresent_spotlight::IpcMessage;

    #[test]
    fn show_message_serializes() {
        let json = serde_json::to_string(&IpcMessage::Show).unwrap();
        assert_eq!(json, r#"{"type":"show"}"#);
    }

    #[test]
    fn hide_message_serializes() {
        let json = serde_json::to_string(&IpcMessage::Hide).unwrap();
        assert_eq!(json, r#"{"type":"hide"}"#);
    }

    #[test]
    fn quit_message_serializes() {
        let json = serde_json::to_string(&IpcMessage::Quit).unwrap();
        assert_eq!(json, r#"{"type":"quit"}"#);
    }

    #[test]
    fn redraw_message_contains_cursor_and_config() {
        let config = SpotlightConfig::default();
        let msg = IpcMessage::Redraw {
            cursor_x: 100,
            cursor_y: 200,
            surface_w: 1920,
            surface_h: 1080,
            config: &config,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["type"], "redraw", "wrong type field: {json}");
        assert_eq!(value["cursor_x"], 100, "wrong cursor_x: {json}");
        assert_eq!(value["cursor_y"], 200, "wrong cursor_y: {json}");
        assert_eq!(value["surface_w"], 1920, "wrong surface_w: {json}");
        assert_eq!(value["surface_h"], 1080, "wrong surface_h: {json}");
        assert!(value["config"].is_object(), "config should be an object: {json}");
    }

    #[test]
    fn redraw_config_contains_shape_type() {
        use rpresent_config::{SpotShape, SpotlightConfig};
        let mut config = SpotlightConfig::default();
        config.shape = SpotShape::Circle;
        let msg = IpcMessage::Redraw {
            cursor_x: 0,
            cursor_y: 0,
            surface_w: 1920,
            surface_h: 1080,
            config: &config,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["type"], "redraw");
        assert_eq!(
            value["config"]["shape"]["type"],
            "circle",
            "shape type missing or wrong: {json}"
        );
    }
}

#[cfg(feature = "qml")]
mod spawn {
    use rpresent_spotlight::QmlSpotlightOverlay;

    /// Requires `rpresent-qml-host` to be in PATH.  Run manually after building the Qt host.
    #[test]
    #[ignore]
    fn can_create_and_drop() {
        let _overlay = QmlSpotlightOverlay::new().expect("QML overlay creation should succeed");
        // Drop sends Quit and joins child — test passes if no panic.
    }
}
