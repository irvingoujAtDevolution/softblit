//! Headless verification of the whole producer pixel path with no GUI: create a shared texture,
//! import it into wgpu, render a known solid color into it, then read it back and check the pixels.
//! If this prints OK, the selected mechanism works on this machine.
//!
//! Backend via `SOFTBLIT_SPIKE_BACKEND=vulkan|dx12` (default vulkan).

use softblit_native::{
    D3DSharedSurface, D3D12SharedSurface, SharedSurface, VulkanSharedSurface, create_dx12_device,
    create_vulkan_device, create_vulkan_export_device,
};

const W: u32 = 64;
const H: u32 = 64;
const BYTES_PER_ROW: u32 = W * 4; // 256, already 256-aligned for copy_texture_to_buffer.

const SHADER: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[vi], 0.0, 1.0);
}
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // Straight red/green/blue = (1, 0, 1) -> stored BGRA as bytes [255, 0, 255, 255].
    return vec4<f32>(1.0, 0.0, 1.0, 1.0);
}
"#;

fn main() {
    let backend_sel = std::env::var("SOFTBLIT_SPIKE_BACKEND").unwrap_or_default();

    let (adapter, device, queue, surface): (
        wgpu::Adapter,
        wgpu::Device,
        wgpu::Queue,
        Box<dyn SharedSurface>,
    ) = if backend_sel.eq_ignore_ascii_case("dx12") {
        let (_instance, adapter, device, queue) =
            pollster::block_on(create_dx12_device()).expect("dx12 device");
        let surface = D3D12SharedSurface::new(device.clone(), queue.clone(), W, H)
            .expect("allocate + import D3D12 shared surface");
        (adapter, device, queue, Box::new(surface))
    } else if backend_sel.eq_ignore_ascii_case("d3d11") {
        let (_instance, adapter, device, queue) =
            pollster::block_on(create_vulkan_device()).expect("vulkan device with external memory");
        let surface = D3DSharedSurface::new(device.clone(), queue.clone(), W, H)
            .expect("allocate + import D3D11 shared surface");
        (adapter, device, queue, Box::new(surface))
    } else {
        let (_instance, adapter, device, queue) =
            pollster::block_on(create_vulkan_export_device()).expect("vulkan export device");
        let surface = VulkanSharedSurface::new(device.clone(), queue.clone(), W, H)
            .expect("allocate exportable Vulkan shared surface");
        (adapter, device, queue, Box::new(surface))
    };

    let sel = if backend_sel.is_empty() {
        "vulkan"
    } else {
        &backend_sel
    };
    let info = adapter.get_info();
    println!(
        "backend={} adapter: {} ({:?}, driver {})",
        sel, info.name, info.backend, info.driver
    );
    let h = surface.export_handle();
    println!(
        "shared handle: {:#x}  {}x{}  {:?}  {:?}",
        h.handle, h.width, h.height, h.format, h.sync
    );

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("probe"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("probe layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("probe pipeline"),
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

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe readback"),
        size: (BYTES_PER_ROW * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    surface.begin_producer();
    let view = surface
        .wgpu_texture()
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("probe") });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("probe pass"),
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
        pass.set_pipeline(&pipeline);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: surface.wgpu_texture(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(BYTES_PER_ROW),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    surface.end_producer();

    readback.slice(..).map_async(wgpu::MapMode::Read, |r| {
        r.expect("map readback buffer");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll for map");

    let data = readback.slice(..).get_mapped_range();
    let center = ((H / 2) * BYTES_PER_ROW + (W / 2) * 4) as usize;
    let px = [
        data[center],
        data[center + 1],
        data[center + 2],
        data[center + 3],
    ];
    println!("center pixel (BGRA bytes): {px:?}");

    let expected = [255u8, 0, 255, 255];
    assert_eq!(
        px, expected,
        "shared texture did not contain the rendered color"
    );
    println!("OK: wgpu rendered into the shared texture and readback matched.");
}
