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
                bool automatic = false;
                var checks = new List<string>();
                AppUpdateManager? manager = null;
                manager = new(dispatcher, new AppPaths(root), () => manager!.Configure(automatic, false, false),
                    () => throw new InvalidOperationException("Must not install"), () => { },
                    () => throw new InvalidOperationException("Must not exit"), new ReleaseClient(handler));
                using (manager)
                {
                    const string offered = "2147483646.0.0";
                    async Task Check(string name, HttpStatusCode status, string tag, UpdatePhase phase, string? version = null, string? message = null)
                    {
                        handler.Status = status; handler.Tag = tag;
                        int expectedRequests = handler.Requests + 1;
                        manager.Check();
                        manager.Check(); // A second click while busy must not issue another request.
                        var deadline = DateTime.UtcNow.AddSeconds(5);
                        while (handler.Requests != expectedRequests || manager.State.Phase != phase || manager.State.Version != version ||
                               manager.State.Message != message || manager.State.CheckedAt == null)
                        {
                            if (DateTime.UtcNow > deadline) throw new InvalidOperationException($"{name}: {manager.State}, requests={handler.Requests}");
                            await Task.Delay(25);
                        }
                        checks.Add(name);
                    }
                    manager.Configure(automatic, false, false);
                    await Check("no published release", HttpStatusCode.NotFound, "", UpdatePhase.Idle, message: "공개된 정식 릴리즈가 없습니다.");
                    await Check("installed version is current", HttpStatusCode.OK, "v0.0.1", UpdatePhase.Current, Brand.Version);
                    await Check("manual check with automatic updates off", HttpStatusCode.OK, "v" + offered, UpdatePhase.Available, offered);
                    automatic = true; manager.Configure(automatic, false, false);
                    await Check("automatic update without a platform asset", HttpStatusCode.OK, "v" + offered, UpdatePhase.Available, offered,
                        "이 PC에 맞는 업데이트 파일이 아직 없습니다.");
                    automatic = false; manager.Configure(automatic, false, false);
                    await Check("rate-limited feed", HttpStatusCode.TooManyRequests, "", UpdatePhase.Failed,
                        message: "업데이트 확인 요청이 제한되었습니다. 잠시 후 다시 시도하세요.");
                    await Check("retry after rate limit", HttpStatusCode.OK, "v" + offered, UpdatePhase.Available, offered);
                }
                File.WriteAllBytes(resultPath, Json.Bytes(new { passed = true, monitorPid = Environment.ProcessId, manualCheckWhileAutomaticUpdatesDisabled = true, updateChecks = checks }));
            }
            catch (Exception error) { File.WriteAllBytes(resultPath, Json.Bytes(new { passed = false, error = error.ToString() })); }
            finally { if (Directory.Exists(root)) Directory.Delete(root, true); dispatcher.Exit(); }
        });
    }
    private sealed class FixtureHandler : HttpMessageHandler
    {
        public int Requests;
        public HttpStatusCode Status;
        public string Tag = "";
        protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token)
        {
            Requests++; await Task.Delay(30, token);
            return new HttpResponseMessage(Status) { Content = new ByteArrayContent(Json.Bytes(new {
                tag_name = Tag, html_url = ReleaseFeed.Repository + "/releases/tag/" + Tag,
                draft = false, prerelease = false, assets = Array.Empty<object>() })) };
        }
    }
}
