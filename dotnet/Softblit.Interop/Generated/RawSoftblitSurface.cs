using System;
using System.Runtime.InteropServices;
using Softblit.Interop;
using Softblit.Interop.Diplomat;

namespace Softblit.Interop.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct SoftblitSurface
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_create", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultSoftblitSurfaceErrFfi Create(uint width, uint height, PixelFormatFfi format, ScalingModeFfi scaling, BackendFfi backend);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_share_info", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ShareInfoFfi ShareInfo(SoftblitSurface* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_present", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultPresentStatsFfiErrFfi Present(SoftblitSurface* handle, DiplomatSliceU8 bytes, DiplomatSliceU32 dirtyRects);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_resize_source", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void ResizeSource(SoftblitSurface* handle, uint width, uint height);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_resize_target", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultVoidErrFfi ResizeTarget(SoftblitSurface* handle, uint width, uint height);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_set_format", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void SetFormat(SoftblitSurface* handle, PixelFormatFfi format);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_set_scaling", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void SetScaling(SoftblitSurface* handle, ScalingModeFfi scaling);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_set_cursor", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void SetCursor(SoftblitSurface* handle, DiplomatSliceU8 image, uint width, uint height);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_clear_cursor", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void ClearCursor(SoftblitSurface* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_set_cursor_position", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void SetCursorPosition(SoftblitSurface* handle, int x, int y);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "SoftblitSurface_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(SoftblitSurface* handle);
}