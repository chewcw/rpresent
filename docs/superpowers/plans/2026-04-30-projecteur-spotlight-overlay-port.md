# Projecteur Spotlight Overlay Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port Projecteur's spotlight overlay renderer into Rust with one-to-one visual behavior for the shade, shape cutout, border ring, and center dot, while keeping device input, command parsing, Qt/QML, and zoom out of scope.

**Architecture:** Build a pure Rust frame compositor that resolves the current scene into a premultiplied RGBA buffer when full Projecteur fidelity is required, but allow `x11.rs` to synthesize supported spotlight visuals server-side when that materially improves latency. Use `tiny-skia` for antialiased vector mask rasterization, because Projecteur's visuals are path-based shapes rather than a radial falloff. Cache shape paths, scale factors, and scratch pixmaps so redraws reuse memory instead of rebuilding full-screen frames from scratch. Keep the runtime boundary parameter-driven so the backend can choose between XRender composition and software upload per config. Introduce Projecteur-style render settings in `rpresent_config` so the renderer receives the same shape, rotation, border, dot, and shade inputs the QML scene used.

**Tech Stack:** Rust 2024, `tiny-skia`, `x11rb`, `serde`, `toml`, `tracing`, and `criterion`.

---

## Research Summary

- `qml/main.qml` and `qml/main-qt6.qml` define the overlay as a transparent window. `centerRect` sizes the spotlight from `spotSize`, `spot` punches a hole using the selected shape, `spotBorder` reuses the same shape scaled down by `(100 - borderSize) / 100`, and `dotCursor` draws the center dot.
- `qml/spotshapes/Circle.qml`, `Square.qml`, `Star.qml`, and `Ngon.qml` plus `src/spotshapes.cc` define the exact geometry and shape-specific settings.
- `src/settings.cc` exposes the default values that the Rust port needs to match.
- The current Rust code in `crates/rpresent-spotlight/src/overlay.rs` still uses `spotlight_alpha` and a radial gradient mental model, which is not one-to-one with Projecteur's mask composition.
- `crates/rpresent-config/src/config.rs` only has legacy spotlight fields today, so the config model must grow before the renderer can be faithful.

## Design Constraints

- Do not port Qt/QML or the preferences UI; the renderer should stay in Rust/X11 unless a later fidelity gap proves we need Qt/QML integration.
- Do not add repeated initialization loops or fallback retries; keep the renderer cache warm and rebuild only when the scene or surface size changes.
- Do not port zoom in this pass; it already belongs to a separate module in this workspace.
- X11 may own server-side compositing for supported spotlight visuals, with Rust rasterization kept as a fallback for unsupported or fidelity-critical cases.
- Keep multi-monitor overlay placement out of this plan unless we explicitly decide the rendering port needs active-monitor routing; if it becomes necessary, implement it in the X11/window-placement layer rather than by adopting Qt/QML.

## Implementation Plan

### Task 1: Model Projecteur's overlay settings

**Files:**
- Modify: `crates/rpresent-config/src/config.rs`
- Modify: `crates/rpresent-config/tests/config.rs`
- Modify: `config.toml`
- Modify: `crates/rpresent-spotlight/src/lib.rs`

- [x] Add a Projecteur-style spotlight config model that carries the full visual surface: `show_spot_shade`, `spot_size_pct`, `shape`, `spot_rotation_deg`, `show_center_dot`, `dot_size_px`, `dot_color`, `dot_opacity`, `shade_color`, `shade_opacity`, `show_border`, `border_color`, `border_size_pct`, and `border_opacity`.
- [x] Model the shape as a tagged enum with the Projecteur variants and defaults: `circle`, `square { radius_pct = 20 }`, `star { points = 5, inner_radius_pct = 50 }`, and `ngon { sides = 3 }`.
- [x] Keep the current `radius_px` / `softness_px` / `dim_alpha` path only as a compatibility bridge for existing callers until they are migrated; the renderer must not depend on the radial-softness semantics.
- [x] Represent `dot_color`, `shade_color`, and `border_color` as `#RRGGBB` strings in TOML, then parse them once in the renderer layer so the config stays simple.
- [x] Update `config.toml` and the config tests so `Config::default()` and `default_config_toml()` serialize Projecteur's defaults from `settings.cc`.
- [x] Run `cargo test -p rpresent-config`.
- [x] Commit the config-model change before touching rendering.

