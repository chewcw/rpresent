# Boomer-Style Zoom Port Plan (Rust)

## Goals
- Port boomer’s interaction model into `rpresent` zoom with acceptable breaking changes.
- Make zoom feel boomer-like:
  - continuous render loop (not event-only repaint)
  - velocity-based zoom (`delta_scale`) with friction
  - zoom-to-cursor pivot preservation (`p0 -> p1` correction)
  - drag-to-pan using X11 `MotionNotify`
- Keep implementation Linux/X11-first and minimal in moving parts.
- Prefer proven crates over custom image math.

## Non-Goals
- Wayland support in this phase.
- Flashlight effect parity with boomer.
- Full GPU renderer in first increment (can be optional follow-up).

## Recommended Crates (Do Not Reinvent)
- `x11rb` (already used): global grabs, pointer/motion events, XGetImage/PutImage, window control.
- `tiny-skia`: high-quality scaling and transforms for CPU path instead of handwritten nearest-neighbor loops.
- `crossbeam-channel` (already used): low-latency state/message handoff.
- Optional follow-up: `softbuffer` + `winit` for a cleaner frame presentation path, or `wgpu` for GPU path.

Rationale: `tiny-skia` removes fragile pixel-loop code while preserving X11 backend architecture and minimizing risk.

## Target Architecture

### 1) Input model (root app crate)
- Extend input routing to include drag lifecycle when zoom mode is active:
  - `ZoomDragStart { x, y }`
  - `ZoomDragMove { x, y }`
  - `ZoomDragEnd`
- Keep wheel modifier behavior for zoom/spotlight unchanged.

### 2) Zoom command/message protocol (zoom crate)
Breaking-change simplification: move away from “precomputed `ZoomState` only” updates toward intent messages consumed by overlay runtime.

Proposed internal message enum:
- `SetEnabled(bool)`
- `Scroll { delta, pivot }`
- `DragStart { x, y }`
- `DragMove { x, y }`
- `DragEnd`
- `Reset`
- `SetFollowCursor(bool)` (optional if config hot-reload is added later)

`ZoomController` remains in `rpresent-zoom`, but emits runtime-oriented state/messages rather than static snapshots only.

### 3) Camera model (boomer-like)
Introduce camera fields in overlay runtime:
- `position: (f32, f32)`
- `velocity: (f32, f32)`
- `scale: f32`
- `delta_scale: f32`
- `scale_pivot: (f32, f32)`
- drag state (`dragging`, `last_cursor`)

Per-frame update (`dt`):
- If `abs(delta_scale) > threshold`:
  - compute `p0 = (pivot - window_center)/scale`
  - update `scale = max(scale + delta_scale*dt, min_scale)`
  - compute `p1 = (pivot - window_center)/scale`
  - `position += p0 - p1` (cursor-locked zoom)
  - apply scale friction to `delta_scale`
- If not dragging and `|velocity| > threshold`: integrate pan velocity and apply drag friction.

### 4) Render loop (fixes non-live + follow-cursor idle)
Replace blocking `rx.recv()` render trigger with fixed timestep loop (e.g. ~60 FPS):
- Drain pending messages each tick (`try_recv` loop).
- If enabled:
  - update pointer pivot from `query_pointer()` each frame when `follow_cursor` is true.
  - update camera physics.
  - capture+render frame.
- Sleep for remaining frame budget.

This directly addresses:
- stale screenshot feeling due to event-only repaint
- follow_cursor not updating while idle

### 5) Capture/Render strategy
Two options; implement A now:

A) **Incremental-safe path** (recommended now)
- Keep per-frame `XGetImage` from root (live screen semantics).
- Replace manual nearest-neighbor scaling with `tiny-skia` transform pipeline.
- Present via `PutImage` to overlay window.

B) boomer-like static screenshot mode (optional switch)
- Capture once on enable; render transforms only until explicit refresh.
- Could be exposed as config mode later (`capture_mode = live|static`).

Given user-reported expectations (“live screen”), choose A as default.

## Config/Data Model Changes
- In `rpresent-config::ZoomConfig`, ensure/keep:
  - `min_scale`, `max_scale`, `scroll_speed`
  - `pan_friction` (already present)
  - `scale_friction` (add if missing in root workspace)
  - `follow_cursor`
- Document defaults aligned with boomer feel:
  - `scroll_speed`: moderate (e.g. 0.25)
  - `scale_friction`: around 5.0 (tunable)
  - `pan_friction`: around 4.0 equivalent damping semantics (or adapt existing meaning)

