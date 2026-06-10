// Unpacks 3-byte-per-pixel framebuffer data (Rgb24 / Bgr24) from a raw storage buffer into the
// persistent rgba8unorm texture, one dispatch per dirty rect. WGSL has no u8, so bytes are
// fetched from an array<u32> with shifts; the buffer holds the framebuffer bytes verbatim
// (little-endian word packing, tightly packed rows: stride == width * 3).

struct UnpackParams {
    rect_origin: vec2<u32>,
    rect_size: vec2<u32>,
    // Source framebuffer width in pixels.
    src_width: u32,
    // 1 when the byte order is B G R.
    bgr: u32,
    _pad: vec2<u32>,
}

@group(0) @binding(0) var<storage, read> src: array<u32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: UnpackParams;

fn load_byte(byte_offset: u32) -> u32 {
    return (src[byte_offset >> 2u] >> ((byte_offset & 3u) * 8u)) & 0xffu;
}

@compute @workgroup_size(8, 8)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.rect_size.x || gid.y >= params.rect_size.y {
        return;
    }
    let x = params.rect_origin.x + gid.x;
    let y = params.rect_origin.y + gid.y;
    let base = (y * params.src_width + x) * 3u;
    let b0 = f32(load_byte(base)) / 255.0;
    let b1 = f32(load_byte(base + 1u)) / 255.0;
    let b2 = f32(load_byte(base + 2u)) / 255.0;
    var rgb = vec3<f32>(b0, b1, b2);
    if params.bgr != 0u {
        rgb = vec3<f32>(b2, b1, b0);
    }
    textureStore(dst, vec2<i32>(i32(x), i32(y)), vec4<f32>(rgb, 1.0));
}
