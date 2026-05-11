# Zoom Module Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to execute this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement an actual visual zoom overlay on Linux/X11 so `rpresent` in daemon mode can render magnification when zoom is enabled, while preserving the existing command, IPC, and shortcut architecture.

**Current Gap (confirmed):** `crates/rpresent-zoom/src/overlay.rs` currently logs state updates but does not render anything. This is why `cargo run -p rpresent -- --config ./config.toml` appears to do nothing unless explicit commands are sent, and even then no zoom visuals appear.

**Scope:**
- In scope: zoom rendering backend, zoom input routing for wheel events, state-to-overlay integration, tests.
- Out of scope: Wayland backend, UI/preferences app, non-Linux overlay backend, speculative multi-retry initialization paths.

---

## Design Constraints

- Keep changes surgical and module-local where possible.
- Do not introduce repeated initialization loops or complex fallback orchestration.
- Preserve current command schema (`zoom=on|off|toggle|reset`) and daemon IPC model.
- Keep startup behavior unchanged unless config explicitly enables zoom.

---

## Implementation Tasks

### Task 1: Implement Linux X11 zoom overlay backend

**Files:**
- Modify: `crates/rpresent-zoom/src/overlay.rs`
- Modify: `crates/rpresent-zoom/Cargo.toml` (only if new deps are needed)

**Approach:** Start implementation in `overlay.rs`; split to a new `x11.rs` only if the backend exceeds ~150 lines. Use `XGetImage` for MVP screen capture — no new crate needed, as x11rb already exposes this call in the workspace. Upload the magnified frame using `x11rb PutImage` (the same transport path used by `rpresent-spotlight`). `XShm` is a named follow-up optimization and is **not** required in this task. The overlay window must be created with an ARGB32 visual and `override_redirect = true` so it floats above all windows without a window manager frame.

- [ ] Replace stub `run_overlay` loop with a real overlay runtime on Linux.
- [ ] Add a minimal `X11ZoomOverlay` backend inside `overlay.rs` that:
- [ ] Creates an ARGB32 overlay window with `override_redirect = true`.
- [ ] Tracks screen size and cursor position.
- [ ] Captures source pixels via `XGetImage` and uploads the magnified frame via `PutImage`.
- [ ] Honors `ZoomState.enabled` by showing/hiding rendering without killing the thread.
- [ ] Honors `ZoomAppearance` + `ZoomState` parameters: scale bounds, follow-cursor behavior, and pivot.
- [ ] Keep non-Linux behavior as `UnsupportedPlatform`.
- [ ] Add trace/debug logs for state transitions and backend lifecycle.

**Acceptance criteria:**
- `zoom=on` produces visible magnification.
- `zoom=off` removes visual output without crashing.
- Overlay thread exits cleanly when sender is dropped.

### Task 2: Wire wheel input to zoom when zoom mode is active

**Files:**
- Modify: `crates/rpresent/src/input.rs`
- Modify: `crates/rpresent/src/app.rs`
- Modify: `crates/rpresent/src/x11_input.rs`
- Modify: `crates/rpresent/tests/input.rs`

- [ ] Extend `InputAction` with zoom-specific wheel action (`ZoomWheel { delta: f32, cursor_x: i16, cursor_y: i16 }`), while keeping existing spotlight wheel behavior. `cursor_x`/`cursor_y` carry root-window coordinates from `x11_input.rs`; `app.rs` converts them to frame-space by passing `(cursor_x as f32, cursor_y as f32)` directly to `ZoomController::zoom_by_scroll(delta, pivot)`. No coordinate remapping or screen-to-frame transform is required for MVP — the controller already clamps pivot via `zoom_sample_rect`.
- [ ] Update `InputRouter::handle_wheel` so:
- [ ] Spotlight mode keeps current spotlight wheel behavior.
- [ ] Zoom mode emits zoom wheel actions (using configured modifier strategy).
- [ ] In `x11_input.rs`, include pointer coordinates from wheel events when emitting zoom wheel actions.
- [ ] In `app.rs` event loop, apply zoom wheel action through `ZoomController::zoom_by_scroll(...)` and forward updated state to zoom overlay channel.
- [ ] Add/adjust tests for mode-specific wheel routing and modifier normalization.

**Acceptance criteria:**
- `Ctrl+3` activates zoom mode and toggles zoom.
- Wheel in zoom mode changes scale and updates overlay in real time.
- Wheel in non-zoom modes does not mutate zoom state.

