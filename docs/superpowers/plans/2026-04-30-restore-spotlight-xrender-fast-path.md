# Restore Spotlight XRender Fast Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recover the cursor-movement performance of commit `54ace59` by restoring the proven XRender radial spotlight fast path for supported visuals, while keeping the current Projecteur software renderer as the fallback for unsupported or fidelity-critical cases.

**Architecture:** Keep `overlay.rs` as a parameter-driven runtime that passes cursor and surface state into the backend. Reintroduce an explicit backend strategy split in `x11.rs`: one branch uses XRender radial composition for the supported subset that matches `54ace59`, and the other branch uses the existing `render.rs` plus `PutImage` software path. The XRender branch should stay intentionally narrow at first; expand it only after it demonstrably matches the known-good performance.

**Tech Stack:** Rust 2024, `x11rb` with `render` and `image`, `tiny-skia`, `tracing`, `cargo test`.

---

## File Structure

- `crates/rpresent-spotlight/src/x11.rs`
  Restore the backend strategy split, reintroduce the XRender radial helpers, and route configs between the fast path and the software fallback.
- `crates/rpresent-spotlight/src/overlay.rs`
  Keep the runtime/backend boundary parameter-driven so the app loop does not own full-frame rasterization.
- `crates/rpresent-spotlight/src/lib.rs`
  Keep the public exports aligned with the final runtime/backend contract.
- `crates/rpresent-spotlight/tests/overlay.rs`
  Preserve the runtime seam tests and add coverage for render-strategy selection if any part of it stays pure.
- `specs/2026-04-30-projecteur-spotlight-overlay-port.md`
  Track the approved requirement change and status of the fast-path restoration.
- `specs/PERFORMANCE_ANALYSIS.md`
  Record measured results against `54ace59` after implementation.

---

### Task 1: Lock the XRender eligibility policy

**Files:**
- Modify: `crates/rpresent-spotlight/src/x11.rs`
- Modify: `crates/rpresent-spotlight/tests/overlay.rs`

- [ ] **Step 1: Write failing tests for strategy selection**

`crates/rpresent-spotlight/src/x11.rs`

```rust
#[cfg(test)]
mod strategy_tests {
    use super::*;

    #[test]
    fn default_circle_prefers_xrender_radial() {
        let config = SpotlightConfig::default();

        assert!(matches!(
            select_render_strategy(&config),
            RenderStrategy::XrenderRadial
        ));
    }

    #[test]
    fn border_or_dot_falls_back_to_software() {
        let mut config = SpotlightConfig::default();
        config.show_border = true;
        assert!(matches!(
            select_render_strategy(&config),
            RenderStrategy::SoftwareFrame
        ));

        config.show_border = false;
        config.show_center_dot = true;
        assert!(matches!(
            select_render_strategy(&config),
            RenderStrategy::SoftwareFrame
        ));
    }
}
```

- [ ] **Step 2: Run the targeted tests to confirm they fail**

Run: `cargo test -p rpresent-spotlight strategy_tests --lib`

Expected: FAIL with unresolved names for `RenderStrategy` and `select_render_strategy`.

- [ ] **Step 3: Add the minimal strategy selector**

`crates/rpresent-spotlight/src/x11.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderStrategy {
    XrenderRadial,
    SoftwareFrame,
}

fn select_render_strategy(config: &SpotlightConfig) -> RenderStrategy {
    let default_circle = matches!(config.shape, rpresent_config::SpotShape::Circle);
    let fidelity_only = config.show_border || config.show_center_dot || config.spot_rotation_deg != 0.0;

    if default_circle && !fidelity_only {
        RenderStrategy::XrenderRadial
    } else {
        RenderStrategy::SoftwareFrame
    }
}
```

- [ ] **Step 4: Re-run the targeted tests**

Run: `cargo test -p rpresent-spotlight strategy_tests --lib`

Expected: PASS.

- [ ] **Step 5: Commit the strategy policy**

```bash
git add crates/rpresent-spotlight/src/x11.rs crates/rpresent-spotlight/tests/overlay.rs
git commit -m "test: lock spotlight fast-path strategy selection"
```

### Task 2: Restore the pure XRender helper layer from `54ace59`

**Files:**
- Modify: `crates/rpresent-spotlight/src/x11.rs`

- [ ] **Step 1: Write failing helper tests**

`crates/rpresent-spotlight/src/x11.rs`

```rust
#[test]
fn alpha_to_u16_scales_full_opacity() {
    assert_eq!(alpha_to_u16(0.0), 0);
    assert_eq!(alpha_to_u16(1.0), u16::MAX);
}

#[test]
fn legacy_appearance_uses_legacy_radius_fields() {
    let config = SpotlightConfig {
        radius_px: 300,
        softness_px: 90,
        dim_alpha: 0.8,
        ..SpotlightConfig::default()
    };

    let appearance = legacy_appearance(&config);

    assert_eq!(appearance.radius_px, 300);
    assert_eq!(appearance.softness_px, 90);
    assert!((appearance.dim_alpha - 0.8).abs() < f32::EPSILON);
}
```

- [ ] **Step 2: Run the targeted tests to confirm they fail**

Run: `cargo test -p rpresent-spotlight alpha_to_u16 --lib`

Expected: FAIL with unresolved names for `alpha_to_u16` and `legacy_appearance`.

- [ ] **Step 3: Restore the pure helpers and the radial gradient builder**

