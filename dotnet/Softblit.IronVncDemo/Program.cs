using System;
using Avalonia;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Themes.Fluent;
using Devolutions.IronVnc;

namespace Softblit.IronVncDemo;

internal static class Program
{
    // Live Xtigervnc/XFCE target (see plan). Fed through IronVncConfig, which reads these env vars.
    private const string Host = "10.9.0.50";
    private const string Port = "5901";
    private const string Username = "irving";
    private const string Password = "990225ojy";

    [STAThread]
    public static void Main(string[] args)
    {
        Environment.SetEnvironmentVariable("IRONVNC_HOST", Host);
        Environment.SetEnvironmentVariable("IRONVNC_PORT", Port);
        Environment.SetEnvironmentVariable("IRONVNC_USERNAME", Username);
        Environment.SetEnvironmentVariable("IRONVNC_PASSWORD", Password);
        IronVncConfig.GetInstance().ProcessArguments(args);

        BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);
    }

    public static AppBuilder BuildAvaloniaApp() =>
        AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .LogToTrace()
            // Vulkan first: SoftblitCanvas needs the Vulkan opaque-NT-handle + semaphore path on the
            // Intel iGPU. ANGLE/EGL only as a fallback (no softblit output there).
            .With(new Win32PlatformOptions
            {
                RenderingMode = new[] { Win32RenderingMode.Vulkan, Win32RenderingMode.AngleEgl },
            });
}

internal sealed class App : Application
{
    public override void Initialize() => Styles.Add(new FluentTheme());

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
            desktop.MainWindow = new MainWindow();

        base.OnFrameworkInitializationCompleted();
    }
}
