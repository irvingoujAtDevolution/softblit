using System;
using System.Runtime.InteropServices;

namespace Spike;

/// Mirrors `softblit_spike::ShareInfo` (repr C).
[StructLayout(LayoutKind.Sequential)]
internal struct ShareInfo
{
    /// Keyed mutex / D3D12: shared handle. Vulkan: exported image memory NT handle.
    public IntPtr Handle;
    public uint Width;
    public uint Height;
    /// 0 = BGRA8Unorm.
    public uint Format;
    /// 0 = keyed mutex, 1 = D3D12 fence, 2 = Vulkan binary semaphores.
    public uint SyncKind;
    public ulong ConsumerAcquireKey;
    public ulong ConsumerReleaseKey;
    public IntPtr FenceHandle;
    /// Vulkan: exported image VkMemoryRequirements::size.
    public ulong MemorySize;
    /// Vulkan: NT handle of the "render finished" binary semaphore.
    public IntPtr RenderFinishedHandle;
    /// Vulkan: NT handle of the "image available" binary semaphore.
    public IntPtr ImageAvailableHandle;
}

internal static class NativeMethods
{
    private const string Lib = "softblit_spike";

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr spike_create(uint width, uint height);

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    public static extern void spike_get_share_info(IntPtr spike, out ShareInfo info);

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    public static extern void spike_render(IntPtr spike, float t);

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    public static extern ulong spike_fence_value(IntPtr spike);

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    public static extern void spike_resize(IntPtr spike, uint width, uint height, out ShareInfo info);

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)]
    public static extern void spike_destroy(IntPtr spike);
}
