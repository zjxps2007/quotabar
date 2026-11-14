using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text.Json;

namespace QuotaBar.Core;

internal static class NativeProviderRequest
{
    internal enum Endpoint { Codex, Refresh, GeminiAssist, GeminiProjects, GeminiQuota }
    private sealed record Description(string Url, string Method, Dictionary<string, string> Headers, JsonElement? Body);
    internal sealed record Discovery(string? Project, string? Tier);
    [DllImport("quotabar_native", EntryPoint = "qb_provider_request")]
    private static extern int Request(int mode, byte[] data, nuint length, out IntPtr output, out nuint count);
    [DllImport("quotabar_native", EntryPoint = "qb_secret_free")]
    private static extern void Release(IntPtr output, nuint count);
    private static T Decode<T>(byte[] input, int mode)
    {
        try
        {
            int status = Request(mode, input, (nuint)input.Length, out var native, out var count);
            if (status != 0) throw new JsonException("공급자 응답 형식을 확인하세요.");
            byte[]? bytes = null;
            try
            {
                bytes = new byte[checked((int)count)];
                Marshal.Copy(native, bytes, 0, bytes.Length);
                return Json.Read<T>(bytes);
            }
            finally
            {
                if (bytes != null) CryptographicOperations.ZeroMemory(bytes);
                Release(native, count);
            }
        }
        finally { CryptographicOperations.ZeroMemory(input); }
    }
    internal static HttpRequestMessage Build(Endpoint endpoint, string token, string? account = null, string? project = null)
    {
        var description = Decode<Description>(Json.Bytes(new { endpoint, token, account, project, version = Brand.Version, timeout = 20 }), 0);
        var request = new HttpRequestMessage(new HttpMethod(description.Method), description.Url);
        try
        {
            if (description.Body is JsonElement body)
                request.Content = new ByteArrayContent(Json.Bytes(body));
            foreach (var (name, value) in description.Headers)
                if (name == "Content-Type") request.Content!.Headers.Add(name, value);
                else request.Headers.Add(name, value);
            return request;
        }
        catch { request.Dispose(); throw; }
    }
    internal static Discovery Discover(byte[] bytes, bool list) => Decode<Discovery>(bytes, list ? 2 : 1);
}