## File-Level Edit Map

### `crates/rpresent/src/input.rs`
- Add zoom drag input actions.
- Keep active mode gating; only emit drag actions in `ActiveMode::Zoom`.

### `crates/rpresent/src/x11_input.rs`
- Add passive/active handling for button1 press/release and motion events.
- Emit `AppEvent::Input(ZoomDragStart/Move/End)` when in zoom mode.
- Preserve existing wheel/key behavior.

### `crates/rpresent/src/app.rs`
- Route new drag input actions to `ZoomController`.
- Send updated zoom runtime messages to overlay channel.

### `crates/rpresent-zoom/src/controller.rs`
- Introduce boomer-like zoom intent handling:
  - on scroll: accumulate `delta_scale`, set pivot
  - on drag move: set/add pan velocity or direct camera offset seed
- Expose runtime state needed by overlay.

### `crates/rpresent-zoom/src/overlay.rs`
- Replace recv-blocking loop with frame loop + message drain.
- Keep Linux guard behavior.

### `crates/rpresent-zoom/src/x11.rs`
- Add camera physics update and cursor-lock correction.
- Query pointer each frame if `follow_cursor`.
- Integrate `tiny-skia` scaling/composition.

### `crates/rpresent-zoom/Cargo.toml`
- Add `tiny-skia`.
- Keep `x11rb`.

### `crates/rpresent-config/src/config.rs` (+ tests)
- Add `scale_friction` field/default/serde if absent.
- Ensure config round-trip tests updated.

### Tests
- `crates/rpresent/tests/input.rs`: drag action routing in zoom mode.
- `crates/rpresent-zoom/tests/controller.rs`: velocity/friction/pivot-correction behavior at controller layer (or camera unit tests if moved).
- `crates/rpresent-zoom/tests/overlay.rs`: frame-loop behavior sanity (message drain + no deadlock).

## Phased Implementation Sequence

1. **Protocol & plumbing**
- Add drag input actions and app routing.
- Add/adjust zoom messages/controller APIs.

2. **Render loop refactor**
- Convert overlay loop to fixed-timestep frame loop with message draining.

3. **Camera physics port**
- Implement boomer-style `delta_scale`, friction, pivot correction.

4. **Drag-to-pan**
- Wire MotionNotify -> controller -> camera velocity/position.

5. **tiny-skia integration**
- Replace manual scaling codepath with `tiny-skia` transforms.

6. **Config + tests + tuning**
- Add `scale_friction` if needed.
- Update tests and defaults.
- Tune feel constants.

## Risks & Mitigations
- Event-grab conflicts with global shortcuts:
  - Mitigate by limiting drag grabs to zoom-active periods and careful ungrab on end.
- CPU usage from 60 FPS live capture:
  - Mitigate with adaptive rendering (skip frame if state unchanged and follow_cursor=false).
- Cross-display/root coordinate mismatches:
  - Mitigate by testing with multi-monitor and clamping sample rect.

## Validation Checklist
- Ctrl+wheel zoom in/out works in zoom mode.
- Zoom continues to update smoothly after wheel stops (friction decay).
- Pixel under cursor remains stable while zooming (pivot correction).
- `follow_cursor=true`: moving cursor without scrolling updates focal area.
- Drag with left mouse button pans scene naturally; release stops drag.
- Overlay enable/disable stable; no stuck grabs.
- `cargo test --workspace` passes.

## Rollback Strategy
- Keep protocol changes isolated in commits:
  1) input/message plumbing
  2) render loop
  3) camera physics + drag
  4) tiny-skia render path
- If regressions occur, disable new path behind temporary config flag `zoom.experimental_boomer_model` and fall back to previous renderer.

---

## Execution Checklist (Ordered)
- [ ] Add drag input actions and route through `AppEvent`.
- [ ] Capture Button1/Motion/Release in X11 input loop for zoom mode.
- [ ] Refactor `ZoomController` API to support scroll accumulation and drag intents.
- [ ] Convert zoom overlay thread to fixed-timestep loop with message draining.
- [ ] Port boomer camera update math (`delta_scale`, friction, `p0/p1` correction).
- [ ] Query pointer each frame when `follow_cursor`.
- [ ] Integrate `tiny-skia` transform-based scaling/composition.
- [ ] Add/update config (`scale_friction`) and serde defaults/tests as needed.
- [ ] Add/adjust unit tests for input routing + controller physics invariants.
- [ ] Run `cargo test --workspace` and sanity-check runtime behavior on X11.
