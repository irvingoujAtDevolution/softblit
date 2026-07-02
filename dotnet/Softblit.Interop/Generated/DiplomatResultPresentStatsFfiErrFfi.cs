using System;
using System.Runtime.InteropServices;

namespace Softblit.Interop.Raw;

using Softblit.Interop;

[StructLayout(LayoutKind.Sequential)]
internal partial struct DiplomatResultPresentStatsFfiErrFfi
{
    [StructLayout(LayoutKind.Explicit)]
    private unsafe struct InnerUnion
    {
        [FieldOffset(0)] internal PresentStatsFfi ok;
        [FieldOffset(0)] internal ErrFfi err;
    }

    private InnerUnion _inner;

    [MarshalAs(UnmanagedType.U1)]
    public bool IsOk;
    public PresentStatsFfi Ok => IsOk ? _inner.ok : throw new InvalidOperationException("Result does not contain Ok value");
    public ErrFfi Err => !IsOk ? _inner.err : throw new InvalidOperationException("Result does not contain Err value");
}