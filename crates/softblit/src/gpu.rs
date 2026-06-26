//! GPU plumbing: persistent source texture, dirty-rect uploads, compute unpack for packed
//! formats (with a CPU-expand fallback for adapters without compute shaders), and the final
//! blit onto the swapchain.

use core::num::NonZeroU64;

use crate::format::PackedKind;
use crate::rect::{self, Rect};
use crate::{Error, PixelFormat, PresentStats, ScalingMode, SurfaceDescriptor};

/// Blit uniform: scale vec2f + offset vec2f + force_opaque u32 + padding.
const BLIT_UNIFORM_SIZE: u64 = 32;
/// Unpack uniform: see `UnpackParams` in `shaders/unpack24.wgsl`.
const UNPACK_PARAMS_SIZE: u64 = 48;
/// Per-rect stride inside the unpack uniform buffer; 256 satisfies every adapter's
/// `min_uniform_buffer_offset_alignment` (the spec caps it at 256).
const UNPACK_UNIFORM_STRIDE: u64 = 256;

pub(crate) struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    source_width: u32,
    source_height: u32,
    format: PixelFormat,
    scaling: ScalingMode,
    /// Adapters without compute shaders (WebGL2 downlevel) expand packed formats on the CPU.
    compute_available: bool,

    blit_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    blit_bgl: wgpu::BindGroupLayout,
    blit_uniform: wgpu::Buffer,
    blit_bind_group: wgpu::BindGroup,
    sampler_linear: wgpu::Sampler,
    sampler_nearest: wgpu::Sampler,

    unpack: Option<UnpackResources>,

    source: SourceResources,
    source_view: wgpu::TextureView,

    overlay: Option<Overlay>,
    overlay_uniform: wgpu::Buffer,

    /// Reused CPU scratch for gathered packed rects / CPU-expanded rects.
    scratch: Vec<u8>,

    /// The blit/overlay uniforms must be rewritten before the next draw.
    params_dirty: bool,
    /// The swapchain must be redrawn even if no source bytes changed
    /// (initial frame, target resize, scaling change, overlay change, VideoFrame import).
    needs_redraw: bool,
}

enum SourceResources {
    /// `write_texture` path: native texture format, or `rgba8unorm` fed by CPU-expanded packed
    /// data when the adapter lacks compute shaders.
    Direct {
        texture: wgpu::Texture,
        cpu_expand: bool,
    },
    /// Storage buffer + compute unpack path.
    Packed {
        // Accessed only through its view (storage write in the unpack pass, sampling in the
        // blit) — except by external-image import (wasm), which copies into it directly.
        #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
        texture: wgpu::Texture,
        buffer: wgpu::Buffer,
        unpack_bind_group: wgpu::BindGroup,
        /// Total buffer capacity; `frame_region..capacity` is the gather tail.
        capacity: u64,
    },
}

/// Compute-unpack machinery; absent on downlevel adapters without compute shaders.
struct UnpackResources {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
}

/// A cursor (or other small overlay) composited over the source in the blit pass.
struct Overlay {
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
}

/// One compute dispatch of the unpack pass.
struct UnpackDispatch {
    rect: Rect,
    params: [u8; UNPACK_PARAMS_SIZE as usize],
}

struct UploadResult {
    bytes_uploaded: u64,
    dispatches: Vec<UnpackDispatch>,
}

impl GpuState {
    pub(crate) fn new(
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_size: (u32, u32),
        desc: &SurfaceDescriptor,
    ) -> Self {
        let caps = surface.get_capabilities(adapter);
        // Pixels arrive already sRGB-encoded; a non-sRGB swapchain passes them through untouched.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes[0]
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: target_size.0.max(1),
            height: target_size.1.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let compute_available = adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);

