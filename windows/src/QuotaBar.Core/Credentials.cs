using System.Text.Json;
using System.Runtime.InteropServices;
using System.Security.Cryptography;

namespace QuotaBar.Core;
public sealed record Credentials(string Identity, string Label, string AccessToken, string? RefreshToken,
    string? AccountId, string? UserId, string? Email, DateTimeOffset? ExpiresAt, DateTimeOffset? LastRefresh)
{
    public bool NeedsRefresh(DateTimeOffset now) => ExpiresAt is { } expiry ? expiry <= now.AddMinutes(5) : LastRefresh == null || LastRefresh <= now.AddHours(-1);
    public static JsonElement Claims(string? token)
    {
        try { var value = token!.Split('.')[1].Replace('-', '+').Replace('_', '/'); return Json.Read<JsonElement>(Convert.FromBase64String(value.PadRight((value.Length + 3) / 4 * 4, '='))); }
        catch (Exception e) when (e is NullReferenceException or IndexOutOfRangeException or FormatException or JsonException) { return default; }
    }
    [DllImport("quotabar_native", EntryPoint = "qb_credentials")]
    private static extern int ParseNative(byte[] data, nuint length, int provider, int mode, out IntPtr output, out nuint count);
    [DllImport("quotabar_native", EntryPoint = "qb_rotate_credentials")]
    private static extern int RotateNative(byte[] current, nuint currentLength, byte[] response, nuint responseLength, long nowMicros, out IntPtr output, out nuint count);
    [DllImport("quotabar_native", EntryPoint = "qb_secret_free")]
    private static extern void Release(IntPtr output, nuint count);

    internal static Exception Failure(int status) => status switch {
                -2 => new UsageException("invalid_auth", "로그인 파일이 너무 큽니다."),
                -4 => new UsageException("missing_token", "API 키 대신 구독 계정의 로그인 파일을 연결하세요."),
                -5 => new UsageException("missing_identity", "로그인 파일에서 계정과 워크스페이스를 확인할 수 없습니다."),
                -8 => new UsageException("oauth_response", "갱신 응답에 인증 정보가 없습니다."),
                -9 => new UsageException("account_mismatch", "갱신된 계정이 다릅니다. 다시 연결하세요."),
                -10 => new UsageException("account_mismatch", "계정 정체성이 변경되었습니다."),
                _ => (Exception)new JsonException("로그인 파일의 JSON 형식을 확인하세요.")
        };
    internal static byte[] TakeBytes(int status, IntPtr output, nuint count)
    {
        if (status != 0) throw Failure(status);
        try
        {
            var buffer = new byte[checked((int)count)];
            Marshal.Copy(output, buffer, 0, buffer.Length);
            return buffer;
        }
        finally { Release(output, count); }
    }
    public static Credentials Parse(byte[] data, Provider provider)
    {
        int status = ParseNative(data, (nuint)data.Length, (int)provider, 0, out var output, out var count);
        var bytes = TakeBytes(status, output, count);
        try { return Json.Read<Credentials>(bytes); }
        finally { CryptographicOperations.ZeroMemory(bytes); }
    }
    internal static byte[] Rotate(byte[] current, byte[] response, DateTimeOffset now)
    {
        var micros = (now.UtcTicks - DateTimeOffset.UnixEpoch.UtcTicks) / 10;
        int status = RotateNative(current, (nuint)current.Length, response, (nuint)response.Length, micros, out var output, out var count);
        return TakeBytes(status, output, count);
    }
}