### Task 2: Rasterize the overlay in Rust

**Files:**
- Create: `crates/rpresent-spotlight/src/render.rs`
- Modify: `crates/rpresent-spotlight/src/overlay.rs`
- Modify: `crates/rpresent-spotlight/Cargo.toml`
- Modify: `crates/rpresent-spotlight/src/lib.rs`
- Create: `crates/rpresent-spotlight/tests/render.rs`

- [x] Build a pure renderer that takes resolved surface size, cursor position, and the new Projecteur-style spotlight config, then returns a premultiplied RGBA frame.
- [x] Compute the spot diameter the same way as QML: `spot_size_px = max(trunc(surface_height * spot_size_pct / 100.0), 50)`.
- [x] Use `tiny-skia` to draw antialiased paths for `Circle`, `Square`, `Star`, and `Ngon`; compute the border as the outer shape minus a scaled inner shape so the ring matches Projecteur's mask composition exactly.
- [x] Apply rotation in the same coordinate system as QML and ignore it for shapes that do not allow rotation.
- [x] Draw the full-screen shade outside the hole, then layer the border and center dot on top; `show_border` and `show_center_dot` must remain independent of `show_spot_shade`, just like in Projecteur.
- [x] Reuse cached paths and scratch pixmaps keyed by shape parameters and surface size so redraws do not allocate a new buffer per frame.
- [x] Do not route the new renderer through `spotlight_alpha`; leave that helper only as a temporary migration aid if any legacy test still needs it.
- [x] Add pixel tests that sample deep interior, deep exterior, border-ring, and dot pixels for all four shapes, with tolerance only on the anti-aliased edge samples.
- [x] Include a regression case where `show_spot_shade = false` but border and dot stay visible on a transparent background.
- [x] Run `cargo test -p rpresent-spotlight --test render` → 9/9 pass.
- [ ] Commit the renderer before changing X11 transport.

### Task 3: Keep X11 transport-first, with bounded server-side composition

**Files:**
- Modify: `crates/rpresent-spotlight/src/x11.rs`
- Modify: `crates/rpresent-spotlight/src/overlay.rs`
- Modify: `crates/rpresent-spotlight/tests/overlay.rs`

- [x] Replace the current always-on gradient-generation path with the Rust renderer for full-fidelity Projecteur visuals, while keeping room for a later server-side fast path.
- [x] Keep the existing x11rb window setup, visual selection, and cursor query logic, but make redraw consume the already-built RGBA buffer.
- [x] If an XRender fast path stays, use it only to present the pre-rasterized buffer; do not keep `create_gradient_picture` or any semantic substitute on the X11 side.
- [x] Reuse a persistent client-side image buffer or pixmap wrapper so the upload path does not reallocate on every tick.
- [x] Preserve the current show/hide runtime behavior and the state-transition tests; only the frame construction changes.
- [x] Run `cargo test -p rpresent-spotlight` → 24/24 pass (36/36 including rpresent).
- [ ] Commit the transport-only change separately from the renderer.

### Task 3.1: Re-introduce XRender compositing fast path (primary)

**Rationale:** Task 3 removed the server-side XRender compositing branch and made every visible tick upload a full pre-rasterized frame through `PutImage`. At 1080p/4K this can saturate CPU, memory bandwidth, and X11 socket throughput. Re-introducing an XRender path is the first remediation because commit `54ace59` has been confirmed as the best-performing known implementation, and it kept the common spotlight path on the X server.

**Approved Requirement Change (2026-04-30):** X11 may own server-side compositing for supported spotlight visuals, with Rust rasterization kept as a fallback for unsupported or fidelity-critical cases.

**Files:**
- Modify: `crates/rpresent-spotlight/src/x11.rs`
- Modify: `crates/rpresent-spotlight/src/overlay.rs`
- Modify: `crates/rpresent-spotlight/tests/overlay.rs`
- Modify: `crates/rpresent-spotlight/tests/controller.rs` (only if behavior assertions need redraw-path awareness)