        let blit_shader = device.create_shader_module(wgpu::include_wgsl!("shaders/blit.wgsl"));

        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("softblit blit bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(BLIT_UNIFORM_SIZE),
                    },
                    count: None,
                },
            ],
        });

        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("softblit blit layout"),
            bind_group_layouts: &[Some(&blit_bgl)],
            immediate_size: 0,
        });
        let make_pipeline = |label: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&blit_layout),
                vertex: wgpu::VertexState {
                    module: &blit_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..wgpu::PrimitiveState::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &blit_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let blit_pipeline = make_pipeline("softblit blit", None);
        // The overlay uses straight (non-premultiplied) alpha.
        let overlay_pipeline =
            make_pipeline("softblit overlay", Some(wgpu::BlendState::ALPHA_BLENDING));

        // Compute-unpack machinery only exists on compute-capable adapters: the bind group
        // layout itself is invalid on WebGL2 (zero compute storage buffers/textures).
        let unpack = compute_available.then(|| {
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("softblit unpack bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: true,
                            min_binding_size: NonZeroU64::new(UNPACK_PARAMS_SIZE),
                        },
                        count: None,
                    },
                ],
            });
            let unpack_shader =
                device.create_shader_module(wgpu::include_wgsl!("shaders/unpack24.wgsl"));
            let unpack_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("softblit unpack layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("softblit unpack"),
                layout: Some(&unpack_layout),
                module: &unpack_shader,
                entry_point: Some("cs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("softblit unpack params"),
                size: UNPACK_UNIFORM_STRIDE * rect::MAX_RECTS as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            UnpackResources {
                pipeline,
                bgl,
                uniform,
            }
        });

        let blit_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("softblit blit params"),
            size: BLIT_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let overlay_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("softblit overlay params"),
            size: BLIT_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler_linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("softblit linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let sampler_nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("softblit nearest"),
            ..wgpu::SamplerDescriptor::default()
        });

        let source_width = desc.source_size.0.max(1);
        let source_height = desc.source_size.1.max(1);
        let (source, source_view) = create_source(
            &device,
            unpack.as_ref(),
            desc.format,
            source_width,
            source_height,
        );
        let blit_bind_group = create_texture_bind_group(
            &device,
            &blit_bgl,
            &source_view,
            if desc.scaling.filter_linear() {
                &sampler_linear
            } else {
                &sampler_nearest
            },
            &blit_uniform,
        );

        Self {
            surface,
            device,
            queue,
            config,
            source_width,
            source_height,
            format: desc.format,
            scaling: desc.scaling,
            compute_available,
            blit_pipeline,
            overlay_pipeline,
            blit_bgl,
            blit_uniform,
            blit_bind_group,
            sampler_linear,
            sampler_nearest,
            unpack,
            source,
            source_view,
            overlay: None,
            overlay_uniform,
            scratch: Vec::new(),
            params_dirty: true,
            needs_redraw: true,
        }
    }

    pub(crate) fn source_size(&self) -> (u32, u32) {
        (self.source_width, self.source_height)
    }

    pub(crate) fn target_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub(crate) fn format(&self) -> PixelFormat {
        self.format
    }

    pub(crate) fn scaling(&self) -> ScalingMode {
        self.scaling
    }

    pub(crate) fn resize_source(&mut self, width: u32, height: u32) {
        self.source_width = width.max(1);
        self.source_height = height.max(1);
        self.recreate_source();
    }

    pub(crate) fn set_format(&mut self, format: PixelFormat) {
        if format == self.format {
            return;
        }
        self.format = format;
        self.recreate_source();
    }

    pub(crate) fn resize_target(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.params_dirty = true;
        self.needs_redraw = true;
    }

    pub(crate) fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    pub(crate) fn set_scaling(&mut self, scaling: ScalingMode) {
        if scaling == self.scaling {
            return;
        }
        let filter_changed = scaling.filter_linear() != self.scaling.filter_linear();
        self.scaling = scaling;
        if filter_changed {
            self.recreate_blit_bind_group();
        }
        self.params_dirty = true;
        self.needs_redraw = true;
    }

    /// Installs (or clears) the overlay image; RGBA8, straight alpha, tightly packed.
    pub(crate) fn set_overlay(&mut self, image: Option<(&[u8], u32, u32)>) {
        match image {
            None => self.overlay = None,
            Some((bytes, width, height)) => {
                let (x, y) = self.overlay.as_ref().map(|o| (o.x, o.y)).unwrap_or((0, 0));
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("softblit overlay"),
                    size: wgpu::Extent3d {
                        width: width.max(1),
                        height: height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * 4),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group = create_texture_bind_group(
                    &self.device,
                    &self.blit_bgl,
                    &view,
                    &self.sampler_linear,
                    &self.overlay_uniform,
                );
                self.overlay = Some(Overlay {
                    _texture: texture,
                    bind_group,
                    width,
                    height,
                    x,
                    y,
                });
            }
        }
        self.params_dirty = true;
        self.needs_redraw = true;
    }

    /// Moves the overlay; `(x, y)` is the overlay's top-left in source pixels (may be negative
    /// or partially off-screen).
    pub(crate) fn set_overlay_position(&mut self, x: i32, y: i32) {
        if let Some(overlay) = &mut self.overlay {
            if overlay.x == x && overlay.y == y {
                return;
            }
            overlay.x = x;
            overlay.y = y;
            self.params_dirty = true;
            self.needs_redraw = true;
        }
    }

    /// Copies an external image (e.g. a WebCodecs `VideoFrame`) into the persistent texture at
    /// `dst` and schedules a redraw. Ordering is caller-sequenced: copies and dirty-rect uploads
    /// are applied to the texture in submission order within a present cycle.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn import_external_image(
        &mut self,
        source: wgpu::ExternalImageSource,
        dst: (u32, u32),
        size: (u32, u32),
    ) -> Result<(), Error> {
        let texture = match &self.source {
            SourceResources::Direct { texture, .. } => texture,
            SourceResources::Packed { texture, .. } => texture,
        };
        let copy_width = size.0.min(self.source_width.saturating_sub(dst.0));
        let copy_height = size.1.min(self.source_height.saturating_sub(dst.1));
        if copy_width == 0 || copy_height == 0 {
            return Err(Error::InvalidRect {
                rect: Rect::new(dst.0, dst.1, size.0, size.1),
                bounds: (self.source_width, self.source_height),
            });
        }
        self.queue.copy_external_image_to_texture(
            &wgpu::CopyExternalImageSourceInfo {
                source,
                origin: wgpu::Origin2d::ZERO,
                flip_y: false,
            },
            wgpu::CopyExternalImageDestInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: dst.0,
                    y: dst.1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
                color_space: wgpu::PredefinedColorSpace::Srgb,
                premultiplied_alpha: false,
            },
            wgpu::Extent3d {
                width: copy_width,
                height: copy_height,
                depth_or_array_layers: 1,
            },
        );
        self.needs_redraw = true;
        Ok(())
    }

    fn recreate_source(&mut self) {
        let (source, source_view) = create_source(
            &self.device,
            self.unpack.as_ref(),
            self.format,
            self.source_width,
            self.source_height,
        );
        self.source = source;
        self.source_view = source_view;
        self.recreate_blit_bind_group();
        self.params_dirty = true;
        self.needs_redraw = true;
    }

    fn recreate_blit_bind_group(&mut self) {
        self.blit_bind_group = create_texture_bind_group(
            &self.device,
            &self.blit_bgl,
            &self.source_view,
            if self.scaling.filter_linear() {
                &self.sampler_linear
            } else {
                &self.sampler_nearest
            },
            &self.blit_uniform,
        );
    }

    /// Upload all dirty regions, unpack (packed formats), blit to the swapchain.
    ///
    /// `bytes` is the full source framebuffer in the current format with tightly packed rows
    /// (planes for I420); it is only read when `dirty` is non-empty.
    pub(crate) fn present_inner(
        &mut self,
        bytes: &[u8],
        dirty: &[Rect],
    ) -> Result<PresentStats, Error> {
        let mut rects = rect::coalesce(dirty, self.source_width, self.source_height);
        if self.format == PixelFormat::I420 {
            for r in &mut rects {
                *r = expand_even(*r, self.source_width, self.source_height);
            }
        }

        if rects.is_empty() && !self.needs_redraw {
            return Ok(PresentStats {
                rects_uploaded: 0,
                bytes_uploaded: 0,
                skipped: true,
            });
        }

        let mut upload = UploadResult {
            bytes_uploaded: 0,
            dispatches: Vec::new(),
        };
        if !rects.is_empty() {
            let expected = self.format.frame_len(self.source_width, self.source_height);
            if bytes.len() != expected {
                return Err(Error::BufferSizeMismatch {
                    expected,
                    actual: bytes.len(),
                });
            }
            upload = self.upload(bytes, &rects);
        }

        let frame = self.acquire_frame()?;

        if self.params_dirty {
            self.write_blit_params();
            self.write_overlay_params();
            self.params_dirty = false;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("softblit present"),
            });

        if !upload.dispatches.is_empty() {
            let mut data = vec![0u8; upload.dispatches.len() * UNPACK_UNIFORM_STRIDE as usize];
            for (i, dispatch) in upload.dispatches.iter().enumerate() {
                let base = i * UNPACK_UNIFORM_STRIDE as usize;
                data[base..base + UNPACK_PARAMS_SIZE as usize].copy_from_slice(&dispatch.params);
            }
            let (
                SourceResources::Packed {
                    unpack_bind_group, ..
                },
                Some(unpack),
            ) = (&self.source, &self.unpack)
            else {
                unreachable!("unpack dispatches are only planned for the GPU packed path");
            };
            self.queue.write_buffer(&unpack.uniform, 0, &data);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("softblit unpack"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&unpack.pipeline);
            for (i, dispatch) in upload.dispatches.iter().enumerate() {
                let offset = u32::try_from(i as u64 * UNPACK_UNIFORM_STRIDE)
                    .expect("MAX_RECTS * 256 fits u32");
                pass.set_bind_group(0, unpack_bind_group, &[offset]);
                pass.dispatch_workgroups(
                    dispatch.rect.width.div_ceil(8),
                    dispatch.rect.height.div_ceil(8),
                    1,
                );
            }
        }

        if let Some(frame) = &frame {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("softblit blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &self.blit_bind_group, &[]);
            pass.draw(0..4, 0..1);
            if let Some(overlay) = &self.overlay {
                pass.set_pipeline(&self.overlay_pipeline);
                pass.set_bind_group(0, &overlay.bind_group, &[]);
                pass.draw(0..4, 0..1);
            }
        }

        self.queue.submit([encoder.finish()]);
        match frame {
            Some(frame) => {
                frame.present();
                self.needs_redraw = false;
            }
            // Frame unavailable (occluded/timeout): the uploads and unpack still ran, so the
            // persistent texture is current; redo only the blit once a frame is available.
            None => self.needs_redraw = true,
        }

        Ok(PresentStats {
            rects_uploaded: u32::try_from(rects.len()).expect("rect count bounded by MAX_RECTS"),
            bytes_uploaded: upload.bytes_uploaded,
            skipped: false,
        })
    }

    /// `Ok(None)` means "skip presenting this call, retry later" (occluded window, timeout).
    fn acquire_frame(&mut self) -> Result<Option<wgpu::SurfaceTexture>, Error> {
        use wgpu::CurrentSurfaceTexture as Cst;
        match self.surface.get_current_texture() {
            Cst::Success(frame) | Cst::Suboptimal(frame) => Ok(Some(frame)),
            Cst::Lost | Cst::Outdated => {
                self.surface.configure(&self.device, &self.config);
                self.needs_redraw = true;
                match self.surface.get_current_texture() {
                    Cst::Success(frame) | Cst::Suboptimal(frame) => Ok(Some(frame)),
                    Cst::Timeout | Cst::Occluded => Ok(None),
                    Cst::Lost | Cst::Outdated | Cst::Validation => Err(Error::SurfaceLost),
                }
            }
            Cst::Timeout | Cst::Occluded => Ok(None),
            Cst::Validation => Err(Error::Device {
                reason: "validation error while acquiring the surface texture".to_owned(),
            }),
        }
    }

    fn upload(&mut self, bytes: &[u8], rects: &[Rect]) -> UploadResult {
        match &self.source {
            SourceResources::Direct {
                cpu_expand: false, ..
            } => UploadResult {
                bytes_uploaded: self.upload_direct(bytes, rects),
                dispatches: Vec::new(),
            },
            SourceResources::Direct {
                cpu_expand: true, ..
            } => UploadResult {
                bytes_uploaded: self.upload_cpu_expanded(bytes, rects),
                dispatches: Vec::new(),
            },
            SourceResources::Packed { .. } => self.upload_packed(bytes, rects),
        }
    }

    /// Direct formats: one `write_texture` per dirty rect, reading straight out of the caller's
    /// framebuffer with the framebuffer's natural stride. No extraction copy.
    fn upload_direct(&self, bytes: &[u8], rects: &[Rect]) -> u64 {
        let SourceResources::Direct { texture, .. } = &self.source else {
            unreachable!()
        };
        let bpp = self
            .format
            .bytes_per_pixel()
            .expect("direct formats are interleaved");
        let stride = self.source_width as usize * bpp;
        let mut uploaded = 0u64;
        for r in rects {
            let offset = r.y as usize * stride + r.x as usize * bpp;
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: r.x,
                        y: r.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes[offset..],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(u32::try_from(stride).expect("stride fits u32")),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: r.width,
                    height: r.height,
                    depth_or_array_layers: 1,
                },
            );
            uploaded += r.area() * bpp as u64;
        }
        uploaded
    }

    /// Downlevel fallback (no compute shaders): packed rects are expanded to RGBA on the CPU
    /// and written into the `rgba8unorm` texture directly.
    fn upload_cpu_expanded(&mut self, bytes: &[u8], rects: &[Rect]) -> u64 {
        let mut uploaded = 0u64;
        let mut scratch = core::mem::take(&mut self.scratch);
        for r in rects {
            expand_rect_to_rgba(
                bytes,
                self.format,
                self.source_width,
                self.source_height,
                *r,
                &mut scratch,
            );
            let SourceResources::Direct { texture, .. } = &self.source else {
                unreachable!()
            };
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: r.x,
                        y: r.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &scratch,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(r.width * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: r.width,
                    height: r.height,
                    depth_or_array_layers: 1,
                },
            );
            uploaded += r.area() * 4;
        }
        self.scratch = scratch;
        uploaded
    }

    /// Packed formats on a compute-capable adapter: upload raw bytes into the storage buffer
    /// (in-place row spans, or CPU-gathered tight rects in the buffer's tail when a narrow rect
    /// would otherwise upload far more than it covers) and plan one unpack dispatch per rect.
    fn upload_packed(&mut self, bytes: &[u8], rects: &[Rect]) -> UploadResult {
        let kind = self
            .format
            .packed_kind()
            .expect("upload_packed is only reached for packed formats");
        let w = self.source_width;
        let frame_region =
            pad4(self.format.frame_len(self.source_width, self.source_height) as u64);

        // Pass 1: pick a mode per rect and size the gather tail.
        let mut gather_total = 0u64;
        let gather_offsets: Vec<Option<u64>> = rects
            .iter()
            .map(|r| match kind {
                PackedKind::Planar420 => None,
                PackedKind::Interleaved {
                    bytes_per_pixel: bpp,
                } => {
                    if should_gather(r.width, r.height, bpp, w * bpp) {
                        let offset = gather_total;
                        gather_total +=
                            pad4(u64::from(r.width) * u64::from(r.height) * u64::from(bpp));
                        Some(offset)
                    } else {
                        None
                    }
                }
            })
            .collect();

        self.ensure_packed_capacity(frame_region + gather_total);
        let SourceResources::Packed { buffer, .. } = &self.source else {
            unreachable!()
        };

        let mut scratch = core::mem::take(&mut self.scratch);
        scratch.clear();
        scratch.resize(gather_total as usize, 0);

        let mut uploaded = 0u64;
        let mut dispatches = Vec::with_capacity(rects.len());
        for (r, gather_offset) in rects.iter().zip(&gather_offsets) {
            let mut params = ParamsWriter::new(*r, self.format.shader_id());
            match (kind, gather_offset) {
                (
                    PackedKind::Interleaved {
                        bytes_per_pixel: bpp,
                    },
                    None,
                ) => {
                    // In-place row span.
                    let stride = w as usize * bpp as usize;
                    let start = r.y as usize * stride + r.x as usize * bpp as usize;
                    let end = (r.y + r.height - 1) as usize * stride
                        + (r.x + r.width) as usize * bpp as usize;
                    uploaded += write_span(&self.queue, buffer, bytes, start, end);
                    params.indexing(0, 0, 0, w);
                }
                (
                    PackedKind::Interleaved {
                        bytes_per_pixel: bpp,
                    },
                    Some(offset),
                ) => {
                    // Gather tight rows into the scratch, upload once into the buffer tail.
                    let stride = w as usize * bpp as usize;
                    let row_bytes = r.width as usize * bpp as usize;
                    let tight = pad4((row_bytes * r.height as usize) as u64) as usize;
                    let dst = &mut scratch[*offset as usize..*offset as usize + tight];
                    for row in 0..r.height as usize {
                        let src_start = (r.y as usize + row) * stride + r.x as usize * bpp as usize;
                        dst[row * row_bytes..(row + 1) * row_bytes]
                            .copy_from_slice(&bytes[src_start..src_start + row_bytes]);
                    }
                    let buffer_offset = frame_region + offset;
                    self.queue.write_buffer(buffer, buffer_offset, dst);
                    uploaded += tight as u64;
                    params.indexing(
                        r.x,
                        r.y,
                        u32::try_from(buffer_offset).expect("packed buffer offsets fit u32"),
                        r.width,
                    );
                }
                (PackedKind::Planar420, _) => {
                    // Three in-place plane spans; rects are pre-expanded to even coordinates.
                    let (h, cw) = (self.source_height, self.source_width.div_ceil(2));
                    let y_len = (w as usize) * (h as usize);
                    let chroma_len = cw as usize * h.div_ceil(2) as usize;

                    let y_start = r.y as usize * w as usize + r.x as usize;
                    let y_end =
                        (r.y + r.height - 1) as usize * w as usize + (r.x + r.width) as usize;
                    uploaded += write_span(&self.queue, buffer, bytes, y_start, y_end);

                    let (cx0, cy0) = (r.x / 2, r.y / 2);
                    let cx1 = (r.x + r.width).div_ceil(2);
                    let cy1 = (r.y + r.height).div_ceil(2);
                    for plane_base in [y_len, y_len + chroma_len] {
                        let start = plane_base + cy0 as usize * cw as usize + cx0 as usize;
                        let end = plane_base + (cy1 - 1) as usize * cw as usize + cx1 as usize;
                        uploaded += write_span(&self.queue, buffer, bytes, start, end);
                    }

                    params.indexing(0, 0, 0, w);
                    params.planes(
                        u32::try_from(y_len).expect("plane offsets fit u32"),
                        u32::try_from(y_len + chroma_len).expect("plane offsets fit u32"),
                        cw,
                    );
                }
            }
            dispatches.push(UnpackDispatch {
                rect: *r,
                params: params.finish(),
            });
        }

        self.scratch = scratch;
        UploadResult {
            bytes_uploaded: uploaded,
            dispatches,
        }
    }

    /// Grows the packed storage buffer (frame region + gather tail) when needed, rebinding the
    /// unpack bind group. Buffer content is repopulated by uploads; stale regions are never read.
    fn ensure_packed_capacity(&mut self, needed: u64) {
        let SourceResources::Packed { capacity, .. } = &self.source else {
            return;
        };
        if *capacity >= needed {
            return;
        }
        let new_capacity = needed.max(*capacity + *capacity / 2);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("softblit packed staging"),
            size: new_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let unpack = self
            .unpack
            .as_ref()
            .expect("packed GPU sources only exist on compute-capable adapters");
        let unpack_bind_group = create_unpack_bind_group(
            &self.device,
            &unpack.bgl,
            &buffer,
            &self.source_view,
            &unpack.uniform,
        );
        if let SourceResources::Packed {
            buffer: b,
            unpack_bind_group: bg,
            capacity: c,
            ..
        } = &mut self.source
        {
            *b = buffer;
            *bg = unpack_bind_group;
            *c = new_capacity;
        }
    }

    fn write_blit_params(&self) {
        let (sx, sy) = self.scaling.ndc_scale(
            (self.source_width, self.source_height),
            (self.config.width, self.config.height),
        );
        let mut data = [0u8; BLIT_UNIFORM_SIZE as usize];
        data[0..4].copy_from_slice(&sx.to_le_bytes());
        data[4..8].copy_from_slice(&sy.to_le_bytes());
        // offset (8..16) stays 0: the source quad is always centered.
        data[16..20].copy_from_slice(&u32::from(self.format.force_opaque()).to_le_bytes());
        self.queue.write_buffer(&self.blit_uniform, 0, &data);
    }

    /// Maps the overlay rect (source pixels) through the same source→NDC transform as the main
    /// quad so the overlay lands exactly over the source pixels it covers.
    fn write_overlay_params(&self) {
        let Some(overlay) = &self.overlay else {
            return;
        };
        let (sx, sy) = self.scaling.ndc_scale(
            (self.source_width, self.source_height),
            (self.config.width, self.config.height),
        );
        let (sw, sh) = (self.source_width as f32, self.source_height as f32);
        // Source-space center and half-extent of the overlay quad.
        let cx = overlay.x as f32 + overlay.width as f32 / 2.0;
        let cy = overlay.y as f32 + overlay.height as f32 / 2.0;
        let scale_x = sx * overlay.width as f32 / sw;
        let scale_y = sy * overlay.height as f32 / sh;
        let offset_x = sx * (cx / sw * 2.0 - 1.0);
        let offset_y = -sy * (cy / sh * 2.0 - 1.0);

        let mut data = [0u8; BLIT_UNIFORM_SIZE as usize];
        data[0..4].copy_from_slice(&scale_x.to_le_bytes());
        data[4..8].copy_from_slice(&scale_y.to_le_bytes());
        data[8..12].copy_from_slice(&offset_x.to_le_bytes());
        data[12..16].copy_from_slice(&offset_y.to_le_bytes());
        // force_opaque = 0: the overlay's alpha channel is meaningful.
        self.queue.write_buffer(&self.overlay_uniform, 0, &data);
    }
}

