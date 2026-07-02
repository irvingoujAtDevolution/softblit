using System;

namespace Softblit.Interop;

public class ErrFfiException : Exception
{
    public ErrFfi Inner { get; }

    public ErrFfiException(ErrFfi inner) : base(
        $"ErrFfi: {inner}"
    )
    {
        Inner = inner;
    }
}