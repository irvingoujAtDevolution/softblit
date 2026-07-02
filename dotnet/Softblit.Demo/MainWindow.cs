using System;
using System.Diagnostics;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Threading;
using Softblit.Avalonia;

namespace Softblit.Demo;

internal sealed class MainWindow : Window
{
    private const int SourceW = 640;
    private const int SourceH = 480;

    private readonly SoftblitCanvas _canvas = new();
    private readonly TextBlock _stats = new() { Foreground = Brushes.White, Margin = new Thickness(8, 0) };
    private readonly DispatcherTimer _timer = new() { Interval = TimeSpan.FromMilliseconds(16) };
    private readonly Stopwatch _clock = Stopwatch.StartNew();

    private FrameGenerator _frames;
    private SoftblitPixelFormat _format = SoftblitPixelFormat.Rgb24;
    private bool _animate = true;
    private bool _dirtyStress;
    private bool _cursor;
    private bool _needBackground = true;

    public MainWindow()
    {
        Title = "Softblit Avalonia demo";
        Width = 1000;
        Height = 720;
        Background = Brushes.Black;

        _frames = new FrameGenerator(_format, SourceW, SourceH);
        _canvas.Format = _format;
        _canvas.Scaling = SoftblitScaling.Fit;
        _canvas.ResizeSource(new PixelSize(SourceW, SourceH));

        Content = BuildLayout();

        _canvas.Ready += (_, _) =>
        {
            _needBackground = true;
            _timer.Start();
        };
        _timer.Tick += (_, _) => Tick();
    }

    private Control BuildLayout()
    {
        var formats = new[] { SoftblitPixelFormat.Rgb24, SoftblitPixelFormat.Bgra8, SoftblitPixelFormat.I420 };
        var formatPicker = new ComboBox
        {
            ItemsSource = formats,
            SelectedItem = _format,
            VerticalAlignment = VerticalAlignment.Center,
        };
        formatPicker.SelectionChanged += (_, _) =>
        {
            if (formatPicker.SelectedItem is SoftblitPixelFormat f)
                SwitchFormat(f);
        };

        var scalings = new[]
        {
            SoftblitScaling.Fit, SoftblitScaling.Fill, SoftblitScaling.Stretch,
            SoftblitScaling.Integer, SoftblitScaling.Native1x,
        };
        var scalingPicker = new ComboBox
        {
            ItemsSource = scalings,
            SelectedItem = SoftblitScaling.Fit,
            VerticalAlignment = VerticalAlignment.Center,
        };
        scalingPicker.SelectionChanged += (_, _) =>
        {
            if (scalingPicker.SelectedItem is SoftblitScaling s)
                _canvas.Scaling = s;
        };

        var animate = new CheckBox { Content = "Animate", IsChecked = _animate, VerticalAlignment = VerticalAlignment.Center };
        animate.IsCheckedChanged += (_, _) => _animate = animate.IsChecked == true;

        var dirty = new CheckBox { Content = "Dirty-rect stress", IsChecked = _dirtyStress, VerticalAlignment = VerticalAlignment.Center };
        dirty.IsCheckedChanged += (_, _) =>
        {
            _dirtyStress = dirty.IsChecked == true;
            _needBackground = true;
        };

        var cursor = new CheckBox { Content = "Cursor overlay", IsChecked = _cursor, VerticalAlignment = VerticalAlignment.Center };
        cursor.IsCheckedChanged += (_, _) =>
        {
            _cursor = cursor.IsChecked == true;
            if (_canvas.IsReady && !_cursor)
                _canvas.ClearCursor();
        };

        var toolbar = new StackPanel
        {
            Orientation = Orientation.Horizontal,
            Spacing = 12,
            Margin = new Thickness(8),
        };
        toolbar.Children.Add(new TextBlock { Text = "Format:", Foreground = Brushes.White, VerticalAlignment = VerticalAlignment.Center });
        toolbar.Children.Add(formatPicker);
        toolbar.Children.Add(new TextBlock { Text = "Scaling:", Foreground = Brushes.White, VerticalAlignment = VerticalAlignment.Center });
        toolbar.Children.Add(scalingPicker);
        toolbar.Children.Add(animate);
        toolbar.Children.Add(dirty);
        toolbar.Children.Add(cursor);

        var bottom = new Border
        {
            Background = new SolidColorBrush(Color.FromArgb(160, 0, 0, 0)),
            Padding = new Thickness(8, 4),
            Child = _stats,
        };

        DockPanel.SetDock(toolbar, Dock.Top);
        DockPanel.SetDock(bottom, Dock.Bottom);
        var root = new DockPanel();
        root.Children.Add(toolbar);
        root.Children.Add(bottom);
        root.Children.Add(_canvas);
        return root;
    }

