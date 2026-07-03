using System;
using System.Linq;
using System.Threading.Tasks;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Platform;
using Avalonia.Rendering.Composition;
using Avalonia.VisualTree;
using Softblit.Interop;

namespace Softblit.Avalonia;

/// A <see cref="Canvas"/>-like control that presents CPU-produced framebuffers through softblit's
/// GPU pipeline (WGSL unpack + blit + scale + cursor overlay) into a texture shared zero-copy with
/// Avalonia's compositor. The consumer calls <see cref="Present"/> with packed frame bytes; softblit
/// does the format conversion and scaling on the GPU.
///
/// The backend (Vulkan opaque-NT-handle + semaphores, or D3D11 keyed mutex) is selected at attach
/// time from the compositor's advertised <see cref="ICompositionGpuInterop.SupportedImageHandleTypes"/>.
///
/// Thread affinity: the underlying Rust surface is <c>!Send</c>. Every method here must run on the
/// UI thread, which owns it.
public sealed class SoftblitCanvas : Control
{
    private Compositor? _compositor;
    private CompositionDrawingSurface? _drawingSurface;
    private CompositionSurfaceVisual? _visual;
    private ICompositionGpuInterop? _interop;

    private SoftblitSurface? _surface;
    private ShareInfoFfi _info;
    private ICompositionImportedGpuImage? _image;
    private ICompositionImportedGpuSemaphore? _renderFinished;
    private ICompositionImportedGpuSemaphore? _imageAvailable;

    private BackendFfi _backend;
    private PixelSize _targetSize;
    private PixelSize _sourceSize;
    private bool _ready;
    private bool _busy;

    private SoftblitPixelFormat _format = SoftblitPixelFormat.Bgra8;
    private SoftblitScaling _scaling = SoftblitScaling.Fit;
    private PixelSize? _pendingSourceSize;

    // Reused across presents (Present is UI-thread-only and the FFI consumes the rects synchronously)
    // so the common single-rect / full-frame present allocates nothing.
    private uint[] _rectScratch = new uint[4];

    /// Raised on the UI thread once the shared surface is created and imported and the canvas is
    /// ready to accept <see cref="Present"/> calls.
    public event EventHandler? Ready;

    public bool IsReady => _ready;

    /// Stats from the most recent <see cref="Present"/>.
    public SoftblitPresentStats LastPresentStats { get; private set; }

    /// The packed layout the consumer's frame bytes are in. Softblit unpacks it on the GPU.
    public SoftblitPixelFormat Format
    {
        get => _format;
        set
        {
            _format = value;
            if (_ready)
                _surface!.SetFormat((PixelFormatFfi)value);
        }
    }

    /// How the source framebuffer maps onto the (possibly differently sized) target.
    public SoftblitScaling Scaling
    {
        get => _scaling;
        set
        {
            _scaling = value;
            if (_ready)
                _surface!.SetScaling((ScalingModeFfi)value);
        }
    }

    /// The current source framebuffer size, in pixels. Frames passed to <see cref="Present"/> must
    /// match this size for the active <see cref="Format"/>.
    public PixelSize SourceSize => _sourceSize;

