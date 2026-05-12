# Zoom Wheel Mode Sync Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make mouse wheel zoom work after zoom is toggled from either the keyboard shortcut or a daemon command by keeping the input router's active mode synchronized with command-driven mode changes.

**Architecture:** Keep the existing mode-based input routing model, but move active mode into a small shared handle that both the X11 input thread and the daemon command loop can read and write. The zoom controller and overlay rendering code stay unchanged; only the source of truth for the selected mode changes so command-driven `zoom=on` / `zoom=toggle` cannot leave the wheel router stuck in spotlight mode.

**Tech Stack:** Rust, `std::sync::{Arc, Mutex}`, `std::sync::mpsc`, `x11rb`, existing integration tests.

---

### Task 1: Capture the regression in tests

**Files:**
- Modify: `crates/rpresent/tests/input.rs`
- Modify: `crates/rpresent/src/app.rs`

- [ ] **Step 1: Add a regression test for shared active-mode wheel routing**

```rust
#[test]
fn zoom_mode_shared_state_routes_wheel_after_command_toggle() {
    let bindings = InputBindings::from(&ShortcutConfig::default());
    let active_mode = ActiveModeState::new(ActiveMode::Spotlight);
    let router = InputRouter::new(bindings, active_mode.clone());

    active_mode.set(ActiveMode::Zoom);

    assert_eq!(
        router.handle_wheel("Ctrl", -1.0, 640, 360),
        Some(InputAction::ZoomWheel {
            delta: -1.0,
            cursor_x: 640,
            cursor_y: 360,
        })
    );
}
```

- [ ] **Step 2: Add a unit test for the daemon-side command helper**

```rust
#[test]
fn zoom_command_sets_zoom_as_the_active_mode() {
    let active_mode = ActiveModeState::new(ActiveMode::Spotlight);

    update_active_mode_for_command(&active_mode, &AppCommand::Zoom(ZoomCommand::On));

    assert_eq!(active_mode.get(), ActiveMode::Zoom);
}
```

- [ ] **Step 3: Run the narrow test slice and confirm it fails before the implementation exists**

Run: `cargo test -p rpresent --test input zoom_mode_shared_state_routes_wheel_after_command_toggle -v`

Run: `cargo test -p rpresent zoom_command_sets_zoom_as_the_active_mode -v`

Expected: both tests fail until the shared mode handle and helper are wired through input and command handling.

### Task 2: Thread shared active mode through input and daemon code

**Files:**
- Modify: `crates/rpresent/src/input.rs`
- Modify: `crates/rpresent/src/x11_input.rs`
- Modify: `crates/rpresent/src/app.rs`

- [ ] **Step 1: Add a cloneable shared mode handle in `input.rs`**

```rust
#[derive(Clone, Debug)]
pub struct ActiveModeState {
    inner: std::sync::Arc<std::sync::Mutex<ActiveMode>>,
}

impl ActiveModeState {
    pub fn new(initial: ActiveMode) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(initial)),
        }
    }

    pub fn get(&self) -> ActiveMode {
        *self.inner.lock().unwrap()
    }

    pub fn set(&self, mode: ActiveMode) {
        *self.inner.lock().unwrap() = mode;
    }
}
```

- [ ] **Step 2: Update `InputRouter` to read and write the shared mode handle**

```rust
pub struct InputRouter {
    bindings: InputBindings,
    active_mode: ActiveModeState,
}

impl InputRouter {
    pub fn new(bindings: InputBindings, active_mode: ActiveModeState) -> Self {
        Self { bindings, active_mode }
    }
}
```

Keep the existing routing rules, but have `handle_key` set the shared mode before returning the matching command, and have `handle_wheel`, `handle_drag_start`, `handle_drag_move`, and `handle_drag_end` consult the shared mode instead of a private copy.

- [ ] **Step 3: Add a small helper in `app.rs` that mirrors shortcut-driven mode selection**

