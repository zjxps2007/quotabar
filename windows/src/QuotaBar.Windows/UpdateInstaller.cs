using System.Diagnostics;
using System.IO.Compression;
using System.Runtime.InteropServices;
using Microsoft.Win32;
using QuotaBar.Core;

namespace QuotaBar.Windows;

public sealed record UpdateInstallResult(string Version, bool Success, string Message);
public sealed record WindowsUpdatePlan(StagedUpdate Staged, string Target, string DataRoot, string CurrentVersion,
    bool Installer, int ParentPid, long ParentStarted);

public static class UpdateInstaller
{
    private const string UninstallKey = @"Software\Microsoft\Windows\CurrentVersion\Uninstall\QuotaBar.Windows_is1";
    public static string UpdatesRoot(AppPaths paths) => Path.Combine(paths.Root, "Updates");
    public static string ResultPath(AppPaths paths) => Path.Combine(UpdatesRoot(paths), "last-result.json");
    public static string PublicKey
    {
        get { using var stream = typeof(UpdateSignature).Assembly.GetManifestResourceStream("QuotaBar.UpdatePublicKey")
            ?? throw new IOException("Update public key missing."); using var reader = new StreamReader(stream); return reader.ReadToEnd(); }
    }
    public static bool Installed(string target)
    {
        using var key = Registry.CurrentUser.OpenSubKey(UninstallKey);
        return key?.GetValue("InstallLocation") is string location && SamePath(location, target);
    }
    private static bool SamePath(string a, string b) => string.Equals(Path.TrimEndingDirectorySeparator(Path.GetFullPath(a)),
        Path.TrimEndingDirectorySeparator(Path.GetFullPath(b)), StringComparison.OrdinalIgnoreCase);

    public static async Task<string> Prepare(StagedUpdate staged, AppPaths paths, string target, string currentVersion, CancellationToken token)
    {
        target = Path.TrimEndingDirectorySeparator(Path.GetFullPath(target));
        if (!SamePath(Path.GetDirectoryName(staged.Directory)!, UpdatesRoot(paths)) || !Guid.TryParse(Path.GetFileName(staged.Directory), out var transaction)
            || SamePath(target, paths.Root) || Path.GetFullPath(paths.Root).StartsWith(target + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))
            throw new IOException("Invalid update location.");
        PrivateFiles.RejectLinks(target); PrivateFiles.EnsureDirectory(staged.Directory);
        using var parent = Process.GetCurrentProcess();
        var plan = new WindowsUpdatePlan(staged, target, paths.Root, currentVersion, Installed(target), parent.Id, parent.StartTime.ToUniversalTime().Ticks);
        await Verify(plan, token);
        var replacement = new UpdateReplacement(target, transaction);
        if (Directory.Exists(replacement.Incoming) || Directory.Exists(replacement.Backup)) throw new IOException("Update transaction already exists.");
        try
        {
            if (!plan.Installer)
            {
                CopyTree(target, replacement.Incoming, token);
                OverlayZip(staged.Archive, replacement.Incoming, token);
                ValidateVersion(replacement.Incoming, staged.Update.Release.Version);
            }
            // Run from a private copy; the installed StatusLine component may be replaced.
            CopyTree(Path.Combine(target, "StatusLine"), Path.Combine(staged.Directory, "Helper"), token);
            string job = Path.Combine(staged.Directory, "install.json");
            PrivateFiles.Write(job, Json.Bytes(plan)); return job;
        }
        catch { if (Directory.Exists(replacement.Incoming)) Directory.Delete(replacement.Incoming, true); throw; }
    }

    public static Process LaunchHelper(string job)
    {
        string directory = Path.GetDirectoryName(job)!;
        File.Delete(Path.Combine(directory, "installer-ready.json"));
        var info = new ProcessStartInfo(Path.Combine(directory, "Helper", "QuotaBar.StatusLine.exe"))
        { UseShellExecute = false, CreateNoWindow = true, WorkingDirectory = directory };
        info.ArgumentList.Add("--apply-update"); info.ArgumentList.Add(job);
        return Process.Start(info) ?? throw new IOException("Update helper did not start.");
    }

