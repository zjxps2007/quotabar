using QuotaBar.Core;
using QuotaBar.Windows;

namespace QuotaBar.Monitor;

// Owned by the monitor UI thread; Dashboard receives display state only.
internal sealed class AppUpdateManager : IDisposable
{
    private readonly ReleaseClient client;
    private readonly UpdateDownloader downloader = new();
    private readonly MonitorLoop dispatcher;
    private readonly AppPaths paths;
    private readonly Action changed;
    private readonly Func<Task> exit;
    private readonly Func<Task> prepareExit;
    private readonly Action resume;
    private CancellationTokenSource? operation;
    private readonly UpdatePolicy policy = UpdatePolicy.Create();
    private StagedUpdate? staged;
    private string? job;
    private bool disposed;
    public AppUpdateState State { get; private set; } = new();

    public AppUpdateManager(MonitorLoop dispatcher, AppPaths paths, Action changed, Func<Task> prepareExit, Action resume, Func<Task> exit, ReleaseClient? releaseClient = null)
    {
        this.dispatcher = dispatcher;
        client = releaseClient ?? new();
        this.paths = paths; this.changed = changed; this.prepareExit = prepareExit; this.resume = resume; this.exit = exit;
        if (UpdateInstaller.ReadResult(UpdateInstaller.ResultPath(paths)) is { Success: false } previous)
        { UpdatePolicy.Block(policy, previous.Version); State = new(UpdatePhase.Failed, previous.Version, Message: previous.Message); }
        dispatcher.Received += OnSystemEvent;
        try { UpdateInstaller.CleanCompleted(paths); } catch (IOException) { } catch (UnauthorizedAccessException) { }
    }
    private void OnSystemEvent(MonitorLoop.Event value) { if (value == MonitorLoop.Event.Update) Check(manual: false); }
    public void Configure(bool enabled, bool canInstall, bool suspended)
    {
        if (disposed) return;
        var action = UpdatePolicy.Configure(policy, enabled, canInstall, suspended);
        if (action == UpdatePolicy.OnConfigure.Cancel) operation?.Cancel();
        if (action == UpdatePolicy.OnConfigure.Install) Install(force: false);
        else Schedule();
    }
    public void Check(bool manual = true, bool install = false)
    {
        if (disposed || !UpdatePolicy.BeginCheck(policy, manual)) return;
        _ = Run(async token =>
        {
            try { await Task.Run(() => UpdateInstaller.CleanCompleted(paths), token); } catch (IOException) { } catch (UnauthorizedAccessException) { }
            Discard(); UpdatePolicy.Attempted(policy, Now); Set(new(UpdatePhase.Checking));
            var platform = UpdateInstaller.Installed(AppContext.BaseDirectory) ? ReleasePlatform.WindowsInstaller : ReleasePlatform.WindowsPortable;
            var release = await client.Latest(platform, Brand.Version, token); token.ThrowIfCancellationRequested();
            if (release == null) { Set(new(Message: "공개된 정식 릴리즈가 없습니다.", CheckedAt: DateTimeOffset.UtcNow)); return; }
            var offer = UpdatePolicy.Evaluate(policy, Brand.Version, release.Version);
            if (offer == UpdatePolicy.Offer.Current) { Set(new(UpdatePhase.Current, Brand.Version, CheckedAt: DateTimeOffset.UtcNow)); return; }
            if (offer == UpdatePolicy.Offer.Blocked)
            { Set(new(UpdatePhase.Failed, release.Version, Message: "이 버전의 자동 설치에 실패했습니다. ‘업데이트 확인’으로 다시 시도할 수 있습니다.")); return; }
            Set(new(UpdatePhase.Available, release.Version, CheckedAt: DateTimeOffset.UtcNow));
            if (!UpdatePolicy.ShouldDownload(policy, install)) return;
            if (release.Asset == null) { Set(State with { Message = "이 PC에 맞는 업데이트 파일이 아직 없습니다." }); return; }
            Set(State with { Phase = UpdatePhase.Downloading, Progress = 0 });
            staged = await downloader.Prepare(release, Brand.Version, UpdateInstaller.PublicKey,
                Path.Combine(UpdateInstaller.UpdatesRoot(paths), Guid.NewGuid().ToString("D")),
                value => { if (!disposed && State.Phase == UpdatePhase.Downloading) Set(State with { Progress = value }); }, token);
            UpdatePolicy.Staged(policy, release.Version);
            token.ThrowIfCancellationRequested(); Set(new(UpdatePhase.Ready, release.Version, CheckedAt: DateTimeOffset.UtcNow));
            await Apply(install, token);
        });
    }
    public void InstallNow() { if (staged == null) Check(install: true); else Install(force: true); }
    private void Install(bool force)
    {
        if (disposed || !UpdatePolicy.BeginInstall(policy, force)) return;
        _ = Run(token => Apply(force, token));
    }
    private bool Allowed(bool force) => !disposed && UpdatePolicy.MayInstall(policy, force);
    private async Task Apply(bool force, CancellationToken token)
    {
        if (disposed || staged == null || !UpdatePolicy.BeginHandoff(policy, force)) return;
        UpdateInstaller.Helper? helper = null; bool handedOff = false, paused = false;
        try
        {
            Set(State with { Phase = UpdatePhase.Installing, Progress = null });
            var payload = staged;
            job ??= await UpdateInstaller.Prepare(payload, paths, AppContext.BaseDirectory, Brand.Version, token);
            token.ThrowIfCancellationRequested();
            if (!Allowed(force)) { Set(State with { Phase = UpdatePhase.Ready }); return; }
            paused = true; await prepareExit(); token.ThrowIfCancellationRequested();
            if (!Allowed(force)) { Set(State with { Phase = UpdatePhase.Ready }); return; }
            helper = UpdateInstaller.Helper.Start(job);
            bool ready = false;
            for (int attempt = 0; attempt < 200; attempt++)
            {
                token.ThrowIfCancellationRequested();
                if (helper.IsReady()) { ready = true; break; }
                await Task.Delay(100, token);
            }
            if (!ready || !helper.IsReady()) throw new IOException("Update helper did not become ready.");
            if (!Allowed(force)) { Set(State with { Phase = UpdatePhase.Ready }); return; }
            handedOff = true;
            await exit(); // Drains credential rotations and closes Dashboard before releasing the instance lease.
        }
        catch { handedOff = false; UpdatePolicy.Block(policy, staged.Update.Release.Version); throw; }
        finally
        {
            if (helper != null)
            {
                if (handedOff) helper.Detach();
                helper.Dispose();
            }
            if (paused && !handedOff) resume();
        }
    }
    private async Task Run(Func<CancellationToken, Task> work)
    {
        dispatcher.Schedule(MonitorLoop.Timer.Update, 0); var cancel = operation = new();
        bool cancelled = false;
        try { await work(cancel.Token); }
        catch (OperationCanceledException) when (cancel.IsCancellationRequested)
        { cancelled = true; Discard(); Set(new()); }
        catch (Exception e)
        { Set(new(UpdatePhase.Failed, State.Version, Message: e is UsageException known ? known.Message : "자동 업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다.", CheckedAt: DateTimeOffset.UtcNow)); }
        finally
        {
            operation = null; cancel.Dispose();
            UpdatePolicy.Finish(policy, cancelled);
            if (disposed) policy.Dispose(); else Schedule();
        }
    }
    private void Set(AppUpdateState state) { if (!disposed && State != state) { State = state; changed(); } }
    private static double Now => DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() / 1000.0;
    private void Schedule()
    {
        if (disposed) return;
        var delay = UpdatePolicy.Delay(policy, Now);
        dispatcher.Schedule(MonitorLoop.Timer.Update, (int)Math.Min(delay * 1000, int.MaxValue));
    }
    private void Discard()
    {
        if (staged == null) return;
        try
        {
            UpdateInstaller.Discard(paths, AppContext.BaseDirectory, staged.Directory);
        }
        catch (IOException) { } catch (UnauthorizedAccessException) { }
        staged = null; job = null;
    }
    public void Dispose()
    {
        disposed = true; operation?.Cancel();
        if (operation == null) policy.Dispose();
        dispatcher.Received -= OnSystemEvent; dispatcher.Schedule(MonitorLoop.Timer.Update, 0);
        client.Dispose(); downloader.Dispose();
    }
}