`crates/rpresent-spotlight/src/x11.rs`

```rust
fn alpha_to_u16(alpha: f32) -> u16 {
    (alpha.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
}

fn legacy_appearance(config: &SpotlightConfig) -> crate::overlay::SpotlightAppearance {
    crate::overlay::SpotlightAppearance {
        radius_px: config.radius_px,
        softness_px: config.softness_px,
        dim_alpha: config.dim_alpha,
    }
}

// Restore these helpers from 54ace59 with only the minimum compile-fix edits needed:
// - find_visual_format(...)
// - fixed_from_i16(...)
// - fixed_from_u32(...)
// - create_gradient_picture(...)
```

- [ ] **Step 4: Run the library tests after restoring the helpers**

Run: `cargo test -p rpresent-spotlight --lib`

Expected: PASS.

- [ ] **Step 5: Commit the XRender helper restoration**

```bash
git add crates/rpresent-spotlight/src/x11.rs
git commit -m "feat: restore spotlight XRender radial helpers"
```

### Task 3: Reintroduce the backend split in `x11.rs`

**Files:**
- Modify: `crates/rpresent-spotlight/src/x11.rs`
- Modify: `crates/rpresent-spotlight/src/overlay.rs`
- Modify: `crates/rpresent-spotlight/src/lib.rs`

- [ ] **Step 1: Add a failing runtime seam assertion**

`crates/rpresent-spotlight/src/overlay.rs`

```rust
#[test]
fn runtime_keeps_delegating_redraw_decisions_to_the_backend() {
    let backend = FakeBackend {
        surface_size: (16, 16),
        cursor: CursorPosition { x: 8, y: 8 },
        ..Default::default()
    };
    let mut runtime = SpotlightOverlayRuntime::new(rpresent_config::SpotlightConfig::default(), backend);

    runtime.process_state(SpotlightState::On).unwrap();

    assert_eq!(runtime.backend.calls, vec!["show", "redraw"]);
}
```

- [ ] **Step 2: Run the spotlight package tests as a baseline**

Run: `cargo test -p rpresent-spotlight`

Expected: PASS.

- [ ] **Step 3: Restore the explicit backend split and route by `RenderStrategy`**

`crates/rpresent-spotlight/src/x11.rs`

```rust
enum BackendKind {
    Render { window_picture: Picture },
    Software { gc: Gcontext, depth: u8 },
}

fn redraw(
    &mut self,
    cursor: CursorPosition,
    config: &SpotlightConfig,
) -> Result<(), OverlayError> {
    match select_render_strategy(config) {
        RenderStrategy::XrenderRadial => {
            let appearance = legacy_appearance(config);
            redraw_render(
                &self.connection,
                self.window,
                self.window_picture.expect("created for render backend"),
                self.surface_size,
                cursor,
                &appearance,
            )?;
            self.flush()
        }
        RenderStrategy::SoftwareFrame => {
            let frame = crate::render::build_projecteur_frame(self.surface_size, cursor, config);
            self.redraw_frame(&frame)
        }
    }
}
```

- [ ] **Step 4: Run the focused validation**

Run: `cargo test -p rpresent-spotlight`

Expected: PASS.

- [ ] **Step 5: Commit the backend split**

```bash
git add crates/rpresent-spotlight/src/x11.rs crates/rpresent-spotlight/src/overlay.rs crates/rpresent-spotlight/src/lib.rs
git commit -m "feat: route supported spotlight configs through XRender"
```

### Task 4: Validate against `54ace59` and record the result

**Files:**
- Modify: `specs/PERFORMANCE_ANALYSIS.md`
- Modify: `specs/2026-04-30-projecteur-spotlight-overlay-port.md`

- [ ] **Step 1: Run the full package tests**

Run:

```bash
cargo test -p rpresent-spotlight
cargo test -p rpresent
```

Expected: PASS.

- [ ] **Step 2: Manually compare the restored fast path with `54ace59`**

Record the same cursor-movement scenario at 1920x1080 and 3840x2160 for:
- `54ace59`
- current branch after Task 3

Collect:
- approximate CPU while moving the cursor continuously for 10 seconds
- subjective cursor smoothness
- whether the default circle path materially matches the known-good commit

- [ ] **Step 3: Record the measurements in the analysis docs**

`specs/PERFORMANCE_ANALYSIS.md`

```md
## Validation Results

- Reference commit: `54ace59`
- Restored fast path commit: `<new commit>`
- 1920x1080 moving-cursor CPU: `<measured value>`
- 3840x2160 moving-cursor CPU: `<measured value>`
- Result: `<matched / improved / still slower>`
```

- [ ] **Step 4: Commit the validation notes**

```bash
git add specs/PERFORMANCE_ANALYSIS.md specs/2026-04-30-projecteur-spotlight-overlay-port.md
git commit -m "docs: record spotlight fast-path validation"
```

---

## Self-Review

- Spec coverage: This plan implements the approved requirement change, restores the proven `54ace59` fast path, keeps the Projecteur software renderer as fallback, and requires validation against the known-good commit.
- Placeholder scan: The only abbreviated instruction is “restore from `54ace59`,” and it is intentional because that commit is the verified behavioral reference for the XRender helper code.
- Type consistency: The runtime/backend contract stays `cursor + config`; the XRender branch converts to `SpotlightAppearance` only inside the supported fast path, and the software branch continues to use `build_projecteur_frame`.
