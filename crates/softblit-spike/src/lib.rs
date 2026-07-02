//! Phase-0 spike producer (throwaway). A C ABI over a shared surface that renders a moving
//! gradient with wgpu each frame. The Avalonia consumer P/Invokes these entry points, then
//! composites the shared texture via `ICompositionGpuInterop`.
//!
//! Producer and consumer must be in the same process: the D3D11 *global* shared handle
//! (`GetSharedHandle`) is only valid within its creating process, which is exactly the arrangement
//! Avalonia's own GpuInterop D3D11 demo uses. (The DX12 NT handle is process-portable, but we host
//! Rust in-process for both backends to keep one code path.)
//!
//! Three producer backends, selected by the `SOFTBLIT_SPIKE_BACKEND` env var:
//! - `vulkan` (default): pure Vulkan↔Vulkan. wgpu (Vulkan) renders into an exportable `VkImage`;
//!   Avalonia's Vulkan compositor imports the opaque-NT-handle memory + two binary semaphores
//!   (mechanism 3). The only on-screen path on Intel iGPUs, and the Linux-ready one.
//! - `d3d11`: wgpu Vulkan imports a D3D11 keyed-mutex texture (mechanism 1). Needs a Vulkan driver
//!   that supports `VK_KHR_external_memory_win32` D3D11 import (NVIDIA/AMD; Intel iGPU drivers crash).
//! - `dx12`: wgpu DX12 owns a shared D3D12 resource, synced by a shared fence (mechanism 2).

use softblit_native::{
    D3DSharedSurface, D3D12SharedSurface, SharedSurface, SyncKind, VulkanSharedSurface,
    create_dx12_device, create_vulkan_device, create_vulkan_export_device,
};

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct Params { t: f32, _p0: f32, _p1: f32, _p2: f32 };
@group(0) @binding(0) var<uniform> params: Params;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: VsOut;
    out.pos = vec4<f32>(p[vi], 0.0, 1.0);
    out.uv = p[vi] * 0.5 + vec2<f32>(0.5, 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = params.t;
    let r = 0.5 + 0.5 * sin(t + in.uv.x * 6.2831853);
    let g = 0.5 + 0.5 * sin(t * 1.3 + in.uv.y * 6.2831853);
    let b = 0.5 + 0.5 * sin(t * 0.7 + (in.uv.x + in.uv.y) * 3.1415926);
    return vec4<f32>(r, g, b, 1.0);
}
"#;

/// Descriptor the consumer needs to import the shared texture. Mirrors
/// `softblit_native::SharedHandle`, laid out C-compatibly for P/Invoke.
#[repr(C)]
pub struct ShareInfo {
    /// Keyed mutex / D3D12: shared texture/resource handle. Vulkan: exported image memory NT handle.
    pub handle: isize,
    pub width: u32,
    pub height: u32,
    /// 0 = BGRA8Unorm.
    pub format: u32,
    /// 0 = keyed mutex, 1 = D3D12 fence, 2 = Vulkan binary semaphores.
    pub sync_kind: u32,
    /// Keyed mutex: key the consumer acquires. Otherwise unused.
    pub consumer_acquire_key: u64,
    /// Keyed mutex: key the consumer releases. Otherwise unused.
    pub consumer_release_key: u64,
    /// D3D12: shared NT handle of the fence. Otherwise unused (0).
    pub fence_handle: isize,
    /// Vulkan: exported image `VkMemoryRequirements::size` (Avalonia's
    /// `PlatformGraphicsExternalImageProperties.MemorySize`). Otherwise unused (0).
    pub memory_size: u64,
    /// Vulkan: NT handle of the "render finished" binary semaphore (compositor waits on it).
    pub render_finished_handle: isize,
    /// Vulkan: NT handle of the "image available" binary semaphore (compositor signals it).
    pub image_available_handle: isize,
}

enum Backend {
    D3d11(D3DSharedSurface),
    Dx12(D3D12SharedSurface),
    Vulkan(Box<VulkanSharedSurface>),
}

impl Backend {
    fn as_shared(&self) -> &dyn SharedSurface {
        match self {
            Backend::D3d11(s) => s,
            Backend::Dx12(s) => s,
            Backend::Vulkan(s) => s.as_ref(),
        }
    }

    fn as_shared_mut(&mut self) -> &mut dyn SharedSurface {
        match self {
            Backend::D3d11(s) => s,
            Backend::Dx12(s) => s,
            Backend::Vulkan(s) => s.as_mut(),
        }
    }
}

