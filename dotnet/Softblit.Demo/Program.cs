using System;
using Avalonia;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Themes.Fluent;
using Avalonia.Controls;

namespace Softblit.Demo;

internal static class Program
{
    [STAThread]
    public static void Main(string[] args) =>
        BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);

    public static AppBuilder BuildAvaloniaApp() =>
        AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .LogToTrace()
            // Vulkan first so the Intel iGPU path (VulkanOpaqueNtHandle + semaphores) is available;
            // ANGLE/EGL as fallback. The D3D11 keyed-mutex backend needs the default D3D11 renderer,
            // which a discrete-GPU box can select instead.
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
