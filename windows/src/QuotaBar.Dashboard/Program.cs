using System.IO;
using System.IO.Pipes;
using System.Windows;
using QuotaBar.Protocol;
using Channel = QuotaBar.Protocol.Channel;

namespace QuotaBar.Dashboard;
internal static class Program
{
    [STAThread] static void Main(string[] args)
    {
        string? Value(string name) { var i = Array.IndexOf(args, name); return i >= 0 && i + 1 < args.Length ? args[i + 1] : null; }
        if (Value("--in") is not { } input || Value("--out") is not { } output) return;
        try
        {
            using var channel = new Channel(new AnonymousPipeClientStream(PipeDirection.In, input), new AnonymousPipeClientStream(PipeDirection.Out, output));
            var app = new Application { ShutdownMode = ShutdownMode.OnExplicitShutdown };
            var window = new DashboardWindow(channel);
            window.Closed += (_, _) => app.Shutdown();
            _ = Task.Run(async () =>
            {
                try
                {
                    await channel.SendAsync(Message.Create("ready", true));
                    await channel.ReadAsync(m => app.Dispatcher.InvokeAsync(() => window.Receive(m)).Task);
                }
                catch (Exception e) when (e is IOException or OperationCanceledException or ObjectDisposedException or System.Text.Json.JsonException) { }
                finally { if (!app.Dispatcher.HasShutdownStarted) _ = app.Dispatcher.BeginInvoke(() => app.Shutdown()); }
            });
            app.Run();
        }
        catch (Exception e) when (e is IOException or ArgumentException) { }
    }
}
