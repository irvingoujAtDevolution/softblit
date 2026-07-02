using System;
using System.Runtime.InteropServices;

namespace Softblit.Interop.Raw;

using Softblit.Interop;

[StructLayout(LayoutKind.Sequential)]
internal partial struct DiplomatResultSoftblitSurfaceErrFfi
{
    [StructLayout(LayoutKind.Explicit)]
    private unsafe struct InnerUnion
    {
        [FieldOffset(0)] internal SoftblitSurface* ok;
        [FieldOffset(0)] internal ErrFfi err;
    }

    private InnerUnion _inner;

    [MarshalAs(UnmanagedType.U1)]
    public bool IsOk;
    public unsafe SoftblitSurface* Ok => IsOk ? _inner.ok : throw new InvalidOperationException("Result does not contain Ok value");
    public ErrFfi Err => !IsOk ? _inner.err : throw new InvalidOperationException("Result does not contain Err value");
}