    protected override void OnAttachedToVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnAttachedToVisualTree(e);
        _ = InitializeAsync();
    }

    protected override void OnDetachedFromVisualTree(VisualTreeAttachmentEventArgs e)
    {
        _ready = false;
        _ = _image?.DisposeAsync();
        _ = _renderFinished?.DisposeAsync();
        _ = _imageAvailable?.DisposeAsync();
        _image = null;
        _renderFinished = null;
        _imageAvailable = null;
        _drawingSurface?.Dispose();
        _drawingSurface = null;
        _surface?.Dispose();
        _surface = null;
        base.OnDetachedFromVisualTree(e);
    }

    private async Task InitializeAsync()
    {
        var self = ElementComposition.GetElementVisual(this)!;
        _compositor = self.Compositor;
        _drawingSurface = _compositor.CreateDrawingSurface();
        _visual = _compositor.CreateSurfaceVisual();
        _visual.Size = new Vector(Bounds.Width, Bounds.Height);
        _visual.Surface = _drawingSurface;
        ElementComposition.SetElementChildVisual(this, _visual);

        _interop = await _compositor.TryGetCompositionGpuInterop();
        if (_interop == null)
            throw new NotSupportedException(
                "The Avalonia compositor exposes no ICompositionGpuInterop for this backend; " +
                "SoftblitCanvas requires the Vulkan or D3D11 rendering mode.");

        _backend = SelectBackend(_interop);

        _targetSize = CurrentPixelSize();
        _sourceSize = _pendingSourceSize ?? _targetSize;

        _surface = SoftblitSurface.Create(
            (uint)_targetSize.Width, (uint)_targetSize.Height,
            (PixelFormatFfi)_format, (ScalingModeFfi)_scaling, _backend);

        if (_pendingSourceSize is { } src)
            _surface.ResizeSource((uint)src.Width, (uint)src.Height);

        _info = _surface.ShareInfo();
        ImportResources();

        _ready = true;
        Ready?.Invoke(this, EventArgs.Empty);
    }

    private static BackendFfi SelectBackend(ICompositionGpuInterop interop)
    {
        if (interop.SupportedImageHandleTypes.Contains(
                KnownPlatformGraphicsExternalImageHandleTypes.VulkanOpaqueNtHandle))
            return BackendFfi.Vulkan;
        if (interop.SupportedImageHandleTypes.Contains(
                KnownPlatformGraphicsExternalImageHandleTypes.D3D11TextureGlobalSharedHandle))
            return BackendFfi.D3d11;
        throw new NotSupportedException(
            "Compositor advertises neither VulkanOpaqueNtHandle nor D3D11TextureGlobalSharedHandle; " +
            "supported: " + string.Join(", ", interop.SupportedImageHandleTypes));
    }

    private void ImportResources()
    {
        _ = _image?.DisposeAsync();
        _ = _renderFinished?.DisposeAsync();
        _ = _imageAvailable?.DisposeAsync();
        _image = null;
        _renderFinished = null;
        _imageAvailable = null;

        var props = new PlatformGraphicsExternalImageProperties
        {
            Width = (int)_info.Width,
            Height = (int)_info.Height,
            Format = PlatformGraphicsExternalImageFormat.B8G8R8A8UNorm,
        };

        switch (_info.SyncKind)
        {
            case SyncKindFfi.VulkanSemaphore:
                props.MemorySize = _info.MemorySize;
                _image = _interop!.ImportImage(
                    new PlatformHandle(_info.Handle,
                        KnownPlatformGraphicsExternalImageHandleTypes.VulkanOpaqueNtHandle),
                    props);
                _renderFinished = _interop.ImportSemaphore(new PlatformHandle(
                    _info.RenderFinishedHandle,
                    KnownPlatformGraphicsExternalSemaphoreHandleTypes.VulkanOpaqueNtHandle));
                _imageAvailable = _interop.ImportSemaphore(new PlatformHandle(
                    _info.ImageAvailableHandle,
                    KnownPlatformGraphicsExternalSemaphoreHandleTypes.VulkanOpaqueNtHandle));
                break;

            case SyncKindFfi.KeyedMutex:
                _image = _interop!.ImportImage(
                    new PlatformHandle(_info.Handle,
                        KnownPlatformGraphicsExternalImageHandleTypes.D3D11TextureGlobalSharedHandle),
                    props);
                break;

            case SyncKindFfi.D3d12Fence:
                throw new NotImplementedException(
                    "D3D12 fence sync is not wired into SoftblitCanvas; Avalonia's D3D11 compositor " +
                    "exposes no semaphore types on this hardware. Use the Vulkan or D3D11 keyed-mutex backend.");

            default:
                throw new NotSupportedException($"Unknown sync kind {_info.SyncKind}.");
        }
    }

    /// Uploads a frame and composites it. If <paramref name="dirty"/> is empty a single full-source
    /// rect is sent. Returns the upload accounting; if the canvas is not yet ready or a previous
    /// present is still committing, the frame is skipped.
    /// Zero-copy fast path: the caller's framebuffer is handed straight to the FFI (the generated
    /// binding pins it), and the dirty rects reuse a scratch buffer, so a steady-state present
    /// allocates nothing on the managed heap. Callers holding a <c>byte[]</c> bind here automatically.
    public SoftblitPresentStats Present(byte[] frame, ReadOnlySpan<PixelRect> dirty = default)
    {
        if (!_ready || _busy || _surface == null || _image == null)
            return new SoftblitPresentStats { Skipped = true };

        EnsureTargetSize();

        var rects = BuildRects(dirty);
        var stats = SoftblitPresentStats.FromFfi(_surface.Present(frame, rects));
        LastPresentStats = stats;

        _busy = true;
        _ = CommitAsync();
        return stats;
    }

    /// Convenience overload for callers that only have a span; it copies into a managed array. Prefer
    /// the <c>byte[]</c> overload on a hot path to avoid the per-present copy.
    public SoftblitPresentStats Present(ReadOnlySpan<byte> frame, ReadOnlySpan<PixelRect> dirty = default)
        => Present(frame.ToArray(), dirty);

    private uint[] BuildRects(ReadOnlySpan<PixelRect> dirty)
    {
        var count = dirty.IsEmpty ? 1 : dirty.Length;
        var rects = count == 1 ? _rectScratch : new uint[count * 4];

        if (dirty.IsEmpty)
        {
            rects[0] = 0;
            rects[1] = 0;
            rects[2] = (uint)_sourceSize.Width;
            rects[3] = (uint)_sourceSize.Height;
            return rects;
        }

        for (var i = 0; i < dirty.Length; i++)
        {
            var r = dirty[i];
            rects[i * 4 + 0] = (uint)r.X;
            rects[i * 4 + 1] = (uint)r.Y;
            rects[i * 4 + 2] = (uint)r.Width;
            rects[i * 4 + 3] = (uint)r.Height;
        }
        return rects;
    }

    private async Task CommitAsync()
    {
        try
        {
            if (_info.SyncKind == SyncKindFfi.VulkanSemaphore)
                await _drawingSurface!.UpdateWithSemaphoresAsync(_image!, _renderFinished!, _imageAvailable!);
            else
                await _drawingSurface!.UpdateWithKeyedMutexAsync(
                    _image!, (uint)_info.ConsumerAcquireKey, (uint)_info.ConsumerReleaseKey);
        }
        finally
        {
            _busy = false;
        }
    }

    private void EnsureTargetSize()
    {
        if (_visual != null)
            _visual.Size = new Vector(Bounds.Width, Bounds.Height);

        var size = CurrentPixelSize();
        if (size == _targetSize)
            return;

        // Reallocating the shared target invalidates the old handle (issue #19244): dispose the old
        // imported image and re-import against the fresh ShareInfo.
        _surface!.ResizeTarget((uint)size.Width, (uint)size.Height);
        _targetSize = size;
        _info = _surface.ShareInfo();
        ImportResources();
    }

    private PixelSize CurrentPixelSize()
    {
        var scaling = (this.GetVisualRoot() as TopLevel)?.RenderScaling ?? 1.0;
        var size = PixelSize.FromSize(Bounds.Size, scaling);
        return new PixelSize(Math.Max(1, size.Width), Math.Max(1, size.Height));
    }

    /// Sets the source framebuffer size. Frames passed to <see cref="Present"/> must then match this
    /// size. May be called before the canvas is ready; it is applied on init.
    public void ResizeSource(PixelSize size)
    {
        size = new PixelSize(Math.Max(1, size.Width), Math.Max(1, size.Height));
        _sourceSize = size;
        if (_ready)
            _surface!.ResizeSource((uint)size.Width, (uint)size.Height);
        else
            _pendingSourceSize = size;
    }

    /// Installs an RGBA8 cursor overlay composited on the GPU.
    public void SetCursor(ReadOnlySpan<byte> rgba, PixelSize size)
    {
        if (!_ready)
            throw new InvalidOperationException("SoftblitCanvas is not ready; wait for the Ready event.");
        _surface!.SetCursor(rgba.ToArray(), (uint)size.Width, (uint)size.Height);
    }

    public void ClearCursor()
    {
        if (_ready)
            _surface!.ClearCursor();
    }

    /// Positions the cursor overlay in source-framebuffer coordinates.
    public void SetCursorPosition(PixelPoint position)
    {
        if (_ready)
            _surface!.SetCursorPosition(position.X, position.Y);
    }
}
