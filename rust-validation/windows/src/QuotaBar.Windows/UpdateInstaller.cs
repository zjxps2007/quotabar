using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
using QuotaBar.Core;

namespace QuotaBar.Windows;

public sealed record UpdateInstallResult(string Version, bool Success, string Message);
public sealed record WindowsUpdatePlan(StagedUpdate Staged, string Target, string DataRoot, string CurrentVersion,
    bool Installer, int ParentPid, long ParentStarted);

public static class UpdateInstaller
{
    private const string Library = "quotabar_native";
    public static string UpdatesRoot(AppPaths paths) => Path.Combine(paths.Root, "Updates");
    public static string ResultPath(AppPaths paths) => Path.Combine(UpdatesRoot(paths), "last-result.json");
    public static string PublicKey
    {
        get { using var stream = typeof(UpdateSignature).Assembly.GetManifestResourceStream("QuotaBar.UpdatePublicKey")
            ?? throw new IOException("Update public key missing."); using var reader = new StreamReader(stream); return reader.ReadToEnd(); }
    }

    [DllImport(Library, EntryPoint = "qb_windows_update_installed")]
    private static extern int IsInstalled([MarshalAs(UnmanagedType.LPUTF8Str)] string target);
    public static bool Installed(string target) => IsInstalled(Path.GetFullPath(target)) switch
    {
        0 => false, 1 => true, _ => throw new IOException("Cannot read Windows installation registration.")
    };

    [DllImport(Library, EntryPoint = "qb_windows_update_prepare")]
    private static extern int PrepareJob(byte[] request, nuint length, NativeCancellation cancel);
    public static async Task<string> Prepare(StagedUpdate staged, AppPaths paths, string target, string currentVersion, CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
        staged = staged with { Directory = Path.GetFullPath(staged.Directory) };
        var request = Json.Bytes(new { staged, target = Path.GetFullPath(target), dataRoot = Path.GetFullPath(paths.Root), currentVersion });
        using var cancel = NativeCancellation.Create();
        using var registration = token.Register(cancel.Cancel);
        int status = await Task.Run(() => PrepareJob(request, (nuint)request.Length, cancel), token).ConfigureAwait(false);
        if (status == 2) throw new UsageException("update_signature", "업데이트 파일의 서명을 확인하지 못했습니다. 현재 버전을 유지합니다.");
        Check(status, token, "Could not prepare the signed Windows update.");
        return Path.Combine(staged.Directory, "install.json");
    }

    [DllImport(Library, EntryPoint = "qb_windows_update_copy")]
    private static extern int Copy([MarshalAs(UnmanagedType.LPUTF8Str)] string source, [MarshalAs(UnmanagedType.LPUTF8Str)] string dest, NativeCancellation cancel);
    [DllImport(Library, EntryPoint = "qb_windows_update_overlay")]
    private static extern int Overlay([MarshalAs(UnmanagedType.LPUTF8Str)] string source, [MarshalAs(UnmanagedType.LPUTF8Str)] string dest, NativeCancellation cancel);

    public static void CopyTree(string source, string destination, CancellationToken token) =>
        Transfer(Copy, source, destination, token);
    public static void OverlayZip(string archive, string destination, CancellationToken token) =>
        Transfer(Overlay, archive, destination, token);

    private static void Transfer(Func<string, string, NativeCancellation, int> transfer, string source, string destination, CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
        using var cancel = NativeCancellation.Create();
        using var registration = token.Register(cancel.Cancel);
        Check(transfer(Path.GetFullPath(source), Path.GetFullPath(destination), cancel), token, "Invalid Windows update package or destination.");
    }
    private static void Check(int status, CancellationToken token, string message)
    {
        if (status == 0) return;
        if (status == 1) throw new OperationCanceledException(token);
        if (status > 10) Marshal.ThrowExceptionForHR(unchecked((int)0x80070000) | (status - 10));
        throw new IOException(message);
    }

    public sealed class Helper : SafeHandleZeroOrMinusOneIsInvalid
    {
        private Helper() : base(true) { }
        [DllImport(Library, EntryPoint = "qb_windows_update_launch")]
        private static extern Helper Launch([MarshalAs(UnmanagedType.LPUTF8Str)] string job, out int error);
        [DllImport(Library, EntryPoint = "qb_windows_update_ready")]
        private static extern int Ready(Helper helper);
        [DllImport(Library, EntryPoint = "qb_windows_update_pid")]
        private static extern int Pid(Helper helper);
        [DllImport(Library, EntryPoint = "qb_windows_update_detach")]
        private static extern void DetachProcess(Helper helper);
        [DllImport(Library, EntryPoint = "qb_windows_update_free")]
        private static extern void Free(IntPtr helper);

        public static Helper Start(string job)
        {
            var helper = Launch(Path.GetFullPath(job), out int error);
            if (helper.IsInvalid)
            {
                helper.Dispose();
                Check(error, CancellationToken.None, "Update helper did not start.");
                throw new IOException("Invalid update helper handle.");
            }
            return helper;
        }
        public int ProcessId => Pid(this);
        public bool IsReady() => Ready(this) switch
        {
            0 => false, 1 => true, _ => throw new IOException("Update helper exited.")
        };
        public void Detach() => DetachProcess(this);
        protected override bool ReleaseHandle() { Free(handle); return true; }
    }

    public static Process LaunchHelper(string job)
    {
        using var helper = Helper.Start(job);
        var process = Process.GetProcessById(helper.ProcessId);
        helper.Detach();
        return process;
    }

    [DllImport(Library, EntryPoint = "qb_windows_update_ack")]
    private static extern int Ack([MarshalAs(UnmanagedType.LPUTF8Str)] string root, [MarshalAs(UnmanagedType.LPUTF8Str)] string target,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string transaction, [MarshalAs(UnmanagedType.LPUTF8Str)] string version);
    public static void Acknowledge(string[] args, AppPaths paths)
    {
        if (Guid.TryParse(Arguments.Value(args, "--update-ack"), out var transaction))
            _ = Ack(paths.Root, AppContext.BaseDirectory, transaction.ToString("D"), Brand.Version);
    }

    public static UpdateInstallResult? ReadResult(string path)
    {
        try { PrivateFiles.RejectLinks(path); return Json.Read<UpdateInstallResult>(PrivateFiles.ReadBounded(path, 4096)); }
        catch (Exception e) when (e is IOException or System.Text.Json.JsonException or UnauthorizedAccessException or UsageException) { return null; }
    }

    [DllImport(Library, EntryPoint = "qb_windows_update_clean")]
    private static extern int Clean([MarshalAs(UnmanagedType.LPUTF8Str)] string root);
    public static void CleanCompleted(AppPaths paths) => Check(Clean(paths.Root), CancellationToken.None, "Could not clean completed updates.");

    [DllImport(Library, EntryPoint = "qb_windows_update_discard")]
    private static extern int DiscardStage([MarshalAs(UnmanagedType.LPUTF8Str)] string root, [MarshalAs(UnmanagedType.LPUTF8Str)] string target,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string stage);
    public static void Discard(AppPaths paths, string target, string stage) =>
        Check(DiscardStage(paths.Root, Path.GetFullPath(target), Path.GetFullPath(stage)), CancellationToken.None, "Could not discard the prepared update.");
}
