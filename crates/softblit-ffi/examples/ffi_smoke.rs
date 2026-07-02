//! In-process smoke test for the diplomat bridge: drives `create -> share_info -> present`
//! through the exact `ffi::SoftblitSurface` API (the same methods diplomat wraps in extern "C"),
//! proving the bridge actually runs softblit's engine into a real shared GPU texture.
//!
//! Run: `CARGO_TARGET_DIR=D:/Dev/build/cargo-target-ffi cargo run -p softblit-ffi --example ffi_smoke`

use softblit_ffi::ffi::{BackendFfi, PixelFormatFfi, ScalingModeFfi, SoftblitSurface, SyncKindFfi};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("SOFTBLIT_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    const W: u32 = 64;
    const H: u32 = 64;

    let mut surface = SoftblitSurface::create(
        W,
        H,
        PixelFormatFfi::Bgra8,
        ScalingModeFfi::Stretch,
        BackendFfi::Vulkan,
    )
    .expect("create SoftblitSurface (Vulkan export backend)");

    let info = surface.share_info();
    println!(
        "share_info: handle={:#x} {}x{} format={} sync_kind={:?} memory_size={} rf={:#x} ia={:#x}",
        info.handle,
        info.width,
        info.height,
        info.format,
        info.sync_kind,
        info.memory_size,
        info.render_finished_handle,
        info.image_available_handle,
    );
    assert_eq!(info.width, W);
    assert_eq!(info.height, H);
    assert_ne!(info.handle, 0, "shared handle must be non-null");
    if info.sync_kind == SyncKindFfi::VulkanSemaphore {
        assert!(info.memory_size > 0, "vulkan memory_size must be set");
        assert_ne!(info.render_finished_handle, 0);
        assert_ne!(info.image_available_handle, 0);
    }

    // One full-frame BGRA8 present: solid opaque blue.
    let mut frame = vec![0u8; (W * H * 4) as usize];
    for px in frame.chunks_exact_mut(4) {
        px[0] = 200; // B
        px[1] = 40; // G
        px[2] = 10; // R
        px[3] = 255; // A
    }
    let dirty = [0u32, 0, W, H];

    let stats = surface.present(&frame, &dirty).expect("present full frame");
    println!(
        "present: rects_uploaded={} bytes_uploaded={} skipped={}",
        stats.rects_uploaded, stats.bytes_uploaded, stats.skipped
    );
    assert!(!stats.skipped, "a full-frame present is not a no-op");
    assert_eq!(stats.rects_uploaded, 1, "one coalesced dirty rect");
    assert_eq!(
        stats.bytes_uploaded,
        u64::from(W * H * 4),
        "exactly the full frame's bytes crossed to the GPU"
    );

    println!("ffi_smoke: OK");
}