/// Whether a narrow rect should be CPU-gathered instead of uploading its contiguous row span:
/// gather costs one extra CPU pass over the rect bytes, so it pays off once the span would
/// upload more than twice the tight size.
pub(crate) fn should_gather(
    width: u32,
    height: u32,
    bytes_per_pixel: u32,
    stride_bytes: u32,
) -> bool {
    if height <= 1 {
        return false;
    }
    let tight = u64::from(width) * u64::from(height) * u64::from(bytes_per_pixel);
    let span = u64::from(height - 1) * u64::from(stride_bytes)
        + u64::from(width) * u64::from(bytes_per_pixel);
    span > tight * 2
}

/// Expands a rect outward to even coordinates (I420 chroma siting), clamped to the source.
pub(crate) fn expand_even(r: Rect, bounds_width: u32, bounds_height: u32) -> Rect {
    let x0 = r.x & !1;
    let y0 = r.y & !1;
    let x1 = (r.x + r.width + 1) & !1;
    let y1 = (r.y + r.height + 1) & !1;
    Rect::new(
        x0,
        y0,
        x1.min(bounds_width) - x0,
        y1.min(bounds_height) - y0,
    )
}

fn pad4(v: u64) -> u64 {
    v.div_ceil(4) * 4
}

/// Uploads the 4-byte-aligned widening of `bytes[start..end]` at the same offset in `buffer`.
/// The widened bytes are current framebuffer content (harmless); a widening past the buffer
/// tail is zero-padded through a small copy.
fn write_span(
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    bytes: &[u8],
    start: usize,
    end: usize,
) -> u64 {
    let aligned_start = start & !3;
    let aligned_end = end.div_ceil(4) * 4;
    if aligned_end <= bytes.len() {
        queue.write_buffer(
            buffer,
            aligned_start as u64,
            &bytes[aligned_start..aligned_end],
        );
    } else {
        let mut padded = bytes[aligned_start..].to_vec();
        padded.resize(aligned_end - aligned_start, 0);
        queue.write_buffer(buffer, aligned_start as u64, &padded);
    }
    (aligned_end - aligned_start) as u64
}