### Task 3: Add optional pan control and deterministic behavior

**Files:**
- Modify: `crates/rpresent/src/input.rs` (if needed)
- Modify: `crates/rpresent/src/x11_input.rs` (if needed)
- Modify: `crates/rpresent/src/app.rs`
- Modify: `crates/rpresent-zoom/tests/controller.rs` — cover:
  - `zoom_by_scroll` with `follow_cursor=true` moves pivot to the given pointer position.
  - `zoom_by_scroll` with a fixed pivot (pass same coordinate twice) keeps pivot stable.
  - `reset()` restores `scale` to `initial_scale` and `pivot` to `(0.0, 0.0)`.

- [ ] Decide and implement one pan path (minimal option): pointer-follow only when `follow_cursor=true`, fixed pivot otherwise.
- [ ] Optional small follow-up: add keyboard panning (`ZoomPan`) only if required for parity.
- [ ] Ensure `zoom=reset` deterministically restores scale and pivot and triggers redraw.

**Acceptance criteria:**
- With `follow_cursor=true`, zoom center tracks pointer.
- With `follow_cursor=false`, zoom center remains stable unless explicitly changed.
- Reset behavior is stable and test-covered.

### Task 4: Add integration and runtime validation

**Files:**
- Modify: `crates/rpresent/tests/app_cli.rs`
- Modify: `crates/rpresent/tests/input.rs`
- Modify: `crates/rpresent-zoom/tests/overlay.rs`

- [ ] Expand overlay tests beyond `zoom_sample_rect` to cover state transitions relevant to rendering lifecycle.
- [ ] Add integration coverage to ensure app event loop forwards zoom updates to overlay channel when commands and input events occur.
- [ ] Keep tests deterministic: the `FrameSource` trait in `capture.rs` already supports injection of a test double. Tests should implement a `struct FakeFrameSource` that returns a fixed-size solid-color `CapturedFrame`, and pass it via `ZoomCapture::new(fake_source)`.

**Acceptance criteria:**
- `cargo test -p rpresent-zoom` passes.
- `cargo test -p rpresent --test input` passes.
- `cargo test -p rpresent` passes.

### Task 5: Operational verification and docs update

**Files:**
- Modify: `README.md`
- Modify: `config.toml` (only if example values need adjustment)

- [ ] Document expected daemon behavior clearly: run mode is idle until shortcuts/commands are triggered.
- [ ] Add zoom usage examples with visible-effect expectations (`zoom=on`, `zoom=reset`, mode toggle shortcut).
- [ ] Document known limitations (Linux/X11 only, compositor requirements).

**Acceptance criteria:**
- Running `cargo run -p rpresent -- --config ./config.toml` plus `Ctrl+3` and wheel produces visible zoom behavior.
- Sending `cargo run -p rpresent -- --config ./config.toml zoom=on` to a running daemon visibly enables zoom.

---

## Execution Order

1. Task 1 (overlay backend)
2. Task 2 (input/event wiring)
3. Task 3 (pan/follow stabilization)
4. Task 4 (tests for full behavior)
5. Task 5 (docs/operational polish)

Task 4 covers follow-cursor behavior defined in Task 3, so Task 3 must complete before Task 4.

---

## Verification Commands

```bash
cargo fmt
cargo test -p rpresent-zoom
cargo test -p rpresent --test input
cargo test -p rpresent
RUST_LOG=debug cargo run -p rpresent -- --config ./config.toml
```

Manual smoke checks while daemon is running:

```bash
cargo run -p rpresent -- --config ./config.toml zoom=on
cargo run -p rpresent -- --config ./config.toml zoom=reset
cargo run -p rpresent -- --config ./config.toml zoom=off
```

---

## Risks and Mitigations

- Risk: X11 capture path performance can be too expensive at high resolutions.
- Mitigation: keep first pass simple; add frame-change throttling or cache only if profiling shows need.

- Risk: wheel event modifier handling diverges across environments (`Ctrl` vs `Control`).
- Mitigation: keep normalization in one place (`input.rs`) with tests.

- Risk: regression in spotlight wheel handling when adding zoom wheel routing.
- Mitigation: retain and extend existing spotlight routing tests before merge.

---

## Definition of Done

- Zoom overlay is visibly rendered on Linux/X11 when enabled.
- Zoom command path and shortcut path both update visuals.
- Existing spotlight/annotation behaviors remain intact.
- All relevant tests pass and docs explain runtime expectations.