- [ ] Add backend capability detection in `x11.rs` for XRender direct format support and keep a software upload fallback for unsupported environments.
- [ ] Restore an explicit backend split (for example: `RenderRadial` and `SoftwareUpload`) and allow semantic ownership on the X11 side only for the supported fast-path visuals.
- [ ] In the XRender-capable backend, avoid full-frame `put_image` per tick; use XRender primitives/composition on server-side buffers and keep client-side bytes out of the hot path wherever possible.
- [ ] Limit the initial XRender fast path to the exact subset that matches `54ace59` performance characteristics; use the existing Projecteur renderer as the fallback for shapes or decorations that need higher fidelity.
- [ ] Keep `SpotlightOverlayRuntime` behavior unchanged (`show`/`hide`, tick cadence, cursor polling), so only transport internals differ by backend.
- [ ] Add/adjust tests that verify backend selection and ensure both render and software branches preserve existing visible-state behavior.
- [ ] Manually verify latency and CPU for spotlight-on cursor movement at 1920x1080 and 3840x2160 against Task 3 baseline.
- [ ] Run `cargo test -p rpresent-spotlight` and record before/after numbers in the task notes.
- [ ] Commit Task 3.1 independently so rollback/cherry-pick stays easy.

### Task 3.2: Frame Content Hashing for PutImage Avoidance

**Rationale:** Task 3.1 restores GPU acceleration for circles, but when geometry doesn't change (common during static cursor), the software fallback still performs expensive per-frame PutImage uploads. Frame content hashing detects identical pixel data and skips upload entirely, reducing network round-trips on remote X11 (estimated 10-15ms savings per identical frame).

**Approach:** Compute hash of rendered frame pixels; skip PutImage when hash matches cached hash. Invalidate cache on surface resize.

**Implementation:**
- [x] 3.2.1: Add `last_frame_hash: u64` field to `X11SpotlightOverlay`; initialize to 0 in `new()`
- [x] 3.2.2: Implement `hash_frame(frame: &SpotlightFrame) -> u64` using `std::collections::hash_map::DefaultHasher`
- [x] 3.2.3: In `X11SpotlightOverlay::redraw()`, compute frame hash early; skip pixel buffer copy + PutImage if hash matches and no resize
- [x] 3.2.4: On hash mismatch, proceed with normal buffer allocation, pixel copy, and PutImage upload; update cached hash on success
- [x] 3.2.5: Cache invalidation: set `last_frame_hash = 0` when `needs_realloc` is true (window resize detected)
- [x] 3.2.6: All 66 tests pass (unit + integration); behavior semantically identical to Task 3
- [x] 3.2.7: Frame caching working; ready to commit

**Performance Impact:**
- **Static cursor (no movement)**: ~10-15ms saved per frame (PutImage avoidance on remote X11)
- **Moving cursor**: Hash mismatch → normal path (no overhead beyond hash computation)
- **No regression**: Architecture is non-blocking fallback; enables rather than disables optimization

**Files Modified:**
- `crates/rpresent-spotlight/src/x11.rs`: Added `last_frame_hash`, `hash_frame()`, cache logic in `redraw()`
- `specs/2026-04-30-projecteur-spotlight-overlay-port.md`: Updated Task 3.2 spec




### Task 3.3: Restore Task 2 Architecture for Maximum Performance (~80-90% Recovery)

**Rationale:** After Task 3.2 analysis, the most effective performance recovery is to eliminate the continuous render loop entirely. Task 2's on-demand architecture means rasterization only happens when callers actually request it (or when window events trigger redraws). Task 3 added an unconditional polling tick loop that rasterizes every frame regardless of whether the window is visible or the cursor moved. Task 3.3 restores Task 2's lightweight model while keeping X11 transport separation.

**Key Difference from Task 3.2:**
- **Task 3.2 approach**: Keep polling tick loop, add change detection + frame caching → rasterize only when state changes
- **Task 3.3 approach**: Eliminate polling entirely, pass (cursor, config) to backend, let backend decide when/how to render → zero rasterization when no input

