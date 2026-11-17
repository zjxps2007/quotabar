using System.Runtime.InteropServices;
using QuotaBar.Core;

namespace QuotaBar.Windows;

public interface ILaunchAtLogin
{
    LaunchAtLoginState State { get; }
    void SetEnabled(bool enabled);
}

// HKCU Run is per-user and persists across reboots without administrator rights.
// Read only at startup; registration changes require an explicit settings action.
public sealed class LaunchAtLogin(string? executable = null,
    string registryPath = @"Software\Microsoft\Windows\CurrentVersion\Run") : ILaunchAtLogin
{
    public const string ValueName = "QuotaBar";
    private readonly string executable = executable ?? Environment.ProcessPath ?? "";
    [DllImport("quotabar_native", EntryPoint = "qb_windows_login_item")]
    private static extern int LoginItem([MarshalAs(UnmanagedType.LPUTF8Str)] string executable,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string registryKey, int operation);

    public LaunchAtLoginState State => LoginItem(executable, registryPath, 0) switch
    {
        0 => LaunchAtLoginState.Disabled, 1 => LaunchAtLoginState.Enabled, 3 => LaunchAtLoginState.Unavailable,
        _ => throw new IOException("자동 실행 설정을 확인할 수 없습니다.")
    };
    public void SetEnabled(bool enabled)
    {
        int status = LoginItem(executable, registryPath, enabled ? 1 : 2);
        if (status == -2) throw new UsageException("startup_unavailable", "설치된 QuotaBar 앱에서 자동 실행을 설정하세요. 실행 파일 경로가 너무 길면 더 짧은 경로를 사용하세요.");
        if (status < 0) throw new IOException("자동 실행 설정을 변경할 수 없습니다.");
    }
}
