namespace Softblit.Interop;

public enum ErrFfi : int
{
    NoAdapter = 0,
    Unsupported = 1,
    Platform = 2,
    InvalidRect = 3,
    BufferSizeMismatch = 4,
    SurfaceLost = 5,
    Device = 6,
}