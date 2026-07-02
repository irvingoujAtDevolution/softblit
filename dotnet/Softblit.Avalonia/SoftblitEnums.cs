using Softblit.Interop;

namespace Softblit.Avalonia;

/// Friendly mirror of <see cref="PixelFormatFfi"/>. The consumer's frame bytes must be packed in
/// this layout; softblit's WGSL unpacks it on the GPU.
public enum SoftblitPixelFormat
{
    Rgba8 = 0,
    Bgra8 = 1,
    Rgbx8 = 2,
    Bgrx8 = 3,
    Rgb24 = 4,
    Bgr24 = 5,
    Rgb565 = 6,
    Rgb555 = 7,
    Gray8 = 8,
    Gray16 = 9,
    I420 = 10,
}

/// Friendly mirror of <see cref="ScalingModeFfi"/>: how the source framebuffer maps onto the target.
public enum SoftblitScaling
{
    Fit = 0,
    Fill = 1,
    Stretch = 2,
    Integer = 3,
    Native1x = 4,
}

/// Result of the last <c>Present</c>, surfacing softblit's zero-copy accounting.
public readonly struct SoftblitPresentStats
{
    public uint RectsUploaded { get; init; }
    public ulong BytesUploaded { get; init; }
    public bool Skipped { get; init; }

    internal static SoftblitPresentStats FromFfi(PresentStatsFfi s) => new()
    {
        RectsUploaded = s.RectsUploaded,
        BytesUploaded = s.BytesUploaded,
        Skipped = s.Skipped,
    };
}
