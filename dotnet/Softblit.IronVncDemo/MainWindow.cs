using System;
using System.Collections.Generic;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Threading;
using Devolutions.IronVnc;
using Devolutions.IronVnc.Example;
using Softblit.Avalonia;

namespace Softblit.IronVncDemo;

/// Hosts a <see cref="SoftblitCanvas"/> fed by a live IronVNC session. Each VNC framebuffer update
/// arrives (on a worker thread) as an RGB24 subregion; we blit it into a persistent full-desktop
/// framebuffer and present the dirty rect through softblit's GPU pipeline — zero-copy into Avalonia's
/// Vulkan compositor.
internal sealed class MainWindow : Window
{
    // Desktop size we ask the server for; the framebuffer auto-grows if the server reports larger.
    private const ushort InitialWidth = 1600;
    private const ushort InitialHeight = 900;

    private readonly SoftblitCanvas _canvas = new();
    private readonly TextBlock _stats = new()
    {
        Foreground = Brushes.White,
        VerticalAlignment = VerticalAlignment.Center,
        Margin = new Thickness(8, 0),
    };

    private readonly IronVnc _ironVnc;
    private readonly IronVncConfig _config = IronVncConfig.GetInstance();
    private CancellationTokenSource? _terminator;

    // Full-desktop RGB24 framebuffer. UI-thread-only state.
    private byte[] _fullFb = Array.Empty<byte>();
    private int _fbWidth;
    private int _fbHeight;

    // Dirty rects (source coords) not yet accepted by a present; carried forward if a present is
    // skipped because the canvas is busy, so no region is permanently lost.
    private readonly List<PixelRect> _pendingDirty = new();
    private bool _fullDirty;

    private ulong _frameCount;

    public MainWindow()
    {
        Title = "Softblit IronVNC demo";
        Width = 1280;
        Height = 800;
        Background = Brushes.Black;

        _canvas.Format = SoftblitPixelFormat.Rgb24;
        _canvas.Scaling = SoftblitScaling.Fit;
        AllocateFramebuffer(InitialWidth, InitialHeight);

        Content = BuildLayout();

        _canvas.Ready += (_, _) =>
        {
            Console.WriteLine("SoftblitCanvas ready");
            _fullDirty = true;
            PresentPending();
        };

        _ironVnc = new IronVnc(_config);
        RegisterServerEvents();
        WireInput();

        _ = StartConnection();
    }

    private Control BuildLayout()
    {
        var bottom = new Border
        {
            Background = new SolidColorBrush(Color.FromArgb(160, 0, 0, 0)),
            Padding = new Thickness(8, 4),
            Child = _stats,
        };
        DockPanel.SetDock(bottom, Dock.Bottom);

        var root = new DockPanel();
        root.Children.Add(bottom);
        root.Children.Add(_canvas);
        return root;
    }

    private async System.Threading.Tasks.Task StartConnection()
    {
        try
        {
            Console.WriteLine($"Connecting to {_config.Host}:{_config.Port} as {_config.Username}...");
            var connectResult = await _ironVnc.Connect();
            if (connectResult.connectionResultType == ConnectResultType.Failed)
            {
                Console.WriteLine("Connection failed");
                return;
            }
            if (connectResult.connectionResultType == ConnectResultType.TransportUpgrade)
                throw new NotImplementedException("VNC transport upgrade is not supported in this demo.");

            var ignition = await _ironVnc.IgniteSession(connectResult, InitialWidth, InitialHeight);
            if (ignition.terminated)
                throw new Exception("Ignite session failed.");

            _terminator = _ironVnc.StartActiveSession(ignition.readyToProcess!);
            Console.WriteLine("Active session started; frames flowing.");
        }
        catch (Exception ex)
        {
            Console.WriteLine("Connection error: " + ex);
        }
    }

    private void RegisterServerEvents()
    {
        _ironVnc.FramebufferUpdatedEvent += (_, region) =>
        {
            // Runs on IronVNC's worker thread. Extract the RGB24 subregion here, then marshal a plain
            // byte[] + coords to the UI thread (the softblit surface is UI-thread-only).
            var left = (int)region.GetLeft();
            var top = (int)region.GetTop();
            var w = (int)region.GetWidth();
            var h = (int)region.GetHeight();
            if (w == 0 || h == 0)
            {
                region.Dispose();
                return;
            }

            var vec = region.GetRgbBuf();
            var bytes = new byte[(int)vec.Len()];
            vec.Fill(bytes);
            vec.Dispose();
            region.Dispose();

            Dispatcher.UIThread.Post(() =>
            {
                try
                {
                    BlitRegion(left, top, w, h, bytes);
                }
                catch (Exception ex)
                {
                    Console.WriteLine("Blit error: " + ex.Message);
                }
            });
        };

        _ironVnc.FramebufferResizedEvent += (_, resized) =>
        {
            var w = (int)resized.GetWidth();
            var h = (int)resized.GetHeight();
            Dispatcher.UIThread.Post(() => ResizeFramebuffer(w, h));
        };

        _ironVnc.NewDisplaysAvailableEvent += (_, displays) =>
        {
            var display = displays.Next();
            if (display == null)
                return;
            var w = (int)display.GetWidth();
            var h = (int)display.GetHeight();
            Dispatcher.UIThread.Post(() => ResizeFramebuffer(w, h));
        };
    }

    private void AllocateFramebuffer(int width, int height)
    {
        _fbWidth = Math.Max(1, width);
        _fbHeight = Math.Max(1, height);
        _fullFb = new byte[_fbWidth * _fbHeight * 3];
        _canvas.ResizeSource(new PixelSize(_fbWidth, _fbHeight));
        _fullDirty = true;
    }

