# Projecteur Spotlight Overlay Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port Projecteur's spotlight overlay renderer into Rust with one-to-one visual behavior for the shade, shape cutout, border ring, and center dot, while keeping device input, command parsing, Qt/QML, and zoom out of scope.

**Architecture:** Build a pure Rust frame compositor that resolves the current scene into a premultiplied RGBA buffer and hands that buffer to the X11 transport layer. Use `tiny-skia` for antialiased vector mask rasterization, because Projecteur's visuals are path-based shapes rather than a radial falloff. Cache shape paths, scale factors, and scratch pixmaps so redraws reuse memory instead of rebuilding full-screen frames from scratch. Keep `x11.rs` focused on window lifecycle, cursor query, and image upload only. Introduce Projecteur-style render settings in `rpresent_config` so the renderer receives the same shape, rotation, border, dot, and shade inputs the QML scene used.

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
- Do keep the X11 boundary thin; visuals must come from Rust rasterization, not from XRender gradients.
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

### Task 3: Keep X11 as transport only

**Files:**
- Modify: `crates/rpresent-spotlight/src/x11.rs`
- Modify: `crates/rpresent-spotlight/src/overlay.rs`
- Modify: `crates/rpresent-spotlight/tests/overlay.rs`

- [x] Replace the current gradient-generation path with the Rust renderer so X11 never owns the visual semantics.
- [x] Keep the existing x11rb window setup, visual selection, and cursor query logic, but make redraw consume the already-built RGBA buffer.
- [x] If an XRender fast path stays, use it only to present the pre-rasterized buffer; do not keep `create_gradient_picture` or any semantic substitute on the X11 side.
- [x] Reuse a persistent client-side image buffer or pixmap wrapper so the upload path does not reallocate on every tick.
- [x] Preserve the current show/hide runtime behavior and the state-transition tests; only the frame construction changes.
- [x] Run `cargo test -p rpresent-spotlight` → 24/24 pass (36/36 including rpresent).
- [ ] Commit the transport-only change separately from the renderer.

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