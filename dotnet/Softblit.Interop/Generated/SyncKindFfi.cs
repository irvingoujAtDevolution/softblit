namespace Softblit.Interop;

public enum SyncKindFfi : int
{
    KeyedMutex = 0,
    D3d12Fence = 1,
    VulkanSemaphore = 2,
}