    private void ResizeFramebuffer(int width, int height)
    {
        width = Math.Max(1, width);
        height = Math.Max(1, height);
        if (width == _fbWidth && height == _fbHeight)
            return;

        var next = new byte[width * height * 3];
        var copyW = Math.Min(_fbWidth, width);
        var copyH = Math.Min(_fbHeight, height);
        for (var row = 0; row < copyH; row++)
            Array.Copy(_fullFb, row * _fbWidth * 3, next, row * width * 3, copyW * 3);

        _fullFb = next;
        _fbWidth = width;
        _fbHeight = height;
        _canvas.ResizeSource(new PixelSize(_fbWidth, _fbHeight));
        _fullDirty = true;
        Console.WriteLine($"Framebuffer resized to {_fbWidth}x{_fbHeight}");
        PresentPending();
    }

    private void BlitRegion(int left, int top, int w, int h, byte[] src)
    {
        // Grow if the server drew outside our current desktop extents (e.g. we guessed too small).
        var needW = Math.Max(_fbWidth, left + w);
        var needH = Math.Max(_fbHeight, top + h);
        if (needW != _fbWidth || needH != _fbHeight)
            ResizeFramebuffer(needW, needH);

        var srcStride = w * 3;
        var dstStride = _fbWidth * 3;
        if (src.Length < srcStride * h)
            return;

        for (var row = 0; row < h; row++)
        {
            var dstOffset = (top + row) * dstStride + left * 3;
            Array.Copy(src, row * srcStride, _fullFb, dstOffset, srcStride);
        }

        _pendingDirty.Add(new PixelRect(left, top, w, h));
        _frameCount++;
        PresentPending();
    }

    private void PresentPending()
    {
        if (!_canvas.IsReady)
            return;

        SoftblitPresentStats stats;
        if (_fullDirty)
        {
            stats = _canvas.Present(_fullFb);
            if (!stats.Skipped)
            {
                _fullDirty = false;
                _pendingDirty.Clear();
            }
        }
        else if (_pendingDirty.Count > 0)
        {
            stats = _canvas.Present(_fullFb, _pendingDirty.ToArray());
            if (!stats.Skipped)
                _pendingDirty.Clear();
        }
        else
        {
            return;
        }

        _stats.Text = stats.Skipped
            ? $"present skipped (busy)  frames={_frameCount}  desktop={_fbWidth}x{_fbHeight}"
            : $"desktop={_fbWidth}x{_fbHeight}  frames={_frameCount}  " +
              $"rects={stats.RectsUploaded}  bytes={stats.BytesUploaded:N0}";
    }

    // --- Input (nice-to-have): forward keyboard + pointer to the VNC session. Pointer coordinates
    // are mapped proportionally from control space to source space; with Fit scaling this ignores
    // letterbox offset, so it is approximate. ---

    private void WireInput()
    {
        KeyDown += (_, arg) => ForwardKey(arg, true);
        KeyUp += (_, arg) => ForwardKey(arg, false);

        _canvas.PointerMoved += (_, e) =>
        {
            var (x, y) = ToSource(e.GetPosition(_canvas));
            _ironVnc.userEventChannel.Writer.TryWrite(UserEvent.PointerMoved(x, y));
        };
        _canvas.PointerPressed += (_, e) => ForwardButton(e.GetCurrentPoint(_canvas).Properties.PointerUpdateKind, true);
        _canvas.PointerReleased += (_, e) => ForwardButton(e.GetCurrentPoint(_canvas).Properties.PointerUpdateKind, false);
    }

    private void ForwardKey(KeyEventArgs arg, bool down)
    {
        var keys = KeyMapper.map(arg.PhysicalKey, arg.KeySymbol);
        foreach (var key in keys)
            _ironVnc.userEventChannel.Writer.TryWrite(UserEvent.Key(down, key));
    }

    private void ForwardButton(PointerUpdateKind kind, bool press)
    {
        Devolutions.IronVnc.MouseButton? button = kind switch
        {
            PointerUpdateKind.LeftButtonPressed or PointerUpdateKind.LeftButtonReleased => Devolutions.IronVnc.MouseButton.Left,
            PointerUpdateKind.RightButtonPressed or PointerUpdateKind.RightButtonReleased => Devolutions.IronVnc.MouseButton.Right,
            PointerUpdateKind.MiddleButtonPressed or PointerUpdateKind.MiddleButtonReleased => Devolutions.IronVnc.MouseButton.Middle,
            PointerUpdateKind.XButton1Pressed or PointerUpdateKind.XButton1Released => Devolutions.IronVnc.MouseButton.X1,
            PointerUpdateKind.XButton2Pressed or PointerUpdateKind.XButton2Released => Devolutions.IronVnc.MouseButton.X2,
            _ => null,
        };
        if (button is { } b)
            _ironVnc.userEventChannel.Writer.TryWrite(UserEvent.MouseButton(press, b));
    }

    private (ushort, ushort) ToSource(Point p)
    {
        var bw = Math.Max(1.0, _canvas.Bounds.Width);
        var bh = Math.Max(1.0, _canvas.Bounds.Height);
        var x = (ushort)Math.Clamp(p.X / bw * _fbWidth, 0, _fbWidth - 1);
        var y = (ushort)Math.Clamp(p.Y / bh * _fbHeight, 0, _fbHeight - 1);
        return (x, y);
    }

    protected override void OnClosed(EventArgs e)
    {
        _terminator?.Cancel();
        base.OnClosed(e);
    }
}