    private void SwitchFormat(SoftblitPixelFormat format)
    {
        _format = format;
        _frames = new FrameGenerator(format, SourceW, SourceH);
        _canvas.Format = format;
        _needBackground = true;
    }

    private void Tick()
    {
        if (!_canvas.IsReady)
            return;

        var t = (float)_clock.Elapsed.TotalSeconds;
        SoftblitPresentStats stats;

        if (_dirtyStress)
        {
            if (_needBackground)
            {
                _frames.FillPlasma(t);
                _canvas.Present(_frames.Buffer);
                _needBackground = false;
            }

            var cx = SourceW / 2 + (int)(180 * MathF.Cos(t));
            var cy = SourceH / 2 + (int)(140 * MathF.Sin(t * 1.3f));
            var x = Math.Clamp((cx - 32) & ~1, 0, SourceW - 64);
            var y = Math.Clamp((cy - 32) & ~1, 0, SourceH - 64);
            var r = (byte)((0.5f + 0.5f * MathF.Sin(t * 3f)) * 255f);
            _frames.FillRect(x, y, 64, 64, r, (byte)(255 - r), 64);
            stats = _canvas.Present(_frames.Buffer, new[] { new PixelRect(x, y, 64, 64) });
            MoveCursor(t);
        }
        else if (_animate)
        {
            _frames.FillPlasma(t);
            stats = _canvas.Present(_frames.Buffer);
            MoveCursor(t);
        }
        else
        {
            if (_needBackground)
            {
                _frames.FillPlasma(t);
                _canvas.Present(_frames.Buffer);
                _needBackground = false;
            }
            return;
        }

        _stats.Text = stats.Skipped
            ? "present: skipped (busy or not ready)"
            : $"format={_format}  rects={stats.RectsUploaded}  bytes={stats.BytesUploaded:N0}  " +
              $"source={SourceW}x{SourceH}";
    }

    private void MoveCursor(float t)
    {
        if (!_cursor)
            return;

        if (_cursorImage == null)
            _cursorImage = BuildCursor();
        _canvas.SetCursor(_cursorImage, new PixelSize(CursorSize, CursorSize));

        var x = SourceW / 2 + (int)(200 * MathF.Cos(t * 0.8f));
        var y = SourceH / 2 + (int)(160 * MathF.Sin(t * 0.8f));
        _canvas.SetCursorPosition(new PixelPoint(x, y));
    }

    private const int CursorSize = 16;
    private byte[]? _cursorImage;

    private static byte[] BuildCursor()
    {
        var img = new byte[CursorSize * CursorSize * 4];
        for (var y = 0; y < CursorSize; y++)
        for (var x = 0; x < CursorSize; x++)
        {
            var i = (y * CursorSize + x) * 4;
            var onCross = x == CursorSize / 2 || y == CursorSize / 2;
            img[i + 0] = 255;                        // R
            img[i + 1] = 255;                        // G
            img[i + 2] = 0;                          // B
            img[i + 3] = (byte)(onCross ? 255 : 0);  // A
        }
        return img;
    }
}
