# softblit — format-aware framebuffer presentation for WebGPU

Presents CPU-produced framebuffers (remote desktop clients, emulators, software renderers) to a
browser canvas with the minimum possible number of copies. Built against the real requirements of
IronVNC (RGB8 framebuffer → `Rgb24` packed path) and IronRDP (RGBA32 `DecodedImage` → `Rgba8`
direct path).

**Status:** 1.0.0-alpha.1 — the full roadmap is implemented and e2e-verified: WebGPU + WebGL2
(CPU-expand) backends, native window target (winit example), 11 pixel formats incl. planar
I420, cursor overlay, external-image (`ImageBitmap`/`VideoFrame`) ingestion, gather heuristic,
worker/OffscreenCanvas rendering, DPI-aware presentation, and an A/B benchmark against the
tuned Canvas2D path. See `CHANGELOG.md`.

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
| `Rgb565`, `Rgb555` | packed | 16-bit little-endian samples, shader unpack |
| `Gray8`, `Gray16` | packed | luminance broadcast in the shader |
| `I420` | planar | Y/U/V planes in one buffer, BT.601 conversion in the unpack pass; dirty rects round to even coords |

`Rgb24` is IronVNC's framebuffer layout (`ImgVec<RGB8>` is tightly packed, `stride == width*3`),
so IronVNC bytes upload as-is. Narrow tall rects are CPU-gathered tightly instead of uploading
their row span when the span would exceed 2× the tight size. On adapters without compute
shaders (WebGL2, `webgl` feature), packed formats are expanded to RGBA on the CPU and take the
direct path — same pixels, verified by the `webgl` e2e project.

## Composition

- **Cursor overlay**: `set_cursor(Some((rgba, w, h)))` / `set_cursor_position(x, y)` — a small
  separate texture alpha-blended in the blit pass; cursor moves cost one uniform write, zero
  uploads, zero framebuffer churn.
- **External images**: `import_image_bitmap(&bitmap, (x, y))` copies WebCodecs/canvas content
  GPU-side into the persistent texture (`import_video_frame` with `--cfg web_sys_unstable_apis`).
  Ordering is caller-sequenced: imports and uploads apply in call order.
- **Native**: `Surface::new_windowed(window, size, desc)` (raw-window-handle); see
  `cargo run --example native_demo`.

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

- `crates/softblit` — the library. Compiles for wasm32 (WebGPU, plus WebGL2 via the `webgl`
  feature) and native (`Surface::new_windowed`); logic core unit-tested on the host
  (`cargo test -p softblit`).
- `crates/softblit-demo` — browser demo: VNC-style animated dirty-rect workload, all eleven
  formats, all five scaling modes, cursor/import test hooks, live `PresentStats`.
  Query params: `?animate=0` (deterministic), `?renderer=canvas2d` (A/B benchmark),
  `?hidpi=1` (physical-resolution rendering); `www/worker.html` runs the same workload in a
  dedicated worker via OffscreenCanvas.
- `e2e` — Playwright suite (CI-ready), two projects: `webgpu` (8 tests: formats, cursor
  overlay, external-image import, geometry, worker, A/B bench, hidpi, upload rate) and `webgl`
  (CPU-expand fallback correctness with `navigator.gpu` removed).

## Build & test

```powershell
# lint + unit tests (host + wasm)
cargo clippy --workspace
cargo clippy --target wasm32-unknown-unknown --workspace
cargo test -p softblit

# native smoke test (opens a window for ~5s)
cargo run --example native_demo

# demo (webgl feature enables the GL fallback in the bundle; WebGPU is still preferred)
$env:RUSTFLAGS = "-Ctarget-feature=+simd128,+bulk-memory"
wasm-pack build crates/softblit-demo --target web --release -- --features webgl
python -m http.server 8917 --directory crates/softblit-demo   # open /www/index.html

# e2e (spawns its own server + Chrome; requires stable Chrome for headless WebGPU)
cd e2e && npm install && npx playwright test
```

Measured on this machine (headless Chrome 149, Intel Gen12), 800x500 source with 12 moving
rects: 60 fps, ~9 coalesced rects/frame, GPU 0.53–0.87 CPU-ms/frame & ~95 KiB/frame uploaded
vs Canvas2D 0.57–1.11 CPU-ms & ~213 KiB/frame.

## Future ideas (not currently planned)

- Native persistent-mapped staging ring (`Buffering::Double`) for true zero-copy decode.
- Worker-shaped Iron* clients: the renderer is worker-proven; moving the session decode loops
  into workers needs an `iron-remote-desktop` framework change (input/clipboard message
  protocol across the worker boundary).
- Gray16 windowing (level/width) for medical-style viewers.
