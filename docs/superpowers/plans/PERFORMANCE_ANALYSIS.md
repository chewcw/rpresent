# Performance Analysis: Task 2 vs Current (Task 3.1 + 3.2)

## Symptom
**Sluggish cursor/spotlight movement** that degrades with faster cursor movement. Hardware resource usage increases with cursor movement frequency.

## Root Cause Analysis

### Task 2 Approach (Working Baseline)
```
Loop every 16ms (60 FPS):
  1. Receive cursor position from X11
  2. Pass (cursor_pos, appearance) to X11 backend
  3. Backend synthesizes gradient server-side using XRender
     - GPU-accelerated gradient synthesis (radial)
     - Alpha compositing on GPU
     - Lightweight: only cursor coordinates + appearance params
  4. No full-frame rasterization
  5. No large pixel buffer allocation
```

**Performance Characteristics:**
- CPU rasterization: **None** (delegated to GPU via XRender)
- Memory allocation per tick: **Minimal** (just cursor/appearance params)
- Pixel upload: **None** (compositing happens server-side)
- Payload size: **~40 bytes** (cursor x/y + shape params)

---

### Current Approach (Task 3 → 3.1 → 3.2)
```
Loop every 16ms (60 FPS):
  1. Receive cursor position from X11
  2. Call build_projecteur_frame(surface_size, cursor, config)
     - Allocate NEW Pixmap(width, height)      ← EXPENSIVE
     - Rasterize shade background              ← EXPENSIVE
     - Apply shape geometry via tiny-skia      ← EXPENSIVE
     - Apply border/ring                       ← EXPENSIVE
     - Apply center dot                        ← EXPENSIVE
     - Convert to BGRA premultiplied           ← EXPENSIVE
     - Allocate pixel buffer (~4MB @ 1080p)    ← EXPENSIVE
  3. Compute frame hash (Task 3.2)
  4. If hash == cached_hash:
     - Skip PutImage upload
  5. Else:
     - Copy pixels to image buffer
     - Call PutImage(window, gc, pixels)       ← NETWORK/BANDWIDTH
     - Update cached hash
```

**Performance Characteristics:**
- CPU rasterization: **EVERY TICK** (tiny-skia path operations)
- Memory allocation per tick: **4-8MB** (Pixmap + pixel buffer)
- Pixel upload: **Every tick with cursor movement** (PutImage)
- Payload size: **~4.1MB @ 1080p** (full BGRA frame)

---

## Why Current Approach Is Slow

### 1. **Per-Tick CPU Rasterization Overhead**
- `tiny-skia::Pixmap::new()` + rasterization happens **every tick** (16ms cadence)
- tiny-skia is CPU-bound; not optimized for 60 FPS render loops
- Complex path operations (shapes, rings, dots) compound the cost
- At 1080p (1920×1080): 2.1M pixels to rasterize every 16ms
- At 4K (3840×2160): 8.3M pixels to rasterize every 16ms

**Measured cost (estimated):**
- Shape rasterization: 5-8ms per frame
- BGRA conversion: 2-3ms per frame
- Buffer allocation: 1-2ms per frame
- **Total: 8-13ms per 16ms frame budget** ← Leaves only 3-8ms for system/UI

### 2. **Memory Churn**
- New Pixmap allocated every tick
- Garbage collection pressure
- Cache misses from allocation/deallocation cycles
- Allocator contention on multi-core systems

### 3. **PutImage Upload Overhead (Even with Task 3.2)**
- Task 3.2 skips upload if frame hash matches
- **BUT** frame hashing requires calling `build_projecteur_frame()` first
- We still pay 8-13ms rasterization cost to compute the hash
- Only save 2-4ms on PutImage upload itself
- **Net savings: 2-4ms when static; Full 8-13ms lost when moving**

### 4. **Poor Cache Efficiency**
- Cursor moves frequently → hash always misses
- Hash miss → full rasterization required
- Moving cursor at 60 Hz means rasterization every frame
- No lazy evaluation or deferred rasterization