/// Serializer for the `UnpackParams` uniform (see `shaders/unpack24.wgsl`).
struct ParamsWriter {
    data: [u8; UNPACK_PARAMS_SIZE as usize],
}

impl ParamsWriter {
    fn new(rect: Rect, shader_id: u32) -> Self {
        let mut writer = Self {
            data: [0; UNPACK_PARAMS_SIZE as usize],
        };
        writer.put(0, rect.x);
        writer.put(4, rect.y);
        writer.put(8, rect.width);
        writer.put(12, rect.height);
        writer.put(32, shader_id);
        writer
    }

    fn indexing(
        &mut self,
        index_origin_x: u32,
        index_origin_y: u32,
        src_base: u32,
        src_pitch: u32,
    ) {
        self.put(16, index_origin_x);
        self.put(20, index_origin_y);
        self.put(24, src_base);
        self.put(28, src_pitch);
    }

    fn planes(&mut self, u_base: u32, v_base: u32, chroma_pitch: u32) {
        self.put(36, u_base);
        self.put(40, v_base);
        self.put(44, chroma_pitch);
    }

    fn put(&mut self, offset: usize, value: u32) {
        self.data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn finish(self) -> [u8; UNPACK_PARAMS_SIZE as usize] {
        self.data
    }
}

/// CPU mirror of the unpack shader: expands `rect` of the source framebuffer into tightly
/// packed RGBA8 (alpha 255). Used on adapters without compute shaders.
pub(crate) fn expand_rect_to_rgba(
    bytes: &[u8],
    format: PixelFormat,
    src_width: u32,
    src_height: u32,
    rect: Rect,
    out: &mut Vec<u8>,
) {
    let w = src_width as usize;
    out.clear();
    out.resize(rect.width as usize * rect.height as usize * 4, 0xff);

    let mut write = |dx: usize, dy: usize, rgb: [u8; 3]| {
        let o = (dy * rect.width as usize + dx) * 4;
        out[o..o + 3].copy_from_slice(&rgb);
    };

    for dy in 0..rect.height as usize {
        let y = rect.y as usize + dy;
        for dx in 0..rect.width as usize {
            let x = rect.x as usize + dx;
            let rgb = match format {
                PixelFormat::Rgb24 => {
                    let b = (y * w + x) * 3;
                    [bytes[b], bytes[b + 1], bytes[b + 2]]
                }
                PixelFormat::Bgr24 => {
                    let b = (y * w + x) * 3;
                    [bytes[b + 2], bytes[b + 1], bytes[b]]
                }
                PixelFormat::Rgb565 => {
                    let b = (y * w + x) * 2;
                    let v = u16::from_le_bytes([bytes[b], bytes[b + 1]]);
                    [
                        (u32::from(v >> 11 & 0x1f) * 255 / 31) as u8,
                        (u32::from(v >> 5 & 0x3f) * 255 / 63) as u8,
                        (u32::from(v & 0x1f) * 255 / 31) as u8,
                    ]
                }
                PixelFormat::Rgb555 => {
                    let b = (y * w + x) * 2;
                    let v = u16::from_le_bytes([bytes[b], bytes[b + 1]]);
                    [
                        (u32::from(v >> 10 & 0x1f) * 255 / 31) as u8,
                        (u32::from(v >> 5 & 0x1f) * 255 / 31) as u8,
                        (u32::from(v & 0x1f) * 255 / 31) as u8,
                    ]
                }
                PixelFormat::Gray8 => {
                    let l = bytes[y * w + x];
                    [l, l, l]
                }
                PixelFormat::Gray16 => {
                    let b = (y * w + x) * 2;
                    let l = (u32::from(u16::from_le_bytes([bytes[b], bytes[b + 1]])) * 255 / 65535)
                        as u8;
                    [l, l, l]
                }
                PixelFormat::I420 => {
                    let cw = src_width.div_ceil(2) as usize;
                    let y_len = w * src_height as usize;
                    let chroma_len = cw * src_height.div_ceil(2) as usize;
                    let luma = f32::from(bytes[y * w + x]);
                    let cb = f32::from(bytes[y_len + (y / 2) * cw + x / 2]);
                    let cr = f32::from(bytes[y_len + chroma_len + (y / 2) * cw + x / 2]);
                    let c = 1.164 * (luma - 16.0);
                    let clamp = |v: f32| v.clamp(0.0, 255.0) as u8;
                    [
                        clamp(c + 1.596 * (cr - 128.0)),
                        clamp(c - 0.391 * (cb - 128.0) - 0.813 * (cr - 128.0)),
                        clamp(c + 2.018 * (cb - 128.0)),
                    ]
                }
                direct => {
                    unreachable!("expand_rect_to_rgba is never called for direct format {direct:?}")
                }
            };
            write(dx, dy, rgb);
        }
    }
}

fn create_source(
    device: &wgpu::Device,
    unpack: Option<&UnpackResources>,
    format: PixelFormat,
    width: u32,
    height: u32,
) -> (SourceResources, wgpu::TextureView) {
    let extent = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    // COPY_DST and RENDER_ATTACHMENT are required by `copyExternalImageToTexture`
    // (VideoFrame import).
    let import_usages = wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT;

    match (format.packed_kind(), unpack) {
        (Some(_), Some(unpack)) => {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("softblit source (packed)"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | import_usages,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let capacity = format.frame_len(width, height).div_ceil(4) as u64 * 4;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("softblit packed staging"),
                size: capacity,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let unpack_bind_group =
                create_unpack_bind_group(device, &unpack.bgl, &buffer, &view, &unpack.uniform);
            (
                SourceResources::Packed {
                    texture,
                    buffer,
                    unpack_bind_group,
                    capacity,
                },
                view,
            )
        }
        (packed, _) => {
            // Direct formats — or packed formats on a downlevel adapter (CPU expand).
            let cpu_expand = packed.is_some();
            let texture_format = if cpu_expand {
                wgpu::TextureFormat::Rgba8Unorm
            } else {
                match format {
                    PixelFormat::Rgba8 | PixelFormat::Rgbx8 => wgpu::TextureFormat::Rgba8Unorm,
                    PixelFormat::Bgra8 | PixelFormat::Bgrx8 => wgpu::TextureFormat::Bgra8Unorm,
                    other => unreachable!("{other:?} is packed"),
                }
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("softblit source"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: texture_format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | import_usages,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            (
                SourceResources::Direct {
                    texture,
                    cpu_expand,
                },
                view,
            )
        }
    }
}

fn create_unpack_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    storage_view: &wgpu::TextureView,
    unpack_uniform: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("softblit unpack bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(storage_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: unpack_uniform,
                    offset: 0,
                    size: NonZeroU64::new(UNPACK_PARAMS_SIZE),
                }),
            },
        ],
    })
}

