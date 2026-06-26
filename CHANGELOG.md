# Changelog

## 1.0.0-alpha.1 (2026-06-10)

Everything from the v0.2–v1.0 roadmap, in one pass. All features e2e-verified in a real
browser (WebGPU and WebGL2 projects) plus a native winit run.

### v0.2 — performance
- **Packed-path gather heuristic**: narrow tall rects are CPU-gathered tightly into the storage
  buffer's tail instead of uploading their full row span (only when the span exceeds 2× the
  tight size; the tail grows lazily). Demo workload upload dropped from ~1215 to ~95 KiB/frame.
- **A/B benchmark vs Canvas2D**: the demo's `?renderer=canvas2d` mode runs the identical
  workload through the tuned pack+`putImageData` path; both report CPU ms/frame.
  Measured (800x500, 12 moving rects, Chrome 149/Intel): GPU 0.53–0.87 ms vs Canvas2D
  0.57–1.11 ms CPU per frame, with ~55% less bytes uploaded.

### v0.3 — composition
- **Cursor/overlay layer**: `set_cursor` / `set_cursor_position` — a separate RGBA texture
  composited with straight-alpha blending in the blit pass; moving it is one uniform write and
  a re-blit (no framebuffer churn, no uploads).
- **External image ingestion**: `import_image_bitmap` (and `import_video_frame` under
  `--cfg web_sys_unstable_apis`, which wgpu requires for the direct `VideoFrame` source) via
  `copyExternalImageToTexture` into the persistent texture. Ordering is caller-sequenced and
  documented; persistent textures now carry `COPY_DST | RENDER_ATTACHMENT`.

### v0.4 — native + video formats
- **Native window target**: `Surface::new_windowed(raw-window-handle, size, desc)`; the GPU
  core is platform-neutral (host unit tests now cover gather/expand/convert logic).
  `examples/native_demo.rs` presents 300 frames through winit as a smoke test.
- **I420 planar YUV**: three-plane layout, BT.601 limited-range conversion in the unpack pass,
  dirty rects expanded to even coordinates, per-plane span uploads.

### v1.0 — coverage
- **New packed formats**: `Rgb565`, `Rgb555`, `Gray8`, `Gray16` (single generalized unpack
  shader with a format switch).
- **WebGL2 fallback** (`webgl` feature): WebGPU support is probed at instance creation
  (`new_instance_with_webgpu_detection` + web display handle); on compute-less adapters packed
  formats take a CPU-expand path into the same `rgba8unorm` texture. Downlevel device limits
  requested automatically.
- **API change**: `PixelFormat::bytes_per_pixel()` returns `Option<usize>` (planar I420 has no
  single bpp); use `PixelFormat::frame_len(w, h)` for buffer sizing.
- **DPI-aware presentation** demonstrated in the demo (`?hidpi=1`): CSS size stays, backing
  store and swapchain scale by `devicePixelRatio`.

## 0.1.0 (2026-06-09)

Initial implementation: direct formats (Rgba8/Bgra8/Rgbx8/Bgrx8), packed Rgb24/Bgr24 with
compute unpack, dirty-rect coalescing, five scaling modes, borrowed-slice `present_external` +
owned `frame_mut`, Canvas/OffscreenCanvas targets, worker rendering, `request_redraw`,
`PresentStats`. Verified live inside IronVNC (VNC session) and IronRDP (RDP session) web
clients behind Devolutions Gateway.
