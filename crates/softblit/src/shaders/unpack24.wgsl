// Unpacks framebuffer data with no GPU texture equivalent (RGB24/BGR24, RGB565/RGB555,
// Gray8/Gray16, planar I420) from a raw storage buffer into the persistent rgba8unorm texture,
// one dispatch per dirty rect. WGSL has no u8, so bytes are fetched from an array<u32> with
// shifts; the buffer holds source bytes verbatim (little-endian word packing).
//
// Two addressing modes share one shader:
// - in-place: the buffer mirrors the framebuffer layout; index_origin = (0,0), src_pitch = the
//   framebuffer width, src_base = 0 (I420: plane bases are absolute byte offsets).
// - gathered: the rect's rows were copied tightly into a scratch region; index_origin = the
//   rect origin, src_pitch = the rect width, src_base = the scratch byte offset.

struct UnpackParams {
    // Destination texel origin (absolute) and size of this rect.
    rect_origin: vec2<u32>,
    rect_size: vec2<u32>,
    // Subtracted from absolute coordinates before buffer indexing (see addressing modes).
    index_origin: vec2<u32>,
    // Byte base of interleaved data / the I420 Y plane.
    src_base: u32,
    // Row pitch in pixels.
    src_pitch: u32,
    // PixelFormat::shader_id().
    format: u32,
    // I420 only: absolute byte bases of the U/V planes and their row pitch in samples.
    u_base: u32,
    v_base: u32,
    chroma_pitch: u32,
}

@group(0) @binding(0) var<storage, read> src: array<u32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: UnpackParams;

fn load_byte(byte_offset: u32) -> u32 {
    return (src[byte_offset >> 2u] >> ((byte_offset & 3u) * 8u)) & 0xffu;
}

fn load_u16le(byte_offset: u32) -> u32 {
    return load_byte(byte_offset) | (load_byte(byte_offset + 1u) << 8u);
}

@compute @workgroup_size(8, 8)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.rect_size.x || gid.y >= params.rect_size.y {
        return;
    }
    let x = params.rect_origin.x + gid.x;
    let y = params.rect_origin.y + gid.y;
    let ix = x - params.index_origin.x;
    let iy = y - params.index_origin.y;

    var rgb = vec3<f32>(1.0, 0.0, 1.0); // magenta = unknown format (visible, never silent)
    switch params.format {
        case 0u, 1u: { // Rgb24 / Bgr24
            let base = params.src_base + (iy * params.src_pitch + ix) * 3u;
            let b0 = f32(load_byte(base)) / 255.0;
            let b1 = f32(load_byte(base + 1u)) / 255.0;
            let b2 = f32(load_byte(base + 2u)) / 255.0;
            rgb = select(vec3<f32>(b0, b1, b2), vec3<f32>(b2, b1, b0), params.format == 1u);
        }
        case 2u: { // Rgb565
            let v = load_u16le(params.src_base + (iy * params.src_pitch + ix) * 2u);
            rgb = vec3<f32>(
                f32((v >> 11u) & 0x1fu) / 31.0,
                f32((v >> 5u) & 0x3fu) / 63.0,
                f32(v & 0x1fu) / 31.0,
            );
        }
        case 3u: { // Rgb555
            let v = load_u16le(params.src_base + (iy * params.src_pitch + ix) * 2u);
            rgb = vec3<f32>(
                f32((v >> 10u) & 0x1fu) / 31.0,
                f32((v >> 5u) & 0x1fu) / 31.0,
                f32(v & 0x1fu) / 31.0,
            );
        }
        case 4u: { // Gray8
            let l = f32(load_byte(params.src_base + iy * params.src_pitch + ix)) / 255.0;
            rgb = vec3<f32>(l, l, l);
        }
        case 5u: { // Gray16 (little-endian)
            let l = f32(load_u16le(params.src_base + (iy * params.src_pitch + ix) * 2u)) / 65535.0;
            rgb = vec3<f32>(l, l, l);
        }
        case 6u: { // I420, BT.601 limited range
            let luma = f32(load_byte(params.src_base + iy * params.src_pitch + ix));
            let cb = f32(load_byte(params.u_base + (iy >> 1u) * params.chroma_pitch + (ix >> 1u)));
            let cr = f32(load_byte(params.v_base + (iy >> 1u) * params.chroma_pitch + (ix >> 1u)));
            let c = 1.163999 * (luma - 16.0);
            rgb = clamp(
                vec3<f32>(
                    c + 1.596 * (cr - 128.0),
                    c - 0.391 * (cb - 128.0) - 0.813 * (cr - 128.0),
                    c + 2.018 * (cb - 128.0),
                ) / 255.0,
                vec3<f32>(0.0),
                vec3<f32>(1.0),
            );
        }
        default: {}
    }

    textureStore(dst, vec2<i32>(i32(x), i32(y)), vec4<f32>(rgb, 1.0));
}