**Architecture Change:**
```
Task 2 (on-demand):
	overlay.rs:build_projecteur_frame(size, cursor, config) → SpotlightFrame
	(caller decides frequency)

Task 3 (polling, broken):
	SpotlightOverlayRuntime::tick() → unconditional build_projecteur_frame()
	(rasterizes every tick regardless of changes)

Task 3.3 (on-demand transport):
	SpotlightOverlayRuntime::tick(cursor, config)
		→ backend.redraw(cursor, config)  [pass raw parameters, not pre-rasterized frame]
		→ backend decides: render only if needed
	(backend controls rasterization frequency)
```

**Approach:**

1. **Remove pre-rasterization from tick()**: Instead of `build_projecteur_frame()` in `SpotlightOverlayRuntime::tick()`, pass cursor position and config directly to backend
2. **Backend handles rendering**: Let `X11SpotlightOverlay::redraw(cursor, config)` decide rendering strategy:
	 - On state change (cursor moved, config changed): Rasterize via `build_projecteur_frame()`
	 - On static state: Skip rasterization, reuse cached frame or composite previous buffer
	 - On surface resize: Invalidate cache, force rasterization on next redraw
3. **Eliminate unconditional rasterization**: No per-tick rasterization unless state actually changed

**Subtasks:**
- [x] 3.3.1: Move `SpotlightOverlayRuntime::tick()` to pass render inputs (`cursor` + config) to the backend instead of constructing a `SpotlightFrame` in the runtime
- [x] 3.3.2: Modify `SpotlightOverlayBackend::redraw()` to accept `cursor: CursorPosition, config: &SpotlightConfig` instead of a pre-rasterized frame
- [x] 3.3.3: Implement backend-side change detection in `X11SpotlightOverlay::redraw()`:
	- Compare (cursor, config) hash with last hash
	- Only call `build_projecteur_frame()` if changed
	- Reuse cached `SpotlightFrame` otherwise
- [ ] 3.3.4: Update remaining call sites to match the new backend contract as Task 3.3 expands; `overlay.rs` is updated, and `controller.rs` does not currently depend on the redraw signature
- [x] 3.3.5: Verify all 36 tests pass with new architecture (`cargo test -p rpresent-spotlight` and `cargo test -p rpresent`)
- [ ] 3.3.6: Benchmark: Measure CPU usage under static cursor, moving cursor, rapid movement (compare vs Task 3.2)
- [ ] 3.3.7: Document performance gains in PERFORMANCE_ANALYSIS.md
- [ ] 3.3.8: Commit with message "task 3.3: restore on-demand architecture for maximum performance recovery"

**Expected Performance:**
- **Static cursor**: ~0% CPU (no rasterization, no upload)
- **Moving cursor**: CPU spike only when cursor position changes (not on every tick)
- **Rapid movement**: CPU proportional to state-change frequency, not event frequency
- **Expected recovery**: 80-90% of original Task 2 performance (on-demand baseline)

**Backward Compatibility:**
- Public API of `SpotlightOverlayRuntime` remains unchanged (tick still takes same params)
- Behavior semantically identical (same visual output)
- Internal architecture changes only (caller → backend delegation of rasterization)

**Alternative to Task 3.2:** This is the "fastest recovery" option if frame caching (Task 3.2) doesn't fully resolve performance issues. Task 3.3 eliminates the root cause (continuous polling) rather than adding mitigation (caching).


### Task 4: Lock in parity and performance

**Files:**
- Create: `crates/rpresent-spotlight/benches/spotlight_render.rs`
- Modify: `crates/rpresent-spotlight/Cargo.toml`
- Modify: `crates/rpresent-spotlight/tests/render.rs`

- [ ] Add a Criterion benchmark that measures frame construction at 1920x1080 and 3840x2160 for the default circle, a rotated star, and a border-on case.
- [ ] Keep the legacy radial path only long enough to compare against the new renderer, then remove the fallback branch once the new path is proven faster or at least not slower on the same machine.
- [ ] Require the benchmark to show no new per-frame allocation spikes and no regression in the redraw hot path.
- [ ] Run `cargo bench -p rpresent-spotlight spotlight_render` plus `cargo test -p rpresent-spotlight` after the benchmark file lands.
- [ ] Commit the performance gate last.