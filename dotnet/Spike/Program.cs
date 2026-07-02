using System;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;

namespace Spike;

internal static class Program
{
    [STAThread]
    public static void Main(string[] args) =>
        BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);

    public static AppBuilder BuildAvaloniaApp()
    {
        var builder = AppBuilder.Configure<App>().UsePlatformDetect().LogToTrace();
        // The default spike backend is pure Vulkan↔Vulkan, which requires Avalonia's Vulkan
        // compositor (it advertises VulkanOpaqueNtHandle images + Vulkan semaphores). Set
        // SPIKE_AVALONIA_D3D=1 to fall back to the default D3D11 compositor for the d3d11/dx12 backends.
        if (Environment.GetEnvironmentVariable("SPIKE_AVALONIA_D3D") != "1")
        {
            builder = builder.With(new Win32PlatformOptions
            {
                RenderingMode = new[] { Win32RenderingMode.Vulkan },
            });
        }
        return builder;
    }
}

internal sealed class App : Application
{
    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            desktop.MainWindow = new Window
            {
                Title = "softblit Phase-0 spike",
                Width = 800,
                Height = 600,
                Content = new SpikeControl(),
            };
        }

        base.OnFrameworkInitializationCompleted();
    }
}
