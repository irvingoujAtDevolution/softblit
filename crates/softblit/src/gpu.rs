//! GPU plumbing: persistent source texture, dirty-rect uploads, compute unpack for packed
//! formats, and the final blit onto the swapchain.

use core::num::NonZeroU64;

use crate::rect::{self, Rect};
use crate::{Error, PixelFormat, PresentStats, ScalingMode, SurfaceDescriptor};

/// Blit uniform: scale vec2f + offset vec2f + force_opaque u32 + padding.
const BLIT_UNIFORM_SIZE: u64 = 32;
/// Unpack uniform: origin vec2u + size vec2u + src_width u32 + bgr u32 + padding.
const UNPACK_PARAMS_SIZE: u64 = 32;
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

    blit_pipeline: wgpu::RenderPipeline,
    blit_bgl: wgpu::BindGroupLayout,
    blit_uniform: wgpu::Buffer,
    blit_bind_group: wgpu::BindGroup,
    sampler_linear: wgpu::Sampler,
    sampler_nearest: wgpu::Sampler,

    unpack_pipeline: wgpu::ComputePipeline,
    unpack_bgl: wgpu::BindGroupLayout,
    unpack_uniform: wgpu::Buffer,

    source: SourceResources,
    source_view: wgpu::TextureView,

    /// The blit uniform must be rewritten before the next draw.
    params_dirty: bool,
    /// The swapchain must be redrawn even if no source bytes changed
    /// (initial frame, target resize, scaling change).
    needs_redraw: bool,
}

enum SourceResources {
    Direct {
        texture: wgpu::Texture,
    },
    Packed {
        // The texture is only ever accessed through its view (storage write in the unpack pass,
        // sampling in the blit); held here to make ownership explicit.
        _texture: wgpu::Texture,
        buffer: wgpu::Buffer,
        unpack_bind_group: wgpu::BindGroup,
    },
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

        let blit_shader = device.create_shader_module(wgpu::include_wgsl!("shaders/blit.wgsl"));
        let unpack_shader =
            device.create_shader_module(wgpu::include_wgsl!("shaders/unpack24.wgsl"));

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