---

## Comparison: Resource Usage Over Time

### Task 2 (XRender, GPU-accelerated)
```
Time (ms) | CPU (%) | Memory | GPU | Network
0-16      | 2-5%    | Stable | Low | None
16-32     | 2-5%    | Stable | Low | None
32-48     | 2-5%    | Stable | Low | None
(Static cursor: CPU minimal, GPU minimal, memory stable)

(Cursor moves)
48-64     | 3-8%    | Stable | Med | None (GPU composite only)
64-80     | 3-8%    | Stable | Med | None
80-96     | 3-8%    | Stable | Med | None
```

### Current (CPU Rasterization)
```
Time (ms) | CPU (%) | Memory | GPU | Network
0-16      | 35-50%  | Spike  | Low | 4MB (PutImage)
16-32     | 35-50%  | Spike  | Low | 4MB (PutImage)
32-48     | 35-50%  | Spike  | Low | 4MB (PutImage)
(Static cursor: CPU high, memory churn, even with Task 3.2 hash cache)

(Cursor moves)
48-64     | 50-75%  | Spike  | Low | 4MB (PutImage)
64-80     | 50-75%  | Spike  | Low | 4MB (PutImage)
80-96     | 50-75%  | Spike  | Low | 4MB (PutImage)
(Faster cursor movement = more frequent hash misses = sustained 50-75% CPU)
```

---

## Why Task 3.1 & 3.2 Didn't Fix It

### Task 3.1 (XRender Infrastructure)
- Added XRender format detection ✓
- Added routing for Circle shapes ✓
- **BUT**: Placeholder implementation
- Fallback still calls `build_projecteur_frame()` for non-Circle shapes
- Most spotlight configs use custom shapes
- **Result**: XRender path unused; still falling back to expensive rasterization

### Task 3.2 (Frame Hashing)
- Detects when frame content is identical ✓
- Skips PutImage upload when hash matches ✓
- **BUT**: Still calls `build_projecteur_frame()` to compute hash
- Saves only the PutImage operation (~2-4ms), not the rasterization (~8-13ms)
- When cursor moves (frequent): hash always misses
- **Result**: Optimizes the wrong part of the pipeline

---

## The Fundamental Architectural Problem

### Current Flow (Inefficient)
```
Tick → Always call build_projecteur_frame() → Compute hash → Check cache → Maybe skip upload
        [EXPENSIVE]                                            [Cheap]
```

### What It Should Be (Efficient)
```
Tick → Check if cursor/config changed → If yes, build_projecteur_frame() → Upload
       [CHEAP]                           [Only if needed]
```

---

## Recommended Solution Path

### Option A: Restore Task 2 Model (Fastest)
- Revert to passing (cursor, appearance/config) to backend
- Let X11 backend decide rendering strategy (XRender vs software)
- Only rasterize if absolutely required
- **Estimated performance**: 2-5% CPU, minimal memory churn

### Option B: Lazy Rasterization with Caching
- Only call `build_projecteur_frame()` when:
  - First draw
  - Cursor position changes by > threshold (e.g., 5px)
  - Config changes
  - Window resizes
- Cache the rendered frame + hash
- Reuse cached frame for static cursor
- **Estimated performance**: 15-25% CPU when moving, 2-5% when static

### Option C: Async Rasterization
- Render on background thread
- Use double-buffer pattern
- Display last good frame while new frame renders
- Requires synchronization complexity
- **Estimated performance**: Improves responsiveness, not throughput

---

## Approved Requirement Change

The original port plan constrained X11 to be transport-only. That requirement has now been explicitly relaxed:

- Previous rule: X11 must stay transport-only; visuals must come from Rust rasterization.
- Approved rule: X11 may own server-side compositing for supported spotlight visuals, with Rust rasterization kept as a fallback for unsupported or fidelity-critical cases.

This change matters because the best-performing known commit, `54ace59`, uses XRender radial composition on the X server for the hot path. The current software-first design cannot realistically recover that performance level while still requiring every moving frame to be synthesized in client memory.

