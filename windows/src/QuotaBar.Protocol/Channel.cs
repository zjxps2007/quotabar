using System.Buffers.Binary;
using System.Text.Json;
using QuotaBar.Core;

namespace QuotaBar.Protocol;

public sealed record Message(int Version, string Type, Guid? RequestId, JsonElement Payload)
{
    public static Message Create<T>(string type, T payload, Guid? id = null) => new(1, type, id, Json.Element(payload));
    public T Body<T>() => Payload.Deserialize<T>(Json.Options) ?? throw new InvalidDataException("Invalid payload");
}
public sealed class Channel(Stream input, Stream output) : IDisposable
{
    public const int MaximumSize = 4 * 1024 * 1024;
    private readonly SemaphoreSlim writer = new(1);
    private readonly CancellationTokenSource closed = new();
    public async Task SendAsync(Message message)
    {
        var data = Json.Bytes(message);
        if (data.Length > MaximumSize) throw new InvalidDataException("Message exceeds limit");
        await writer.WaitAsync(closed.Token);
        try
        {
            var header = new byte[4]; BinaryPrimitives.WriteInt32BigEndian(header, data.Length);
            await output.WriteAsync(header, closed.Token);
            await output.WriteAsync(data, closed.Token);
            await output.FlushAsync(closed.Token);
        }
        finally { writer.Release(); }
    }
    public async Task ReadAsync(Func<Message, Task> receive, CancellationToken cancellation = default)
    {
        using var cts = CancellationTokenSource.CreateLinkedTokenSource(closed.Token, cancellation);
        var header = new byte[4];
        while (true)
        {
            int first = await input.ReadAsync(header.AsMemory(0, 1), cts.Token);
            if (first == 0) return;
            await input.ReadExactlyAsync(header.AsMemory(1), cts.Token);
            int size = BinaryPrimitives.ReadInt32BigEndian(header);
            if (size <= 0 || size > MaximumSize) throw new InvalidDataException("Invalid frame length");
            var data = new byte[size]; await input.ReadExactlyAsync(data, cts.Token);
            var message = Json.Read<Message>(data);
            if (message.Version != 1) throw new InvalidDataException("Unsupported protocol version");
            await receive(message);
        }
    }
    public void Dispose() { if (closed.IsCancellationRequested) return; closed.Cancel(); output.Dispose(); input.Dispose(); }
}
