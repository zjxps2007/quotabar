using System.Net;
using QuotaBar.Core;
using QuotaBar.Windows;

namespace QuotaBar.Monitor;

// Isolated regression probe: no real settings, credentials, downloads or installations.
internal static class UpdateManagerProbe
{
    public static void Start(MonitorLoop dispatcher, string resultPath)
    {
        dispatcher.TryPost(async () =>
        {
            string root = Path.Combine(Path.GetTempPath(), "QuotaBar.Update.Manager-" + Guid.NewGuid());
            try
            {
                using var handler = new FixtureHandler();
                AppUpdateManager? manager = null;
                manager = new(dispatcher, new AppPaths(root), () => manager!.Configure(false, false, false),
                    () => throw new InvalidOperationException("Must not install"), () => { },
                    () => throw new InvalidOperationException("Must not exit"), new ReleaseClient(handler));
                using (manager)
                {
                    manager.Configure(false, false, false);
                    manager.Check();
                    var deadline = DateTime.UtcNow.AddSeconds(5);
                    while (manager.State.Phase is UpdatePhase.Idle or UpdatePhase.Checking)
                    { if (DateTime.UtcNow > deadline) throw new TimeoutException(); await Task.Delay(25); }
                    if (manager.State.Phase != UpdatePhase.Available || handler.Requests != 1)
                        throw new InvalidOperationException("Manual check was cancelled while automatic updates were disabled.");
                    manager.Configure(false, false, false);
                    if (manager.State.Phase != UpdatePhase.Available) throw new InvalidOperationException("Display updates changed manual mode.");
                }
                File.WriteAllBytes(resultPath, Json.Bytes(new { passed = true, monitorPid = Environment.ProcessId, manualCheckWhileAutomaticUpdatesDisabled = true }));
            }
            catch (Exception error) { File.WriteAllBytes(resultPath, Json.Bytes(new { passed = false, error = error.ToString() })); }
            finally { if (Directory.Exists(root)) Directory.Delete(root, true); dispatcher.Exit(); }
        });
    }
    private sealed class FixtureHandler : HttpMessageHandler
    {
        public int Requests;
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token)
        {
            token.ThrowIfCancellationRequested(); Requests++;
            var response = new HttpResponseMessage(HttpStatusCode.OK) { Content = new ByteArrayContent(Json.Bytes(new {
                tag_name = "v2147483647.0.0", html_url = ReleaseFeed.Repository + "/releases/tag/v2147483647.0.0",
                draft = false, prerelease = false, assets = Array.Empty<object>() })) };
            return Task.FromResult(response);
        }
    }
}