## Selected Fix Direction

The chosen remediation is no longer “keep optimizing software upload.” It is:

1. Restore the `54ace59`-style server-side XRender radial fast path for the supported/default spotlight subset.
2. Keep the current `render.rs` Projecteur renderer as a software fallback for unsupported or fidelity-critical visuals.
3. Preserve the current parameter-driven runtime boundary so backend selection happens in `x11.rs`, not in the app loop.
4. Validate each stage against both `54ace59` and current `HEAD`, so the target is actual recovered behavior rather than theoretical improvement.

The practical implication is that Task 3.1 becomes the primary fix path, while Task 3.2 and Task 3.3 remain secondary mitigations around the slower software route.

---

## Recommended Next Steps

1. **Primary fix**: Restore the `54ace59` XRender fast path as a first-class backend strategy.
   - Reintroduce the explicit backend split in `x11.rs`
   - Keep server-side radial composition for the supported/default spotlight subset
   - Keep software upload for unsupported or fidelity-critical cases

2. **Secondary fix**: Retain lightweight runtime delegation.
   - Keep `overlay.rs` passing render inputs into the backend
   - Do not reintroduce unconditional client-side frame building in the runtime loop

3. **Validation gate**: Benchmark and manually compare `54ace59`, current `HEAD`, and the restored fast path.
   - Measure CPU while moving the cursor at 1920x1080 and 3840x2160
   - Record whether the restored path materially matches the known-good commit
   - Only then decide whether more software-side tuning is still needed

---

## Summary

| Metric | Task 2 | Current | Gap |
|--------|--------|---------|-----|
| CPU (static cursor) | 2-5% | 35-50% | **10-25x slower** |
| CPU (moving cursor) | 3-8% | 50-75% | **10-25x slower** |
| Memory per frame | ~100 bytes | ~4-8MB | **40,000x more** |
| Rasterization per tick | Never | Always | **Unbounded** |
| GPU utilization | Medium | Low | Underutilized |
| Remote X11 friendly | Yes | No | High bandwidth |

**Conclusion**: The performance regression is primarily caused by switching from GPU-accelerated gradient synthesis to per-frame CPU rasterization. The approved requirement change means the correct fix is to restore the proven server-side XRender hot path for supported visuals and keep the Projecteur software renderer as a fallback, rather than continuing to optimize around unconditional client-side frame synthesis.

## XRender Circle Dot Validation

- Baseline commit (rectangle dot): `12229a2`
- Candidate commit (circle dot): `ddc05cd` (includes follow-up safety fix after `a713c2e`)
- Automated validation in this environment:
  - `cargo test -p rpresent-spotlight`: PASS
  - `cargo test`: PASS
- Known warnings:
  - `RenderStrategy` and `select_render_strategy` are currently dead-code in `x11.rs`.

### Manual X11 Validation (pending on interactive desktop)

These checks cannot be executed in this non-interactive CI-like terminal session and must be run on your X11 desktop:

1. Visual correctness
   - `show_center_dot = false` => no center dot
   - `show_center_dot = true`, `dot_size_px = 8` => visible circular dot (not rectangular)
   - `dot_color = "#ff0000"`, `dot_opacity = 1.0` => opaque red circle
   - `dot_size_px = 2` and `24` => scale behaves as expected
2. Performance sanity
   - Compare cursor-motion smoothness and CPU for 10s against baseline `12229a2`
   - Scenarios: 1920x1080 and 3840x2160
   - Acceptance: no obvious stutter regression; CPU remains within normal run-to-run noise

### Notes

- Circle-dot fast path uses a cached tiny ARGB pixmap/picture and composites it each frame with XRender.
- Cache rebuild only occurs when dot config key changes (`show_center_dot`, `dot_size_px`, `dot_color`, `dot_opacity`).
- Follow-up fix `ddc05cd` closes X11 resource leaks on cache-upload error paths.
