using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace QuotaBar.Monitor;

internal sealed class UpdatePolicy : SafeHandleZeroOrMinusOneIsInvalid
{
    private const string Library = "quotabar_native";
    private UpdatePolicy() : base(true) { }
    internal enum OnConfigure { Schedule, Cancel, Install }
    internal enum Offer { Current, Blocked, Available }

    [DllImport(Library, EntryPoint = "qb_update_policy_new")]
    internal static extern UpdatePolicy Create();
    [DllImport(Library, EntryPoint = "qb_update_policy_free")]
    private static extern void Free(nint handle);
    protected override bool ReleaseHandle() { Free(handle); return true; }

    [DllImport(Library, EntryPoint = "qb_update_configure")]
    internal static extern OnConfigure Configure(UpdatePolicy policy, bool automatic, bool canInstall, bool suspended);
    [DllImport(Library, EntryPoint = "qb_update_begin_check")]
    internal static extern bool BeginCheck(UpdatePolicy policy, bool manual);
    [DllImport(Library, EntryPoint = "qb_update_attempted")]
    internal static extern void Attempted(UpdatePolicy policy, double now);
    [DllImport(Library, EntryPoint = "qb_update_offer")]
    internal static extern Offer Evaluate(UpdatePolicy policy, [MarshalAs(UnmanagedType.LPUTF8Str)] string current,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string version);
    [DllImport(Library, EntryPoint = "qb_update_should_download")]
    internal static extern bool ShouldDownload(UpdatePolicy policy, bool requested);
    [DllImport(Library, EntryPoint = "qb_update_staged")]
    internal static extern void Staged(UpdatePolicy policy, [MarshalAs(UnmanagedType.LPUTF8Str)] string version);
    [DllImport(Library, EntryPoint = "qb_update_block")]
    internal static extern void Block(UpdatePolicy policy, [MarshalAs(UnmanagedType.LPUTF8Str)] string version);
    [DllImport(Library, EntryPoint = "qb_update_begin_install")]
    internal static extern bool BeginInstall(UpdatePolicy policy, bool force);
    [DllImport(Library, EntryPoint = "qb_update_begin_handoff")]
    internal static extern bool BeginHandoff(UpdatePolicy policy, bool force);
    [DllImport(Library, EntryPoint = "qb_update_may_install")]
    internal static extern bool MayInstall(UpdatePolicy policy, bool force);
    [DllImport(Library, EntryPoint = "qb_update_finish")]
    internal static extern void Finish(UpdatePolicy policy, bool cancelled);
    [DllImport(Library, EntryPoint = "qb_update_delay")]
    internal static extern double Delay(UpdatePolicy policy, double now);
}
