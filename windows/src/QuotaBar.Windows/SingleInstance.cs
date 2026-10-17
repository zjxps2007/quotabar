using System.IO.Pipes;
using System.Security.Cryptography;
using System.Text;

namespace QuotaBar.Windows;
public sealed class SingleInstance : IDisposable
{
    private readonly FileStream? lease;
    private readonly CancellationTokenSource stop = new();
    private readonly string name;
    public bool IsOwner => lease != null;
    public SingleInstance(AppPaths paths)
    {
        PrivateFiles.EnsureDirectory(paths.Root);
        name = "QuotaBar-" + Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(paths.Root.ToUpperInvariant() + PrivateFiles.User.Value)))[..24];
        try { lease = new(Path.Combine(paths.Root, "monitor.lock"), FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.None); }
        catch (IOException) { }
    }
    public async Task Activate()
    {
        using var pipe = new NamedPipeClientStream(".", name, PipeDirection.Out, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
        using var timeout = new CancellationTokenSource(3000); await pipe.ConnectAsync(timeout.Token); await pipe.WriteAsync(new byte[] { 1 }, timeout.Token);
    }
    public async Task Listen(Action activate)
    {
        while (!stop.IsCancellationRequested)
        {
            try
            {
                using var pipe = new NamedPipeServerStream(name, PipeDirection.In, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
                await pipe.WaitForConnectionAsync(stop.Token);
                using var deadline = CancellationTokenSource.CreateLinkedTokenSource(stop.Token); deadline.CancelAfter(1000);
                var data = new byte[1]; if (await pipe.ReadAsync(data, deadline.Token) == 1 && data[0] == 1) activate();
            }
            catch (Exception e) when (e is OperationCanceledException or IOException) { }
        }
    }
    public void Dispose() { stop.Cancel(); lease?.Dispose(); }
}
