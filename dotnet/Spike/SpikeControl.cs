using System;
using System.Diagnostics;
using System.Threading.Tasks;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Platform;
using Avalonia.Rendering.Composition;
using Avalonia.Threading;
using Avalonia.VisualTree;

namespace Spike;

/// Hosts the Rust wgpu producer and composites its shared texture through Avalonia's
/// `ICompositionGpuInterop`. Modeled on Avalonia's `samples/GpuInterop/DrawingSurfaceDemoBase`.
internal sealed class SpikeControl : Control
{
    private Compositor? _compositor;
    private CompositionDrawingSurface? _surface;
    private CompositionSurfaceVisual? _visual;
    private ICompositionGpuInterop? _interop;

    private IntPtr _spike;
    private ShareInfo _info;
    private ICompositionImportedGpuImage? _image;
    private ICompositionImportedGpuSemaphore? _semaphore;
    private ICompositionImportedGpuSemaphore? _renderFinished;
    private ICompositionImportedGpuSemaphore? _imageAvailable;

    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private DispatcherTimer? _timer;
    private PixelSize _lastSize;
    private bool _busy;
    private bool _initialized;

    protected override void OnAttachedToVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnAttachedToVisualTree(e);
        _ = InitializeAsync();
    }

    protected override void OnDetachedFromVisualTree(VisualTreeAttachmentEventArgs e)
    {
        _timer?.Stop();
        _timer = null;
        _ = _image?.DisposeAsync();
        _ = _semaphore?.DisposeAsync();
        _ = _renderFinished?.DisposeAsync();
        _ = _imageAvailable?.DisposeAsync();
        _surface?.Dispose();
        if (_spike != IntPtr.Zero)
        {
            NativeMethods.spike_destroy(_spike);
            _spike = IntPtr.Zero;
        }
        _initialized = false;
        base.OnDetachedFromVisualTree(e);
    }

    private async Task InitializeAsync()
    {
        try
        {
            var self = ElementComposition.GetElementVisual(this)!;
            _compositor = self.Compositor;
            _surface = _compositor.CreateDrawingSurface();
            _visual = _compositor.CreateSurfaceVisual();
            _visual.Size = new Vector(Bounds.Width, Bounds.Height);
            _visual.Surface = _surface;
            ElementComposition.SetElementChildVisual(this, _visual);

            _interop = await _compositor.TryGetCompositionGpuInterop();
            if (_interop == null)
            {
                Console.WriteLine("[spike] compositor has no GPU interop for this backend");
                return;
            }

            Console.WriteLine("[spike] SupportedImageHandleTypes: " +
                              string.Join(", ", _interop.SupportedImageHandleTypes));
            Console.WriteLine("[spike] SupportedSemaphoreTypes: " +
                              string.Join(", ", _interop.SupportedSemaphoreTypes));

            var size = CurrentPixelSize();
            _spike = NativeMethods.spike_create((uint)size.Width, (uint)size.Height);
            NativeMethods.spike_get_share_info(_spike, out _info);
            Console.WriteLine($"[spike] producer handle=0x{_info.Handle:x} {_info.Width}x{_info.Height} " +
                              $"syncKind={_info.SyncKind} fence=0x{_info.FenceHandle:x}");
            _lastSize = new PixelSize((int)_info.Width, (int)_info.Height);

            ImportResources();

            _initialized = true;
            _timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(16) };
            _timer.Tick += (_, _) => Tick();
            _timer.Start();
        }
        catch (Exception ex)
        {
            Console.WriteLine("[spike] init failed: " + ex);
        }
    }

    private void ImportResources()
    {
        _ = _image?.DisposeAsync();
        _ = _semaphore?.DisposeAsync();
        _ = _renderFinished?.DisposeAsync();
        _ = _imageAvailable?.DisposeAsync();
        _image = null;
        _semaphore = null;
        _renderFinished = null;
        _imageAvailable = null;

        var props = new PlatformGraphicsExternalImageProperties
        {
            Width = (int)_info.Width,
            Height = (int)_info.Height,
            Format = PlatformGraphicsExternalImageFormat.B8G8R8A8UNorm,
        };

        if (_info.SyncKind == 0)
        {
            // Mechanism 1: D3D11 keyed-mutex global shared handle.
            var type = KnownPlatformGraphicsExternalImageHandleTypes.D3D11TextureGlobalSharedHandle;
            _image = _interop!.ImportImage(new PlatformHandle(_info.Handle, type), props);
        }
        else if (_info.SyncKind == 1)
        {
            // Mechanism 2: shared D3D12 resource (NT handle) + shared D3D12 fence as a timeline semaphore.
            props.MemorySize = (ulong)_info.Width * _info.Height * 4;
            var type = KnownPlatformGraphicsExternalImageHandleTypes.D3D11TextureNtHandle;
            _image = _interop!.ImportImage(new PlatformHandle(_info.Handle, type), props);
            _semaphore = _interop.ImportSemaphore(new PlatformHandle(
                _info.FenceHandle,
                KnownPlatformGraphicsExternalSemaphoreHandleTypes.Direct3D12FenceNtHandle));
        }
        else
        {
            // Mechanism 3: pure Vulkan. Opaque-NT-handle image memory + two binary semaphores.
            // MemorySize must equal the exported image's VkMemoryRequirements::size, which Avalonia's
            // importer asserts against its own image's requirements.
            props.MemorySize = _info.MemorySize;
            var imageType = KnownPlatformGraphicsExternalImageHandleTypes.VulkanOpaqueNtHandle;
            _image = _interop!.ImportImage(new PlatformHandle(_info.Handle, imageType), props);

            var semType = KnownPlatformGraphicsExternalSemaphoreHandleTypes.VulkanOpaqueNtHandle;
            _renderFinished = _interop.ImportSemaphore(
                new PlatformHandle(_info.RenderFinishedHandle, semType));
            _imageAvailable = _interop.ImportSemaphore(
                new PlatformHandle(_info.ImageAvailableHandle, semType));
        }
    }

    private PixelSize CurrentPixelSize()
    {
        var scaling = (this.GetVisualRoot() as TopLevel)?.RenderScaling ?? 1.0;
        var size = PixelSize.FromSize(Bounds.Size, scaling);
        return new PixelSize(Math.Max(1, size.Width), Math.Max(1, size.Height));
    }

    private async void Tick()
    {
        if (!_initialized || _busy || _surface == null || _image == null)
            return;
        _busy = true;
        try
        {
            if (_visual != null)
                _visual.Size = new Vector(Bounds.Width, Bounds.Height);

            var size = CurrentPixelSize();
            if (size != _lastSize)
            {
                NativeMethods.spike_resize(_spike, (uint)size.Width, (uint)size.Height, out _info);
                _lastSize = size;
                ImportResources();
            }

            var t = (float)_clock.Elapsed.TotalSeconds;
            NativeMethods.spike_render(_spike, t);

            if (_info.SyncKind == 0)
            {
                await _surface.UpdateWithKeyedMutexAsync(
                    _image, (uint)_info.ConsumerAcquireKey, (uint)_info.ConsumerReleaseKey);
            }
            else if (_info.SyncKind == 1)
            {
                var produced = NativeMethods.spike_fence_value(_spike);
                await _surface.UpdateWithTimelineSemaphoresAsync(
                    _image, _semaphore!, produced, _semaphore!, produced + 1);
            }
            else
            {
                // Vulkan: compositor waits on renderFinished (producer signalled it in end_producer),
                // then signals imageAvailable when done, which the producer waits on next frame.
                await _surface.UpdateWithSemaphoresAsync(_image, _renderFinished!, _imageAvailable!);
            }
        }
        catch (Exception ex)
        {
            Console.WriteLine("[spike] frame failed: " + ex);
            _initialized = false;
        }
        finally
        {
            _busy = false;
        }
    }
}
