//! Handle-leak regression check for the Vulkan shared surface: loop create -> resize xN -> drop and
//! assert the process's open kernel-handle count stays flat. This proves the exported OPAQUE_WIN32 NT
//! handles (image memory + the two semaphores) are actually closed. No GUI, no validation layer.
//!
//! Pre-fix, each cycle leaked ~6 handles (1 initial memory + 3 resize memories + 2 semaphores), so
//! 50 cycles grew the count by ~300; post-fix it stays near zero.

use softblit_native::{SharedSurface, VulkanSharedSurface, create_vulkan_export_device};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

fn handle_count() -> u32 {
    let process: HANDLE = unsafe { GetCurrentProcess() };
    let mut count = 0u32;
    // SAFETY: the current-process pseudo-handle is always valid; `count` is a live u32 out-param.
    unsafe {
        GetProcessHandleCount(process, &mut count).expect("GetProcessHandleCount");
    }
    count
}

fn cycle(device: &wgpu::Device, queue: &wgpu::Queue) {
    let mut surface =
        VulkanSharedSurface::new(device.clone(), queue.clone(), 64, 64).expect("create surface");
    surface.begin_producer();
    surface.end_producer();
    for (w, h) in [(128u32, 96u32), (200, 200), (64, 480)] {
        surface.resize(w, h);
        surface.begin_producer();
        surface.end_producer();
    }
}

fn main() {
    let (_instance, _adapter, device, queue) =
        pollster::block_on(create_vulkan_export_device()).expect("vulkan export device");

    // Warm up so first-use loader/driver handle allocations settle before the baseline.
    for _ in 0..5 {
        cycle(&device, &queue);
    }
    let baseline = handle_count();

    const ITERS: u32 = 50;
    for _ in 0..ITERS {
        cycle(&device, &queue);
    }
    let after = handle_count();

    let growth = after as i64 - baseline as i64;
    println!(
        "handle count: baseline={baseline} after={after} growth={growth} over {ITERS} cycles \
         (each = 1 create + 3 resizes + drop)"
    );

    assert!(
        growth < ITERS as i64,
        "handle count grew by {growth} over {ITERS} cycles (<1/cycle expected; pre-fix ~6/cycle) \
         — exported NT handles are leaking"
    );
    println!("OK: no per-cycle handle leak.");
}