pub struct Spike {
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    backend: Backend,
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Spike {
    fn new(width: u32, height: u32) -> Self {
        let backend_sel = std::env::var("SOFTBLIT_SPIKE_BACKEND").unwrap_or_default();

        let (instance, adapter, device, queue, backend) =
            if backend_sel.eq_ignore_ascii_case("dx12") {
                let (instance, adapter, device, queue) =
                    pollster::block_on(create_dx12_device()).expect("create a DX12 wgpu device");
                let surface = D3D12SharedSurface::new(device.clone(), queue.clone(), width, height)
                    .expect("allocate D3D12 shared surface");
                (instance, adapter, device, queue, Backend::Dx12(surface))
            } else if backend_sel.eq_ignore_ascii_case("d3d11") {
                let (instance, adapter, device, queue) = pollster::block_on(create_vulkan_device())
                    .expect("create a Vulkan wgpu device with VULKAN_EXTERNAL_MEMORY_WIN32");
                let surface = D3DSharedSurface::new(device.clone(), queue.clone(), width, height)
                    .expect("allocate D3D11 shared surface and import into wgpu");
                (instance, adapter, device, queue, Backend::D3d11(surface))
            } else {
                let (instance, adapter, device, queue) =
                    pollster::block_on(create_vulkan_export_device())
                        .expect("create a Vulkan wgpu export device");
                let surface = VulkanSharedSurface::new(device.clone(), queue.clone(), width, height)
                    .expect("allocate exportable Vulkan shared surface");
                (instance, adapter, device, queue, Backend::Vulkan(Box::new(surface)))
            };

        let (pipeline, uniform, bind_group) = build_pipeline(&device);

        Self {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
            backend,
            pipeline,
            uniform,
            bind_group,
        }
    }

    fn render(&self, t: f32) {
        let surface = self.backend.as_shared();

        self.queue.write_buffer(
            &self.uniform,
            0,
            &[t, 0.0, 0.0, 0.0].map(f32::to_le_bytes).concat(),
        );

        surface.begin_producer();

        let view = surface
            .wgpu_texture()
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("spike") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("spike gradient"),
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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);

        surface.end_producer();
    }

    fn fence_value(&self) -> u64 {
        match &self.backend {
            Backend::Dx12(s) => s.fence_value(),
            Backend::D3d11(_) | Backend::Vulkan(_) => 0,
        }
    }
}

fn build_pipeline(device: &wgpu::Device) -> (wgpu::RenderPipeline, wgpu::Buffer, wgpu::BindGroup) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("spike shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("spike bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("spike params"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("spike bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("spike layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("spike pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Bgra8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    (pipeline, uniform, bind_group)
}

fn fill_share_info(spike: &Spike, out: &mut ShareInfo) {
    let h = spike.backend.as_shared().export_handle();
    out.handle = h.handle;
    out.width = h.width;
    out.height = h.height;
    out.format = 0;
    out.consumer_acquire_key = 0;
    out.consumer_release_key = 0;
    out.fence_handle = 0;
    out.memory_size = 0;
    out.render_finished_handle = 0;
    out.image_available_handle = 0;
    match h.sync {
        SyncKind::KeyedMutex {
            consumer_acquire_key,
            consumer_release_key,
        } => {
            out.sync_kind = 0;
            out.consumer_acquire_key = consumer_acquire_key;
            out.consumer_release_key = consumer_release_key;
        }
        SyncKind::D3D12Fence { fence_handle } => {
            out.sync_kind = 1;
            out.fence_handle = fence_handle;
        }
        SyncKind::VulkanSemaphore {
            memory_size,
            render_finished_handle,
            image_available_handle,
        } => {
            out.sync_kind = 2;
            out.memory_size = memory_size;
            out.render_finished_handle = render_finished_handle;
            out.image_available_handle = image_available_handle;
        }
    }
}

/// Creates the spike producer and its shared surface. Returns an opaque pointer; free with
/// [`spike_destroy`].
///
/// # Safety
/// The returned pointer must be passed only to the other `spike_*` functions and freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spike_create(width: u32, height: u32) -> *mut Spike {
    static INIT_TRACING: std::sync::Once = std::sync::Once::new();
    INIT_TRACING.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_env("SOFTBLIT_LOG")
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
    });
    Box::into_raw(Box::new(Spike::new(width.max(1), height.max(1))))
}

/// Writes the shared-texture import descriptor into `out`.
///
/// # Safety
/// `spike` must be a live pointer from [`spike_create`]; `out` must point to a writable `ShareInfo`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spike_get_share_info(spike: *mut Spike, out: *mut ShareInfo) {
    let spike = unsafe { &*spike };
    let out = unsafe { &mut *out };
    fill_share_info(spike, out);
}

/// Renders one gradient frame at time `t` (seconds) into the shared texture and hands it off.
///
/// # Safety
/// `spike` must be a live pointer from [`spike_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spike_render(spike: *mut Spike, t: f32) {
    let spike = unsafe { &*spike };
    spike.render(t);
}

/// The fence value published after the most recent [`spike_render`] (DX12 backend). Returns 0 for
/// the keyed-mutex backend.
///
/// # Safety
/// `spike` must be a live pointer from [`spike_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spike_fence_value(spike: *mut Spike) -> u64 {
    let spike = unsafe { &*spike };
    spike.fence_value()
}

/// Resizes the shared surface, invalidating the previous handle, and writes the new descriptor
/// into `out`.
///
/// # Safety
/// `spike` must be a live pointer from [`spike_create`]; `out` must point to a writable `ShareInfo`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spike_resize(
    spike: *mut Spike,
    width: u32,
    height: u32,
    out: *mut ShareInfo,
) {
    let spike = unsafe { &mut *spike };
    spike.backend.as_shared_mut().resize(width.max(1), height.max(1));
    let out = unsafe { &mut *out };
    fill_share_info(spike, out);
}

/// Frees the spike producer.
///
/// # Safety
/// `spike` must be a live pointer from [`spike_create`] and must not be used afterward.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spike_destroy(spike: *mut Spike) {
    if !spike.is_null() {
        drop(unsafe { Box::from_raw(spike) });
    }
}
