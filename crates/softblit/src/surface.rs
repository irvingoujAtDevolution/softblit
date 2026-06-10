use crate::gpu::GpuState;
use crate::rect::Rect;
use crate::{Error, PixelFormat, PresentStats, ScalingMode, SurfaceDescriptor};

/// Where frames are presented.
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
    pub async fn new(target: SurfaceTarget, desc: SurfaceDescriptor) -> Result<Self, Error> {
        let target_size = match &target {
            SurfaceTarget::Canvas(c) => (c.width(), c.height()),
            SurfaceTarget::OffscreenCanvas(c) => (c.width(), c.height()),
        };

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let wgpu_target = match target {
            SurfaceTarget::Canvas(c) => wgpu::SurfaceTarget::Canvas(c),
            SurfaceTarget::OffscreenCanvas(c) => wgpu::SurfaceTarget::OffscreenCanvas(c),
        };
        let surface =
            instance
                .create_surface(wgpu_target)
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
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("softblit device"),
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

    /// Borrows the library-owned framebuffer for in-place decoding, allocating it on first use.
    ///
    /// The decoder contract: write pixels at their natural offsets
    /// (`stride == width * bytes_per_pixel`), `mark_dirty` each decoded rect, drop the guard,
    /// then call [`Surface::present`].
    pub fn frame_mut(&mut self) -> FrameMut<'_> {
        let (width, height) = self.gpu.source_size();
        let bpp = self.gpu.format().bytes_per_pixel();
        let expected = width as usize * height as usize * bpp;
        if self.framebuffer.len() != expected {
            self.framebuffer.clear();
            self.framebuffer.resize(expected, 0);
        }
        FrameMut {
            bytes: &mut self.framebuffer,
            dirty: &mut self.dirty,
            width,
            height,
            bpp,
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
        let expected = width as usize * height as usize * self.gpu.format().bytes_per_pixel();
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
    bpp: usize,
}

impl FrameMut<'_> {
    /// The whole framebuffer in the surface's pixel format, row-major,
    /// `stride == width * bytes_per_pixel` (tightly packed, no padding).
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        self.bytes
    }

    pub fn stride(&self) -> usize {
        self.width as usize * self.bpp
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
