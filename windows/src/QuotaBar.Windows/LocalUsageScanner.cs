using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;
using QuotaBar.Core;

namespace QuotaBar.Windows;

public sealed class LocalUsageScanner : IDisposable
{
    private const string Library = "quotabar_native";
    private readonly ScannerHandle handle = CreateScanner();

    private sealed class ScannerHandle() : SafeHandleZeroOrMinusOneIsInvalid(true)
    {
        protected override bool ReleaseHandle() { FreeScanner(handle); return true; }
    }
    private sealed class CancelHandle() : SafeHandleZeroOrMinusOneIsInvalid(true)
    {
        protected override bool ReleaseHandle() { FreeCancel(handle); return true; }
    }

    [DllImport(Library, EntryPoint = "qb_scanner_new")] private static extern ScannerHandle CreateScanner();
    [DllImport(Library, EntryPoint = "qb_scanner_free")] private static extern void FreeScanner(IntPtr scanner);
    [DllImport(Library, EntryPoint = "qb_scanner_release_cache")] private static extern void Release(ScannerHandle scanner);
    [DllImport(Library, EntryPoint = "qb_cancel_new")] private static extern CancelHandle CreateCancel();
    [DllImport(Library, EntryPoint = "qb_cancel_free")] private static extern void FreeCancel(IntPtr cancel);
    [DllImport(Library, EntryPoint = "qb_cancel_set")] private static extern void Cancel(CancelHandle cancel);
    [DllImport(Library, EntryPoint = "qb_string_free")] private static extern void FreeString(IntPtr value);
    [DllImport(Library, EntryPoint = "qb_scanner_scan")]
    private static extern int ScanNative(ScannerHandle handle, CancelHandle cancel,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string request, out IntPtr result);

    public void ReleaseCache() => Release(handle);
    public void Dispose() => handle.Dispose();

    public LocalUsageReport Scan(IEnumerable<string> codexRoots, IEnumerable<string> claudeRoots, CancellationToken token,
        DateTimeOffset? now = null, TimeZoneInfo? zone = null, int historyDays = 182)
    {
        token.ThrowIfCancellationRequested();
        var clock = now ?? DateTimeOffset.UtcNow;
        string? timeZone = null;
        int? offsetSeconds = null;
        if (zone != null && !zone.Equals(TimeZoneInfo.Local))
        {
            if (!zone.SupportsDaylightSavingTime) offsetSeconds = (int)zone.BaseUtcOffset.TotalSeconds;
            else if (zone.HasIanaId) timeZone = zone.Id;
            else if (!TimeZoneInfo.TryConvertWindowsIdToIanaId(zone.Id, out timeZone))
                throw new ArgumentException("Unknown time zone.", nameof(zone));
        }
        string request = Encoding.UTF8.GetString(Json.Bytes(new { codexRoots, claudeRoots, historyDays, timeZone, offsetSeconds,
            nowMicros = (clock.UtcTicks - DateTimeOffset.UnixEpoch.UtcTicks) / 10 }));
        using var cancel = CreateCancel();
        using var registration = token.Register(() => Cancel(cancel));
        int status = ScanNative(handle, cancel, request, out var json);
        if (status == 1) throw new OperationCanceledException(token);
        if (status != 0 || json == IntPtr.Zero) throw new IOException("사용량 기록을 읽을 수 없습니다.");
        try
        {
            token.ThrowIfCancellationRequested();
            return Json.Read<LocalUsageReport>(Encoding.UTF8.GetBytes(Marshal.PtrToStringUTF8(json)!)) with { ScannedAt = clock };
        }
        finally { FreeString(json); }
    }
}
