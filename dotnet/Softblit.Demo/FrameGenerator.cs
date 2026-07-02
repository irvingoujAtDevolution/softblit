using System;
using Softblit.Avalonia;

namespace Softblit.Demo;

/// Produces CPU framebuffers in each softblit pixel format so the demo exercises the corresponding
/// WGSL unpack path. Owns a byte buffer sized to the current format and source resolution.
internal sealed class FrameGenerator
{
    public int Width { get; }
    public int Height { get; }
    public SoftblitPixelFormat Format { get; }
    public byte[] Buffer { get; }

    public FrameGenerator(SoftblitPixelFormat format, int width, int height)
    {
        Format = format;
        Width = width & ~1;   // keep even for I420 chroma subsampling
        Height = height & ~1;
        Buffer = new byte[BufferLength(format, Width, Height)];
    }

    public static int BufferLength(SoftblitPixelFormat format, int w, int h) => format switch
    {
        SoftblitPixelFormat.Bgra8 => w * h * 4,
        SoftblitPixelFormat.Rgb24 => w * h * 3,
        SoftblitPixelFormat.I420 => w * h + 2 * ((w / 2) * (h / 2)),
        _ => throw new NotSupportedException($"Demo does not generate {format}; add a byte layout for it."),
    };

    /// Fills the whole buffer with an animated plasma so every dirty byte differs frame to frame.
    public void FillPlasma(float t)
    {
        for (var y = 0; y < Height; y++)
        for (var x = 0; x < Width; x++)
        {
            var v = MathF.Sin(x * 0.03f + t)
                    + MathF.Sin(y * 0.04f + t * 1.3f)
                    + MathF.Sin((x + y) * 0.02f + t * 0.7f);
            var r = (byte)((0.5f + 0.5f * MathF.Sin(v)) * 255f);
            var g = (byte)((0.5f + 0.5f * MathF.Sin(v + 2.094f)) * 255f);
            var b = (byte)((0.5f + 0.5f * MathF.Sin(v + 4.188f)) * 255f);
            SetPixel(x, y, r, g, b);
        }
    }

    /// Paints a solid rectangle; used by the dirty-rect stress path so only these bytes are uploaded.
    public void FillRect(int rx, int ry, int rw, int rh, byte r, byte g, byte b)
    {
        for (var y = ry; y < ry + rh; y++)
        for (var x = rx; x < rx + rw; x++)
            SetPixel(x, y, r, g, b);
    }

    private void SetPixel(int x, int y, byte r, byte g, byte b)
    {
        switch (Format)
        {
            case SoftblitPixelFormat.Bgra8:
            {
                var i = (y * Width + x) * 4;
                Buffer[i + 0] = b;
                Buffer[i + 1] = g;
                Buffer[i + 2] = r;
                Buffer[i + 3] = 255;
                break;
            }
            case SoftblitPixelFormat.Rgb24:
            {
                var i = (y * Width + x) * 3;
                Buffer[i + 0] = r;
                Buffer[i + 1] = g;
                Buffer[i + 2] = b;
                break;
            }
            case SoftblitPixelFormat.I420:
            {
                var yy = (byte)Math.Clamp(0.299f * r + 0.587f * g + 0.114f * b, 0, 255);
                Buffer[y * Width + x] = yy;

                var cw = Width / 2;
                var ch = Height / 2;
                var uPlane = Width * Height;
                var vPlane = uPlane + cw * ch;
                var ci = (y / 2) * cw + (x / 2);
                Buffer[uPlane + ci] = (byte)Math.Clamp(-0.169f * r - 0.331f * g + 0.5f * b + 128f, 0, 255);
                Buffer[vPlane + ci] = (byte)Math.Clamp(0.5f * r - 0.419f * g - 0.081f * b + 128f, 0, 255);
                break;
            }
            default:
                throw new NotSupportedException($"Demo does not generate {Format}.");
        }
    }
}
