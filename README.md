# softblit — format-aware framebuffer presentation for WebGPU

Presents CPU-produced framebuffers (remote desktop clients, emulators, software renderers) to a
browser canvas with the minimum possible number of copies. Built against the real requirements of
IronVNC (RGB8 framebuffer → `Rgb24` packed path) and IronRDP (RGBA32 `DecodedImage` → `Rgba8`
direct path).

**Status:** v0.1 — implemented and e2e-verified (wasm32 / WebGPU). Native targets, `VideoFrame`
ingestion, and WebGL2 fallback are not yet implemented.

## Copy accounting (normative)

| Step | CPU copies | Notes |
|---|---|---|
| Decoder → framebuffer | 0 | decode in place |
| Framebuffer → GPU staging (`write_texture` / `write_buffer`) | 1 | platform floor; wasm memory cannot be GPU-mapped |
| GPU staging → texture → swapchain | 0 | GPU-internal |
| **Total CPU copies** | **1 per dirty byte** | verified via `PresentStats` |

Compared to the Canvas2D baseline this removes the RGB→RGBA pack pass (IronVNC `canvas.rs`) and
the per-rect extraction copy (IronRDP `extract_partial_image`); `putImageData`'s own copy becomes
the single `write_texture` copy.

## Pixel formats

| Format | Path | Mechanism |
|---|---|---|
| `Rgba8`, `Bgra8` | direct | persistent `rgba8unorm`/`bgra8unorm` texture, per-rect `write_texture` |
| `Rgbx8`, `Bgrx8` | direct | as above; alpha forced to 1 in the blit shader |
| `Rgb24`, `Bgr24` | packed | raw bytes in a storage buffer, compute-pass unpack into `rgba8unorm` — no CPU repack, 25% less upload than RGBA |

`Rgb24` is IronVNC's framebuffer layout (`ImgVec<RGB8>` is tightly packed, `stride == width*3`),
so IronVNC bytes upload as-is.

## Two ingestion paths, one persistent texture

```rust
// Borrowed (recommended for existing sessions that own their framebuffer — IronVNC/IronRDP):
let mut surface = Surface::new(
    SurfaceTarget::Canvas(canvas),
    SurfaceDescriptor { source_size: (w, h), format: PixelFormat::Rgb24, scaling: ScalingMode::Fit },
).await?;
// on each FramebufferUpdated / GraphicsUpdate:
surface.present_external(framebuffer_bytes, &[Rect::new(x, y, rw, rh)])?;

// Owned (library-owned buffer, decode in place):
let mut frame = surface.frame_mut();
decode_into(frame.bytes_mut(), frame.stride());
frame.mark_dirty(rect);
drop(frame);
let stats = surface.present()?; // PresentStats { rects_uploaded, bytes_uploaded, skipped }
```

Other API: `resize_source` (remote resolution change — reallocates), `resize_target` (canvas/DPI —
swapchain reconfigure only), `set_format` (runtime renegotiation — reallocates), `set_scaling`
(`Fit`/`Fill`/`Stretch`/`Integer`/`Native1x`), `request_redraw` (re-blit without upload).

Dirty rects are clipped, merged to fixpoint, and collapsed to their bounding box when dense
(> 0.8 coverage) or numerous (> 64): each upload has fixed JS-boundary overhead on wasm.

## Known tradeoff: packed-path upload width

Packed-format rect uploads transfer the contiguous byte span covering the rect's rows (one
`write_buffer`, includes the row remainder outside narrow rects) rather than gathering rows on
the CPU. For full-width updates this is exact; for narrow tall rects it uploads
`stride/(rect_width*3)`× extra bytes. `PresentStats::bytes_uploaded` reports the real number.
A per-rect gather heuristic is a candidate v0.2 optimization — decide by benchmark.

## Layout

- `crates/softblit` — the library. GPU surface is wasm32-only for now; rect/format/scaling core is
  platform-independent and unit-tested on the host (`cargo test -p softblit`).
- `crates/softblit-demo` — browser demo: VNC-style animated dirty-rect workload, all six formats,
  all five scaling modes, live `PresentStats`. `?animate=0` renders the deterministic background
  only (used by e2e tests).
- `e2e` — Playwright suite (CI-ready): cross-format pixel correctness against computed expected
  values, `Native1x` letterbox geometry, animated upload-rate smoke test.

## Build & test

```powershell
# lint + unit tests
cargo clippy --target wasm32-unknown-unknown --workspace
cargo test -p softblit

# demo
$env:RUSTFLAGS = "-Ctarget-feature=+simd128,+bulk-memory"
wasm-pack build crates/softblit-demo --target web --release
python -m http.server 8917 --directory crates/softblit-demo   # open /www/index.html

# e2e (spawns its own server + Chrome; requires stable Chrome for headless WebGPU)
cd e2e && npm install && npx playwright test
```

Measured on this machine (headless Chrome 149, Intel Gen12): 60 fps, ~10 coalesced rects/frame.

## Roadmap (from the design doc)

- v0.2: packed-path gather heuristic, benchmark harness vs IronVNC's tuned Canvas2D path.
- v0.3: `VideoFrame` ingestion (`copyExternalImageToTexture`) with a defined ordering model;
  cursor overlay layer.
- v0.4: native targets via wgpu (the GPU core is already platform-agnostic), mapped-staging
  double buffering.
- v1.0: WebGL2 fallback, Gray8/16, API freeze.
