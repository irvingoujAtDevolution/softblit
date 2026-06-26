use crate::gpu::GpuState;
use crate::rect::Rect;
use crate::{Error, PixelFormat, PresentStats, ScalingMode, SurfaceDescriptor};

/// Where frames are presented (web). Native windows use [`Surface::new_windowed`].
#[cfg(target_arch = "wasm32")]
pub enum SurfaceTarget {
    /// A main-thread `<canvas>` element.
    Canvas(web_sys::HtmlCanvasElement),
    /// An `OffscreenCanvas`, typically transferred into a worker
    /// (`canvas.transferControlToOffscreen()`).
    OffscreenCanvas(web_sys::OffscreenCanvas),
}

/// A presentation surface: owns the persistent GPU texture, pipelines, and swapchain
/// configuration for one canvas, plus an optional library-owned framebuffer.
///
/// Two ingestion paths share the same persistent texture and upload machinery:
///
/// - **Borrowed**: the caller keeps its own framebuffer (the typical remote-desktop session
///   already does) and calls [`Surface::present_external`] with the bytes and the damaged rects.
/// - **Owned**: the caller decodes directly into the library-owned buffer borrowed via
///   [`Surface::frame_mut`], then calls [`Surface::present`].
///
/// `Surface` is not `Send`: it wraps JS objects and must live on the thread (main or worker)
/// that owns its canvas.
pub struct Surface {
    gpu: GpuState,
    framebuffer: Vec<u8>,
    dirty: Vec<Rect>,
}

impl Surface {
    /// Creates a surface presenting to `target`.
    ///
    /// The swapchain is sized to the canvas's current backing size; call
    /// [`Surface::resize_target`] when that changes (CSS resize, DPI change).
    #[cfg(target_arch = "wasm32")]
    pub async fn new(target: SurfaceTarget, desc: SurfaceDescriptor) -> Result<Self, Error> {
        let target_size = match &target {
            SurfaceTarget::Canvas(c) => (c.width(), c.height()),
            SurfaceTarget::OffscreenCanvas(c) => (c.width(), c.height()),
        };
        let wgpu_target = match target {
            SurfaceTarget::Canvas(c) => wgpu::SurfaceTarget::Canvas(c),
            SurfaceTarget::OffscreenCanvas(c) => wgpu::SurfaceTarget::OffscreenCanvas(c),
        };
        Self::from_wgpu_target(wgpu_target, target_size, desc).await
    }