    public static void Acknowledge(string[] args, AppPaths paths)
    {
        if (!Guid.TryParse(Arguments.Value(args, "--update-ack"), out var transaction)) return;
        string directory = Path.Combine(UpdatesRoot(paths), transaction.ToString("D"));
        try
        {
            PrivateFiles.RejectLinks(directory);
            var plan = Json.Read<WindowsUpdatePlan>(PrivateFiles.ReadBounded(Path.Combine(directory, "install.json"), 64 * 1024));
            if (!SamePath(plan.Target, AppContext.BaseDirectory) || !SamePath(plan.DataRoot, paths.Root) || plan.Staged.Update.Release.Version != Brand.Version) return;
            WriteResult(Path.Combine(directory, "started.json"), plan, true, "업데이트를 설치했습니다.");
        }
        catch (Exception e) when (e is IOException or System.Text.Json.JsonException or UnauthorizedAccessException or UsageException) { }
    }
    public static UpdateInstallResult? ReadResult(string path)
    {
        try { PrivateFiles.RejectLinks(path); return Json.Read<UpdateInstallResult>(PrivateFiles.ReadBounded(path, 4096)); }
        catch (Exception e) when (e is IOException or System.Text.Json.JsonException or UnauthorizedAccessException or UsageException) { return null; }
    }
    public static void CleanCompleted(AppPaths paths)
    {
        string root = UpdatesRoot(paths); if (!Directory.Exists(root)) return;
        foreach (string directory in Directory.EnumerateDirectories(root))
        {
            if (!Guid.TryParse(Path.GetFileName(directory), out _) || ReadResult(Path.Combine(directory, "completed.json")) is not { Success: true }) continue;
            try { PrivateFiles.RejectLinks(directory); Directory.Delete(directory, true); } catch (IOException) { } catch (UnauthorizedAccessException) { }
        }
    }
    private static void WriteResult(string path, WindowsUpdatePlan plan, bool success, string message) =>
        PrivateFiles.Write(path, Json.Bytes(new UpdateInstallResult(plan.Staged.Update.Release.Version, success, message)));
    private static async Task Verify(WindowsUpdatePlan plan, CancellationToken token)
    {
        var staged = plan.Staged;
        var verified = UpdateSignature.Verify(PrivateFiles.ReadBounded(Path.Combine(staged.Directory, "update-manifest.json"), 128 * 1024),
            PrivateFiles.ReadBounded(Path.Combine(staged.Directory, "update-manifest.sig"), 128), PublicKey, staged.Update.Release, plan.CurrentVersion);
        string expected = $"QuotaBar-{verified.Release.Version}-Windows-x64" + (plan.Installer ? "-Setup.exe" : ".zip");
        if (verified != staged.Update || verified.Asset.Name != expected) throw new IOException("Update platform mismatch.");
        PrivateFiles.RejectLinks(staged.Archive); await UpdateSignature.VerifyFile(staged.Archive, verified.Asset, token);
    }
    [DllImport("quotabar_native", EntryPoint = "qb_release_windows_components")]
    private static extern int ValidateComponents([MarshalAs(UnmanagedType.LPUTF8Str)] string root, [MarshalAs(UnmanagedType.LPUTF8Str)] string version);
    private static void ValidateVersion(string root, string version)
    {
        if (root.Contains('\0') || version.Contains('\0') || ValidateComponents(root, version) != 0)
            throw new IOException("Update package version mismatch.");
    }

    public static void CopyTree(string source, string destination, CancellationToken token)
    {
        PrivateFiles.RejectLinks(source); PrivateFiles.RejectLinks(destination);
        if (Directory.Exists(destination)) throw new IOException("Copy destination already exists.");
        Directory.CreateDirectory(destination);
        foreach (string path in Directory.EnumerateFileSystemEntries(source))
        {
            token.ThrowIfCancellationRequested(); PrivateFiles.RejectLinks(path);
            string target = Path.Combine(destination, Path.GetFileName(path));
            if (Directory.Exists(path)) CopyTree(path, target, token); else File.Copy(path, target);
        }
    }
    public static void OverlayZip(string archive, string destination, CancellationToken token)
    {
        using var zip = ZipFile.OpenRead(archive); long total = 0;
        if (zip.Entries.Count > 10000) throw new IOException("Update archive has too many files.");
        string root = Path.GetFullPath(destination) + Path.DirectorySeparatorChar;
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var entry in zip.Entries)
        {
            token.ThrowIfCancellationRequested(); string name = entry.FullName.Replace('\\', '/');
            const string prefix = "QuotaBar-Windows-x64/";
            if (!name.StartsWith(prefix, StringComparison.Ordinal) || name.Contains(':') || (entry.ExternalAttributes >> 16 & 0xF000) == 0xA000)
                throw new IOException("Unsafe update archive entry.");
            string relative = name[prefix.Length..]; if (relative.Length == 0) continue;
            if (relative.Split('/').Any(p => p is "." or ".." || p.EndsWith('.') || p.EndsWith(' '))) throw new IOException("Unsafe update path.");
            string target = Path.GetFullPath(Path.Combine(destination, relative.Replace('/', Path.DirectorySeparatorChar)));
            if (!target.StartsWith(root, StringComparison.OrdinalIgnoreCase) || !seen.Add(target)) throw new IOException("Duplicate or escaping update entry.");
            PrivateFiles.RejectLinks(target);
            total = checked(total + entry.Length); if (total > 2L * 1024 * 1024 * 1024) throw new IOException("Update archive is too large.");
            if (name.EndsWith('/')) Directory.CreateDirectory(target);
            else { Directory.CreateDirectory(Path.GetDirectoryName(target)!); entry.ExtractToFile(target, true); }
        }
    }
}