```rust
fn update_active_mode_for_command(active_mode: &ActiveModeState, command: &AppCommand) {
    match command {
        AppCommand::Spotlight(_) => active_mode.set(crate::input::ActiveMode::Spotlight),
        AppCommand::Annotation(_) => active_mode.set(crate::input::ActiveMode::Annotation),
        AppCommand::Zoom(_) => active_mode.set(crate::input::ActiveMode::Zoom),
    }
}
```

Use this helper in the daemon event loop before the corresponding controller applies the command so the keyboard path and IPC path agree on the currently selected mode.

- [ ] **Step 4: Pass one shared `ActiveModeState` into both threads**

In `run_daemon`, create one shared state seeded to `ActiveMode::Spotlight`, pass a clone into `x11_input::spawn(config.shortcuts.clone(), active_mode.clone(), event_tx)`, and reuse the same handle in the command-processing loop.

```rust
let active_mode = crate::input::ActiveModeState::new(crate::input::ActiveMode::Spotlight);

#[cfg(target_os = "linux")]
crate::x11_input::spawn(config.shortcuts.clone(), active_mode.clone(), event_tx);

for event in event_rx {
    match event {
        AppEvent::Command(command) => {
            update_active_mode_for_command(&active_mode, &command);
            match command {
                AppCommand::Spotlight(command) => {
                    if spotlight_controller.apply(command) {
                        let _ = spotlight_tx.send(SpotlightRuntimeMessage::Visibility(
                            spotlight_controller.state(),
                        ));
                    }
                }
                AppCommand::Annotation(command) => {
                    if annotation_controller.apply(command) {
                        let _ = annotation_tx.send(annotation_controller.state());
                    }
                }
                AppCommand::Zoom(command) => {
                    if let Some(message) = zoom_controller.apply(command) {
                        let _ = zoom_tx.send(message);
                    }
                }
            }
        }
        _ => {}
    }
}
```

- [ ] **Step 5: Update `x11_input::spawn` to accept the shared mode handle and seed the router from it**

```rust
pub(crate) fn spawn(
    shortcuts: rpresent_config::ShortcutConfig,
    active_mode: crate::input::ActiveModeState,
    tx: std::sync::mpsc::Sender<crate::input::AppEvent>,
) {
    std::thread::spawn(move || {
        if let Err(e) = run_input_loop(shortcuts, active_mode, tx) {
            tracing::error!(error = %e, "X11 input loop failed");
        }
    });
}
```

### Task 3: Validate the fixed slice and stop before widening scope

**Files:**
- Test: `crates/rpresent/tests/input.rs`
- Test: `crates/rpresent/src/app.rs`

- [ ] **Step 1: Re-run the focused regression tests**

Run: `cargo test -p rpresent --test input zoom_mode_shared_state_routes_wheel_after_command_toggle -v`

Run: `cargo test -p rpresent zoom_command_sets_zoom_as_the_active_mode -v`

Expected: both pass after the shared mode handle and daemon helper are wired through.

- [ ] **Step 2: Run the broader `rpresent` slice that covers input routing and command parsing**

Run: `cargo test -p rpresent --test input`

Run: `cargo test -p rpresent`

Expected: pass, with spotlight and annotation routing unchanged and no edits required in `rpresent-zoom`.

- [ ] **Step 3: If a test fails, repair only the mode-sync slice and rerun the same focused command before widening scope**

Do not touch the zoom overlay renderer unless the new shared mode API unexpectedly changes a zoom crate signature.

## Execution Status

```text
## Execution Status
- Status: Completed
```

## Execution Log

### Zoom wheel mode sync - Completed
- **Issues Encountered**: None.
- **Resolution**: Seeded the shared input mode from `zoom.enabled_on_start`, let zoom mode accept wheel events without the configured modifier, and widened the X11 wheel grab so unmodified wheel events reach the router.
- **Changes from Plan**: The daemon now treats `zoom=on` as Zoom, `zoom=off` as Spotlight, `zoom=toggle` according to the controller's enabled result, and `zoom=reset` leaves the active mode unchanged.
- **Notes**: Added regression coverage for startup seeding, zoom wheel routing without a modifier, and precise zoom command mode updates.

## Blockers

- None.

**Next Steps**: None.
