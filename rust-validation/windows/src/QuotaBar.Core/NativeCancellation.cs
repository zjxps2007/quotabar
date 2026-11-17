using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace QuotaBar.Core;

public sealed class NativeCancellation() : SafeHandleZeroOrMinusOneIsInvalid(true)
{
    private const string Library = "quotabar_native";
    [DllImport(Library, EntryPoint = "qb_cancel_new")] public static extern NativeCancellation Create();
    [DllImport(Library, EntryPoint = "qb_cancel_free")] private static extern void Free(IntPtr value);
    [DllImport(Library, EntryPoint = "qb_cancel_set")] private static extern void Set(NativeCancellation value);
    public void Cancel() => Set(this);
    protected override bool ReleaseHandle() { Free(handle); return true; }
}