    /// Creates a surface presenting to a native window (anything implementing
    /// `raw-window-handle`, e.g. a winit window: `window.into()`); `target_size` is the
    /// window's physical size in pixels.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new_windowed(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        target_size: (u32, u32),
        desc: SurfaceDescriptor,
    ) -> Result<Self, Error> {
        Self::from_wgpu_target(target.into(), target_size, desc).await
    }

    async fn from_wgpu_target(
        target: wgpu::SurfaceTarget<'static>,
        target_size: (u32, u32),
        desc: SurfaceDescriptor,
    ) -> Result<Self, Error> {
        #[cfg(target_arch = "wasm32")]
        let instance = {
            // The GL backend requires an instance-level display handle (the web display), and
            // WebGPU support must be probed before instance creation so wgpu can drop the
            // BROWSER_WEBGPU backend and fall back to WebGL when navigator.gpu is missing.
            #[derive(Debug)]
            struct WebDisplay;
            impl wgpu::rwh::HasDisplayHandle for WebDisplay {
                fn display_handle(
                    &self,
                ) -> Result<wgpu::rwh::DisplayHandle<'_>, wgpu::rwh::HandleError> {
                    Ok(wgpu::rwh::DisplayHandle::web())
                }
            }
            wgpu::util::new_instance_with_webgpu_detection(
                wgpu::InstanceDescriptor::new_without_display_handle()
                    .with_display_handle(Box::new(WebDisplay)),
            )
            .await
        };
        #[cfg(not(target_arch = "wasm32"))]
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(target)
            .map_err(|e| Error::WebGpuUnavailable {
                reason: e.to_string(),
            })?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::None,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .map_err(|e| Error::WebGpuUnavailable {
                reason: e.to_string(),
            })?;
        // Downlevel adapters (WebGL2) reject the default WebGPU limits (e.g. compute limits).
        let required_limits = if adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
        {
            wgpu::Limits::default()
        } else {
            wgpu::Limits::downlevel_webgl2_defaults()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("softblit device"),
                required_limits,
                ..wgpu::DeviceDescriptor::default()
            })
            .await
            .map_err(|e| Error::WebGpuUnavailable {
                reason: e.to_string(),
            })?;

        let gpu = GpuState::new(surface, &adapter, device, queue, target_size, &desc);
        Ok(Self {
            gpu,
            framebuffer: Vec::new(),
            dirty: Vec::new(),
        })
    }

    /// Source framebuffer size in pixels.
    pub fn source_size(&self) -> (u32, u32) {
        self.gpu.source_size()
    }

    /// Presentation (swapchain) size in physical pixels.
    pub fn target_size(&self) -> (u32, u32) {
        self.gpu.target_size()
    }

    pub fn format(&self) -> PixelFormat {
        self.gpu.format()
    }

    pub fn scaling(&self) -> ScalingMode {
        self.gpu.scaling()
    }

    /// Resizes the *remote framebuffer* (e.g. remote desktop resolution change).
    ///
    /// Reallocates the persistent texture (and the owned framebuffer, if in use, whose content
    /// resets to zero with full damage). Not cheap; call only when the source actually changes.
    pub fn resize_source(&mut self, width: u32, height: u32) {
        self.gpu.resize_source(width, height);
        self.reset_owned_framebuffer();
    }

    /// Resizes the *presentation surface* (canvas backing size / DPI change).
    /// Reconfigures the swapchain only; the source is untouched.
    pub fn resize_target(&mut self, width: u32, height: u32) {
        self.gpu.resize_target(width, height);
    }

    /// Changes the source pixel format (e.g. protocol renegotiation). Reallocates the
    /// framebuffer and switches upload paths; documented as a non-cheap operation.
    pub fn set_format(&mut self, format: PixelFormat) {
        if format == self.gpu.format() {
            return;
        }
        self.gpu.set_format(format);
        self.reset_owned_framebuffer();
    }

    pub fn set_scaling(&mut self, scaling: ScalingMode) {
        self.gpu.set_scaling(scaling);
    }

    /// Forces the next [`Surface::present`] to redraw the swapchain from the persistent texture
    /// even with no dirty regions (no upload happens). Use when the canvas needs repainting for
    /// reasons outside the source framebuffer — e.g. the compositor discarded the frame, or a
    /// consumer needs a freshly presented frame for `drawImage`/screenshot readback.
    pub fn request_redraw(&mut self) {
        self.gpu.request_redraw();
    }

    /// Installs, replaces, or clears the cursor/overlay image: RGBA8, straight alpha, tightly
    /// packed rows. The overlay is composited over the source in the blit pass; it never touches
    /// the framebuffer, so it causes no dirty-rect churn.
    pub fn set_cursor(&mut self, image: Option<(&[u8], u32, u32)>) {
        self.gpu.set_overlay(image);
    }

    /// Moves the overlay; `(x, y)` is its top-left corner in source pixels (may be negative or
    /// partially off-screen). Cheap: one uniform rewrite and a re-blit, no uploads.
    pub fn set_cursor_position(&mut self, x: i32, y: i32) {
        self.gpu.set_overlay_position(x, y);
    }

    /// Imports an [`web_sys::ImageBitmap`] into the persistent texture at `dst_origin`; pixel
    /// transfer and color conversion happen GPU-side. The covered region is presented on the
    /// next [`Surface::present`] (a redraw is scheduled).
    ///
    /// For WebCodecs output, `createImageBitmap(videoFrame)` produces a suitable bitmap without
    /// a CPU round-trip (or use [`Surface::import_video_frame`] when building with
    /// `--cfg web_sys_unstable_apis`, which wgpu requires for the direct `VideoFrame` source).
    ///
    /// # Ordering
    ///
    /// Imports and dirty-rect uploads are applied to the persistent texture in **call order**.
    /// The library does not reorder or timestamp them: a caller mixing WebCodecs output with raw
    /// rect updates must sequence the calls (e.g. drain decoder output before applying newer raw
    /// rects to the same region). The caller keeps ownership of the bitmap/frame and should
    /// `close()` it after import.
    #[cfg(target_arch = "wasm32")]
    pub fn import_image_bitmap(
        &mut self,
        bitmap: &web_sys::ImageBitmap,
        dst_origin: (u32, u32),
    ) -> Result<(), Error> {
        let size = (bitmap.width(), bitmap.height());
        self.gpu.import_external_image(
            wgpu::ExternalImageSource::ImageBitmap(bitmap.clone()),
            dst_origin,
            size,
        )
    }

    /// [`Surface::import_image_bitmap`] for a decoded [`web_sys::VideoFrame`] directly, without
    /// the intermediate bitmap. Available when building with `--cfg web_sys_unstable_apis`
    /// (required by wgpu's `VideoFrame` external-image source).
    #[cfg(all(target_arch = "wasm32", web_sys_unstable_apis))]
    pub fn import_video_frame(
        &mut self,
        frame: &web_sys::VideoFrame,
        dst_origin: (u32, u32),
    ) -> Result<(), Error> {
        let size = (frame.display_width(), frame.display_height());
        // `VideoFrame` has an inherent fallible `clone()` (the WebCodecs deep-copy) that shadows the
        // `Clone` trait; wgpu wants the cheap reference clone, so call the trait method explicitly.
        self.gpu.import_external_image(
            wgpu::ExternalImageSource::VideoFrame(Clone::clone(frame)),
            dst_origin,
            size,
        )
    }

    /// Borrows the library-owned framebuffer for in-place decoding, allocating it on first use.
    ///
    /// The decoder contract: write pixels at their natural offsets
    /// (`stride == width * bytes_per_pixel`; I420 is plane-ordered with luma stride `width`),
    /// `mark_dirty` each decoded rect, drop the guard, then call [`Surface::present`].
    pub fn frame_mut(&mut self) -> FrameMut<'_> {
        let (width, height) = self.gpu.source_size();
        let format = self.gpu.format();
        let expected = format.frame_len(width, height);
        if self.framebuffer.len() != expected {
            self.framebuffer.clear();
            self.framebuffer.resize(expected, 0);
        }
        FrameMut {
            bytes: &mut self.framebuffer,
            dirty: &mut self.dirty,
            width,
            height,
            row_bytes: width as usize * format.bytes_per_pixel().unwrap_or(1),
        }
    }

    /// Uploads all accumulated dirty regions from the owned framebuffer, unpacks (packed
    /// formats), and blits to the swapchain.
    ///
    /// With no dirty regions and no pending swapchain damage this is a no-op (cheap to call
    /// every rAF). On error, accumulated dirty state is kept so the next call retries.
    pub fn present(&mut self) -> Result<PresentStats, Error> {
        let stats = self.gpu.present_inner(&self.framebuffer, &self.dirty)?;
        self.dirty.clear();
        Ok(stats)
    }

    /// [`Surface::present`] for a caller-owned framebuffer: `bytes` is the full frame in the
    /// current format (tightly packed rows), `dirty` lists the damaged rects for this call.
    ///
    /// This is the integration path for sessions that already own their framebuffer
    /// (IronVNC's `Framebuffer`, IronRDP's `DecodedImage`): no restructuring, identical copy
    /// count. Returns [`Error::InvalidRect`] if a rect is out of bounds.
    pub fn present_external(
        &mut self,
        bytes: &[u8],
        dirty: &[Rect],
    ) -> Result<PresentStats, Error> {
        let (width, height) = self.gpu.source_size();
        for r in dirty {
            if !r.within(width, height) {
                return Err(Error::InvalidRect {
                    rect: *r,
                    bounds: (width, height),
                });
            }
        }
        self.gpu.present_inner(bytes, dirty)
    }

    fn reset_owned_framebuffer(&mut self) {
        self.dirty.clear();
        if self.framebuffer.is_empty() {
            return;
        }
        let (width, height) = self.gpu.source_size();
        let expected = self.gpu.format().frame_len(width, height);
        self.framebuffer.clear();
        self.framebuffer.resize(expected, 0);
        self.dirty.push(Rect::new(0, 0, width, height));
    }
}

/// Mutable borrow of the library-owned framebuffer plus its dirty tracker.
///
/// Under single buffering the borrow makes "decode while presenting" a compile error, which is
/// the honest expression of the constraint.
pub struct FrameMut<'a> {
    bytes: &'a mut [u8],
    dirty: &'a mut Vec<Rect>,
    width: u32,
    height: u32,
    row_bytes: usize,
}

impl FrameMut<'_> {
    /// The whole framebuffer in the surface's pixel format, row-major,
    /// `stride == width * bytes_per_pixel`, tightly packed, no padding (I420: Y, U, V planes in
    /// order, each tightly packed).
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        self.bytes
    }

    /// Row stride in bytes (for I420, the luma plane stride: `width`).
    pub fn stride(&self) -> usize {
        self.row_bytes
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Accumulates a damaged rect (clamped to the framebuffer bounds; coalescing happens at
    /// present time).
    pub fn mark_dirty(&mut self, rect: Rect) {
        if let Some(clipped) = rect.clipped(self.width, self.height) {
            self.dirty.push(clipped);
        }
    }

    pub fn mark_full_dirty(&mut self) {
        self.dirty.clear();
        self.dirty.push(Rect::new(0, 0, self.width, self.height));
    }
}
