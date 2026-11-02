using Microsoft.Win32;
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
    private string Command => $"\"{executable}\" --background";
    private bool Available => Path.IsPathFullyQualified(executable) && !executable.Contains('"')
        && Path.GetFileName(executable).Equals("QuotaBar.Monitor.exe", StringComparison.OrdinalIgnoreCase)
        && File.Exists(executable) && Command.Length <= 260;

    public LaunchAtLoginState State
    {
        get
        {
            if (!Available) return LaunchAtLoginState.Unavailable;
            using var key = Registry.CurrentUser.OpenSubKey(registryPath);
            return string.Equals(key?.GetValue(ValueName) as string, Command, StringComparison.OrdinalIgnoreCase)
                ? LaunchAtLoginState.Enabled : LaunchAtLoginState.Disabled;
        }
    }

    public void SetEnabled(bool enabled)
    {
        if (enabled)
        {
            if (!Available) throw new UsageException("startup_unavailable", "설치된 QuotaBar 앱에서 자동 실행을 설정하세요. 실행 파일 경로가 너무 길면 더 짧은 경로를 사용하세요.");
            using var key = Registry.CurrentUser.CreateSubKey(registryPath, writable: true);
            key.SetValue(ValueName, Command, RegistryValueKind.String);
        }
        else
        {
            using var key = Registry.CurrentUser.OpenSubKey(registryPath, writable: true);
            key?.DeleteValue(ValueName, throwOnMissingValue: false);
        }
    }
}
