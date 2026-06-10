// Final composition pass: draws the persistent source texture as a centered quad onto the
// swapchain, applying the scaling-mode transform. The swapchain is always fully redrawn
// (getCurrentTexture() contents are undefined across frames); letterbox bars come from the
// pass clear color.

struct BlitParams {
    // NDC half-extents of the destination quad.
    scale: vec2<f32>,
    // NDC center offset (currently always 0; reserved).
    offset: vec2<f32>,
    // 1 to force alpha to 1.0 (Rgbx8 / Bgrx8; packed formats already store alpha = 1).
    force_opaque: u32,
    _pad0: u32,
    _pad1: vec2<f32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: BlitParams;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // 4-vertex triangle strip: uv = (0,0) (1,0) (0,1) (1,1); v = 0 is the top row.
    let uv = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let ndc = vec2<f32>(
        (uv.x * 2.0 - 1.0) * params.scale.x + params.offset.x,
        (1.0 - uv.y * 2.0) * params.scale.y + params.offset.y,
    );
    return VsOut(vec4<f32>(ndc, 0.0, 1.0), uv);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var color = textureSample(source_tex, source_sampler, in.uv);
    if params.force_opaque != 0u {
        color.a = 1.0;
    }
    return color;
}
