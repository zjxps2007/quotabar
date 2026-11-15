using System.Diagnostics;
using System.Net;
using System.Net.Security;
using System.Net.Sockets;
using System.Runtime.Loader;
using System.Security.Authentication;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using System.Text;
using QuotaBar.Core;

internal static class EnvironmentProxyCompatibility
{
    // Each client gets a fresh process because HttpClient.DefaultProxy is cached.
    public static async Task Compare(string referenceDirectory)
    {
        foreach (string operation in new[] { "FetchCodex", "Refresh" })
        foreach (string protocol in new[] { "http", "https", "socks5" })
        {
            string target = operation == "Refresh" ? "auth.openai.com" : "chatgpt.com";
            foreach (string assembly in new[] { Path.GetFullPath(referenceDirectory), "current" })
            {
                using var listener = new TcpListener(IPAddress.Loopback, 0);
                listener.Start();
                string proxy = $"{protocol}://fixture:p%40ss@127.0.0.1:{((IPEndPoint)listener.LocalEndpoint).Port}";
                var start = new ProcessStartInfo(Environment.ProcessPath!) {
                    UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true
                };
                start.ArgumentList.Add("--provider-proxy-child");
                start.ArgumentList.Add(assembly);
                start.ArgumentList.Add(operation);
                start.Environment.Remove("NO_PROXY");
                start.Environment.Remove("GATEWAY_INTERFACE");
                start.Environment["HTTP_PROXY"] = proxy;
                // Exercise ALL_PROXY fallback as well as the protocol-specific setting.
                start.Environment["HTTPS_PROXY"] = protocol == "socks5" ? "invalid:port" : proxy;
                start.Environment["ALL_PROXY"] = proxy;
                using var child = Process.Start(start)!;
                using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(10));
                try
                {
                    if (protocol == "socks5") await RejectSocksTunnel(listener, target, deadline.Token);
                    else if (protocol == "https") await UntrustedTlsProxy(listener, deadline.Token);
                    else await RejectHttpTunnel(listener, target, deadline.Token);
                    await child.WaitForExitAsync(deadline.Token);
                    Suite.Check(child.ExitCode == 0, await child.StandardError.ReadToEndAsync());
                }
                finally { if (!child.HasExited) child.Kill(entireProcessTree: true); }
            }
            Console.WriteLine($"PASS original and Rust {operation} via {protocol} proxy");
        }
    }

    private static async Task UntrustedTlsProxy(TcpListener listener, CancellationToken token)
    {
        // The fixture certificate stays in memory and has no trusted root.
        using var key = RSA.Create(2048);
        var request = new CertificateRequest("CN=127.0.0.1", key, HashAlgorithmName.SHA256, RSASignaturePadding.Pkcs1);
        var names = new SubjectAlternativeNameBuilder();
        names.AddIpAddress(IPAddress.Loopback);
        request.CertificateExtensions.Add(names.Build());
        using var cert = request.CreateSelfSigned(DateTimeOffset.UtcNow.AddMinutes(-1), DateTimeOffset.UtcNow.AddHours(1));
        using var connection = await listener.AcceptTcpClientAsync(token);
        var header = new byte[1];
        Suite.Check(await connection.Client.ReceiveAsync(header, SocketFlags.Peek, token) == 1 && header[0] == 0x16,
            "Expected TLS before any proxy headers");
        using var tls = new SslStream(connection.GetStream());
        try {
            await tls.AuthenticateAsServerAsync(new SslServerAuthenticationOptions { ServerCertificate = cert }, token);
            Suite.Check(await tls.ReadAsync(header, token) == 0, "Client sent an HTTP request through an untrusted TLS proxy");
        }
        catch (Exception e) when (e is AuthenticationException or IOException) { }
    }

    private static async Task RejectHttpTunnel(TcpListener listener, string host, CancellationToken token)
    {
        for (int step = 0; step < 2; step++)
        {
            using var connection = await listener.AcceptTcpClientAsync(token);
            var stream = connection.GetStream();
            using var reader = new StreamReader(stream, Encoding.ASCII, leaveOpen: true);
            Suite.Check(await reader.ReadLineAsync(token) == $"CONNECT {host}:443 HTTP/1.1");
            var headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            while (await reader.ReadLineAsync(token) is { Length: > 0 } line) {
                var parts = line.Split(':', 2);
                headers.Add(parts[0], parts[1].Trim());
            }
            Suite.Check(headers.GetValueOrDefault("Host") == $"{host}:443");
            Suite.Check(!headers.ContainsKey("Authorization") && !headers.ContainsKey("ChatGPT-Account-Id"));
            string? expected = step == 0 ? null : "Basic " + Convert.ToBase64String("fixture:p@ss"u8);
            Suite.Check(headers.GetValueOrDefault("Proxy-Authorization") == expected);
            string status = step == 0 ? "407 Authentication Required\r\nProxy-Authenticate: Basic realm=\"fixture\"" : "502 Bad Gateway";
            await stream.WriteAsync(Encoding.ASCII.GetBytes($"HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"), token);
        }
    }

    private static async Task RejectSocksTunnel(TcpListener listener, string host, CancellationToken token)
    {
        using var connection = await listener.AcceptTcpClientAsync(token);
        var stream = connection.GetStream();
        async Task<byte[]> Read(int count) {
            byte[] bytes = new byte[count];
            await stream.ReadExactlyAsync(bytes, token);
            return bytes;
        }
        async Task<string> ReadString() => Encoding.UTF8.GetString(await Read((await Read(1))[0]));
        var hello = await Read(2);
        Suite.Check(hello[0] == 5 && (await Read(hello[1])).Order().SequenceEqual(new byte[] { 0, 2 }));
        await stream.WriteAsync(new byte[] { 5, 2 }, token);
        Suite.Check((await Read(1))[0] == 1 && await ReadString() == "fixture" && await ReadString() == "p@ss");
        await stream.WriteAsync(new byte[] { 1, 0 }, token);
        Suite.Check((await Read(4)).SequenceEqual(new byte[] { 5, 1, 0, 3 }) && await ReadString() == host);
        Suite.Check((await Read(2)).SequenceEqual(new byte[] { 1, 187 }));
        await stream.WriteAsync(new byte[] { 5, 5, 0, 1, 127, 0, 0, 1, 0, 0 }, token);
    }

    public static async Task<int> Child(string directory, string operation)
    {
        var reference = new AssemblyLoadContext("original environment proxy", isCollectible: true);
        try
        {
            Type type = directory == "current" ? typeof(ProviderClient) :
                reference.LoadFromAssemblyPath(Path.Combine(directory, "QuotaBar.Core.dll")).GetType("QuotaBar.Core.ProviderClient", true)!;
            using var client = (IDisposable)Activator.CreateInstance(type, new object?[] { null })!;
            object[] args = operation == "Refresh" ? [Fixtures.Auth()] : [Fixtures.Auth(), CancellationToken.None];
            await (Task)type.GetMethod(operation)!.Invoke(client, args)!;
            return 1;
        }
        catch (HttpRequestException) { return 0; }
        finally { reference.Unload(); }
    }
}