fn create_texture_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    uniform: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("softblit texture bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gather_only_when_span_is_wasteful() {
        // Full-width rect: span == tight, never gather.
        assert!(!should_gather(800, 100, 3, 2400));
        // Narrow tall rect on a wide framebuffer: span >> tight.
        assert!(should_gather(60, 60, 3, 2400));
        // Single row: span == tight.
        assert!(!should_gather(60, 1, 3, 2400));
        // Half-width: span ≈ 2x tight, at the threshold — not worth the extra CPU pass.
        assert!(!should_gather(400, 100, 3, 2400));
    }

    #[test]
    fn expand_even_rounds_outward_and_clamps() {
        assert_eq!(
            expand_even(Rect::new(3, 5, 4, 4), 100, 100),
            Rect::new(2, 4, 6, 6)
        );
        assert_eq!(
            expand_even(Rect::new(0, 0, 100, 50), 100, 50),
            Rect::new(0, 0, 100, 50)
        );
        // Odd-sized source: clamp keeps the rect in bounds.
        assert_eq!(
            expand_even(Rect::new(4, 2, 1, 1), 5, 3),
            Rect::new(4, 2, 1, 1)
        );
    }

    #[test]
    fn cpu_expand_matches_formats() {
        let mut out = Vec::new();

        // Rgb565: pure red / pure green / pure blue.
        let px = |v: u16| v.to_le_bytes();
        let bytes: Vec<u8> = [px(0xf800), px(0x07e0), px(0x001f)].concat();
        expand_rect_to_rgba(
            &bytes,
            PixelFormat::Rgb565,
            3,
            1,
            Rect::new(0, 0, 3, 1),
            &mut out,
        );
        assert_eq!(&out[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out[4..8], &[0, 255, 0, 255]);
        assert_eq!(&out[8..12], &[0, 0, 255, 255]);

        // Gray8 broadcast.
        expand_rect_to_rgba(
            &[0x80, 0xff],
            PixelFormat::Gray8,
            2,
            1,
            Rect::new(0, 0, 2, 1),
            &mut out,
        );
        assert_eq!(&out[0..4], &[0x80, 0x80, 0x80, 255]);
        assert_eq!(&out[4..8], &[0xff, 0xff, 0xff, 255]);

        // I420 white (Y=235, U=V=128 in limited range).
        let bytes = [235, 235, 235, 235, 128, 128];
        expand_rect_to_rgba(
            &bytes,
            PixelFormat::I420,
            2,
            2,
            Rect::new(0, 0, 2, 2),
            &mut out,
        );
        assert_eq!(&out[0..4], &[254, 254, 254, 255]);
    }
}
