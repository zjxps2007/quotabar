using System.Security.AccessControl;
using System.Security.Principal;
using QuotaBar.Core;

namespace QuotaBar.Windows;

public sealed class AppPaths(string? root = null)
{
    public string Root { get; } = Path.GetFullPath(root ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "QuotaBar"));
    public string Credentials => Path.Combine(Root, "Credentials");
    public string Profiles => Path.Combine(Root, "ClaudeProfiles");
    public string Login => Path.Combine(Root, "Login");
    public static string UserHome => Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
    public static string CodexHome => Environment.GetEnvironmentVariable("CODEX_HOME") ?? Path.Combine(UserHome, ".codex");
    public static string ClaudeHome => Environment.GetEnvironmentVariable("CLAUDE_CONFIG_DIR") ?? Path.Combine(UserHome, ".claude");
    public static string DefaultPath(Provider provider) => provider switch {
        Provider.Codex => Path.Combine(CodexHome, "auth.json"), Provider.Claude => ClaudeHome,
        _ => Path.Combine(UserHome, ".gemini", "oauth_creds.json") };
}
public static class PrivateFiles
{
    public static SecurityIdentifier User => WindowsIdentity.GetCurrent().User!;
    public static void RejectLinks(string path)
    {
        for (var p = Path.GetFullPath(path); !string.IsNullOrEmpty(p); p = Path.GetDirectoryName(p))
            if ((File.Exists(p) || Directory.Exists(p)) && (File.GetAttributes(p) & FileAttributes.ReparsePoint) != 0)
                throw new IOException("앱 데이터 경로에 링크를 사용할 수 없습니다.");
    }
    public static void EnsureDirectory(string path)
    {
        RejectLinks(path);
        var info = new DirectoryInfo(path);
        var acl = new DirectorySecurity(); acl.SetOwner(User); acl.SetAccessRuleProtection(true, false);
        acl.AddAccessRule(new FileSystemAccessRule(User, FileSystemRights.FullControl,
            InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit, PropagationFlags.None, AccessControlType.Allow));
        if (!info.Exists) info.Create(acl);
        else
        {
            if (!User.Equals(info.GetAccessControl().GetOwner(typeof(SecurityIdentifier)))) throw new IOException("앱 데이터 폴더 소유자가 다릅니다.");
            info.SetAccessControl(acl);
        }
    }
    public static byte[] ReadBounded(string path, int limit = 1_048_576)
    {
        using var file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
        return ReadBounded(file, limit);
    }
    public static byte[] ReadBounded(Stream stream, int limit)
    {
        using var result = new MemoryStream(); var buffer = new byte[8192]; int count;
        while ((count = stream.Read(buffer)) != 0)
        { if (result.Length + count > limit) throw new IOException("파일 크기 제한을 초과했습니다."); result.Write(buffer, 0, count); }
        return result.ToArray();
    }
    public static void Write(string path, byte[] data)
    {
        string folder = Path.GetDirectoryName(path)!; EnsureDirectory(folder); RejectLinks(path);
        string temp = Path.Combine(folder, "." + Guid.NewGuid() + ".tmp");
        try
        {
            // Elevated tokens can otherwise create files owned by the Administrators group.
            var acl = new FileSecurity(); acl.SetOwner(User); acl.SetAccessRuleProtection(true, false);
            acl.AddAccessRule(new FileSystemAccessRule(User, FileSystemRights.FullControl, AccessControlType.Allow));
            using (var file = new FileInfo(temp).Create(FileMode.CreateNew, FileSystemRights.Write, FileShare.None, 4096, FileOptions.None, acl))
            { file.Write(data); file.Flush(true); }
            File.Move(temp, path, true);
        }
        finally { if (File.Exists(temp)) File.Delete(temp); }
    }
}
public sealed class SettingsStore(AppPaths paths)
{
    public Settings Load()
    {
        string path = Path.Combine(paths.Root, "settings.json");
        if (!File.Exists(path)) return new();
        PrivateFiles.RejectLinks(path);
        var result = Json.Read<Settings>(PrivateFiles.ReadBounded(path, 4 * 1024 * 1024));
        if (result.SchemaVersion != 1 || result.Accounts == null || result.Accounts.Count > 50 || result.Accounts.Any(a => a == null || a.Id == Guid.Empty || !Enum.IsDefined(a.Provider) || string.IsNullOrWhiteSpace(a.Identity) || string.IsNullOrWhiteSpace(a.Name)) || result.Accounts.Select(a => a.Id).Distinct().Count() != result.Accounts.Count)
            throw new IOException("설정 형식이 지원되지 않습니다. 원본 파일을 보존했습니다.");
        return result with { RefreshSeconds = RefreshSchedule.Intervals.Contains(result.RefreshSeconds) ? result.RefreshSeconds : 300,
            Appearance = new[] { "system", "light", "dark" }.Contains(result.Appearance) ? result.Appearance : "system",
            TrayDisplayMode = new[] { "remaining", "icon" }.Contains(result.TrayDisplayMode) ? result.TrayDisplayMode : "remaining" };
    }
    public void Save(Settings settings) => PrivateFiles.Write(Path.Combine(paths.Root, "settings.json"), Json.Bytes(settings));
}