        let unpack_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("softblit blit layout"),
            bind_group_layouts: &[Some(&blit_bgl)],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("softblit blit"),
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
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let unpack_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("softblit unpack layout"),
            bind_group_layouts: &[Some(&unpack_bgl)],
            immediate_size: 0,
        });
        let unpack_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("softblit unpack"),
            layout: Some(&unpack_layout),
            module: &unpack_shader,
            entry_point: Some("cs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let blit_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("softblit blit params"),
            size: BLIT_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let unpack_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("softblit unpack params"),
            size: UNPACK_UNIFORM_STRIDE * rect::MAX_RECTS as u64,
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
            &unpack_bgl,
            &unpack_uniform,
            desc.format,
            source_width,
            source_height,
        );
        let blit_bind_group = create_blit_bind_group(
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
            blit_pipeline,
            blit_bgl,
            blit_uniform,
            blit_bind_group,
            sampler_linear,
            sampler_nearest,
            unpack_pipeline,
            unpack_bgl,
            unpack_uniform,
            source,
            source_view,
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

    fn recreate_source(&mut self) {
        let (source, source_view) = create_source(
            &self.device,
            &self.unpack_bgl,
            &self.unpack_uniform,
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
        self.blit_bind_group = create_blit_bind_group(
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
    /// `bytes` is the full source framebuffer in the current format with tightly packed rows;
    /// it is only read when `dirty` is non-empty.
    pub(crate) fn present_inner(
        &mut self,
        bytes: &[u8],
        dirty: &[Rect],
    ) -> Result<PresentStats, Error> {
        let rects = rect::coalesce(dirty, self.source_width, self.source_height);

        if rects.is_empty() && !self.needs_redraw {
            return Ok(PresentStats {
                rects_uploaded: 0,
                bytes_uploaded: 0,
                skipped: true,
            });
        }

        let mut bytes_uploaded: u64 = 0;
        if !rects.is_empty() {
            let expected = self.source_width as usize
                * self.source_height as usize
                * self.format.bytes_per_pixel();
            if bytes.len() != expected {
                return Err(Error::BufferSizeMismatch {
                    expected,
                    actual: bytes.len(),
                });
            }
            bytes_uploaded = match &self.source {
                SourceResources::Direct { texture } => self.upload_direct(texture, bytes, &rects),
                SourceResources::Packed { buffer, .. } => self.upload_packed(buffer, bytes, &rects),
            };
        }

        let frame = self.acquire_frame()?;

        if self.params_dirty {
            self.write_blit_params();
            self.params_dirty = false;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("softblit present"),
            });

        if let SourceResources::Packed {
            unpack_bind_group, ..
        } = &self.source
            && !rects.is_empty()
        {
            self.write_unpack_params(&rects);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("softblit unpack"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.unpack_pipeline);
            for (i, r) in rects.iter().enumerate() {
                let offset = u32::try_from(i as u64 * UNPACK_UNIFORM_STRIDE)
                    .expect("MAX_RECTS * 256 fits u32");
                pass.set_bind_group(0, unpack_bind_group, &[offset]);
                pass.dispatch_workgroups(r.width.div_ceil(8), r.height.div_ceil(8), 1);
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
            bytes_uploaded,
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

    /// Direct formats: one `write_texture` per dirty rect, reading straight out of the caller's
    /// framebuffer with the framebuffer's natural stride. No extraction copy.
    fn upload_direct(&self, texture: &wgpu::Texture, bytes: &[u8], rects: &[Rect]) -> u64 {
        let bpp = self.format.bytes_per_pixel();
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

    /// Packed formats: upload the contiguous byte range spanning each dirty rect into the raw
    /// storage buffer (`write_buffer` requires 4-byte alignment, so ranges are widened to word
    /// boundaries; the extra bytes are current framebuffer content, so this is harmless).
    fn upload_packed(&self, buffer: &wgpu::Buffer, bytes: &[u8], rects: &[Rect]) -> u64 {
        let bpp = self.format.bytes_per_pixel();
        let stride = self.source_width as usize * bpp;
        let mut uploaded = 0u64;
        for r in rects {
            let start = r.y as usize * stride + r.x as usize * bpp;
            let end = (r.y + r.height - 1) as usize * stride + (r.x + r.width) as usize * bpp;
            let aligned_start = start & !3;
            let aligned_end = end.div_ceil(4) * 4;
            if aligned_end <= bytes.len() {
                self.queue.write_buffer(
                    buffer,
                    aligned_start as u64,
                    &bytes[aligned_start..aligned_end],
                );
            } else {
                // The rect reaches the framebuffer tail and the rounded range overhangs it; pad
                // into a small scratch (the pad bytes map past the last pixel, never read).
                let mut scratch = bytes[aligned_start..].to_vec();
                scratch.resize(aligned_end - aligned_start, 0);
                self.queue
                    .write_buffer(buffer, aligned_start as u64, &scratch);
            }
            uploaded += (aligned_end - aligned_start) as u64;
        }
        uploaded
    }

    fn write_blit_params(&self) {
        let (sx, sy) = self.scaling.ndc_scale(
            (self.source_width, self.source_height),
            (self.config.width, self.config.height),
        );
        let mut data = [0u8; BLIT_UNIFORM_SIZE as usize];
        data[0..4].copy_from_slice(&sx.to_le_bytes());
        data[4..8].copy_from_slice(&sy.to_le_bytes());
        // offset (8..16) stays 0: the quad is always centered.
        data[16..20].copy_from_slice(&u32::from(self.format.force_opaque()).to_le_bytes());
        self.queue.write_buffer(&self.blit_uniform, 0, &data);
    }

    fn write_unpack_params(&self, rects: &[Rect]) {
        let mut data = vec![0u8; rects.len() * UNPACK_UNIFORM_STRIDE as usize];
        for (i, r) in rects.iter().enumerate() {
            let base = i * UNPACK_UNIFORM_STRIDE as usize;
            data[base..base + 4].copy_from_slice(&r.x.to_le_bytes());
            data[base + 4..base + 8].copy_from_slice(&r.y.to_le_bytes());
            data[base + 8..base + 12].copy_from_slice(&r.width.to_le_bytes());
            data[base + 12..base + 16].copy_from_slice(&r.height.to_le_bytes());
            data[base + 16..base + 20].copy_from_slice(&self.source_width.to_le_bytes());
            data[base + 20..base + 24]
                .copy_from_slice(&u32::from(self.format.packed_is_bgr()).to_le_bytes());
        }
        self.queue.write_buffer(&self.unpack_uniform, 0, &data);
    }
}

fn create_source(
    device: &wgpu::Device,
    unpack_bgl: &wgpu::BindGroupLayout,
    unpack_uniform: &wgpu::Buffer,
    format: PixelFormat,
    width: u32,
    height: u32,
) -> (SourceResources, wgpu::TextureView) {
    let extent = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    if format.is_packed() {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("softblit source (packed)"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let byte_len = width as u64 * height as u64 * 3;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("softblit packed staging"),
            size: byte_len.div_ceil(4) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let unpack_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("softblit unpack bind group"),
            layout: unpack_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
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
        });
        (
            SourceResources::Packed {
                _texture: texture,
                buffer,
                unpack_bind_group,
            },
            view,
        )
    } else {
        let texture_format = match format {
            PixelFormat::Rgba8 | PixelFormat::Rgbx8 => wgpu::TextureFormat::Rgba8Unorm,
            PixelFormat::Bgra8 | PixelFormat::Bgrx8 => wgpu::TextureFormat::Bgra8Unorm,
            PixelFormat::Rgb24 | PixelFormat::Bgr24 => unreachable!("packed handled above"),
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("softblit source"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (SourceResources::Direct { texture }, view)
    }
}

fn create_blit_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    uniform: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("softblit blit bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
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
