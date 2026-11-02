using System.Diagnostics;
using QuotaBar.Core;
using QuotaBar.Windows;

namespace QuotaBar.Monitor;

// Owned by the monitor UI thread; Dashboard receives display state only.
internal sealed class AppUpdateManager : IDisposable
{
    private readonly ReleaseClient client;
    private readonly UpdateDownloader downloader = new();
    private readonly System.Windows.Forms.Timer timer = new();
    private readonly AppPaths paths;
    private readonly Action changed;
    private readonly Func<Task> exit;
    private readonly Func<Task> prepareExit;
    private readonly Action resume;
    private CancellationTokenSource? operation;
    private DateTimeOffset? lastAttempt;
    private StagedUpdate? staged;
    private string? job, blockedVersion;
    private bool enabled, canInstall, suspended, disposed;
    public AppUpdateState State { get; private set; } = new();

    public AppUpdateManager(AppPaths paths, Action changed, Func<Task> prepareExit, Action resume, Func<Task> exit, ReleaseClient? releaseClient = null)
    {
        client = releaseClient ?? new();
        this.paths = paths; this.changed = changed; this.prepareExit = prepareExit; this.resume = resume; this.exit = exit;
        if (UpdateInstaller.ReadResult(UpdateInstaller.ResultPath(paths)) is { Success: false } previous)
        { blockedVersion = previous.Version; State = new(UpdatePhase.Failed, previous.Version, Message: previous.Message); }
        timer.Tick += (_, _) => Check(manual: false);
        try { UpdateInstaller.CleanCompleted(paths); } catch (IOException) { } catch (UnauthorizedAccessException) { }
    }
    public void Configure(bool enabled, bool canInstall, bool suspended)
    {
        bool stop = (this.enabled && !enabled) || (!this.suspended && suspended);
        this.enabled = enabled; this.canInstall = canInstall; this.suspended = suspended;
        if (stop && State.Phase != UpdatePhase.Installing) operation?.Cancel();
        if (enabled && canInstall && !suspended && staged != null && operation == null && staged.Update.Release.Version != blockedVersion)
            Install(force: false);
        else Schedule();
    }
    public void Check(bool manual = true, bool install = false)
    {
        if (disposed || suspended || operation != null) return;
        if (manual) blockedVersion = null;
        _ = Run(async token =>
        {
            try { await Task.Run(() => UpdateInstaller.CleanCompleted(paths), token); } catch (IOException) { } catch (UnauthorizedAccessException) { }
            Discard(); lastAttempt = DateTimeOffset.UtcNow; Set(new(UpdatePhase.Checking));
            var platform = UpdateInstaller.Installed(AppContext.BaseDirectory) ? ReleasePlatform.WindowsInstaller : ReleasePlatform.WindowsPortable;
            var release = await client.Latest(platform, Brand.Version, token); token.ThrowIfCancellationRequested();
            if (release == null) { Set(new(Message: "공개된 정식 릴리즈가 없습니다.", CheckedAt: DateTimeOffset.UtcNow)); return; }
            if (!release.IsNewer(Brand.Version)) { Set(new(UpdatePhase.Current, Brand.Version, CheckedAt: DateTimeOffset.UtcNow)); return; }
            if (release.Version == blockedVersion)
            { Set(new(UpdatePhase.Failed, release.Version, Message: "이 버전의 자동 설치에 실패했습니다. ‘업데이트 확인’으로 다시 시도할 수 있습니다.")); return; }
            Set(new(UpdatePhase.Available, release.Version, CheckedAt: DateTimeOffset.UtcNow));
            if (!enabled && !install) return;
            if (release.Asset == null) { Set(State with { Message = "이 PC에 맞는 업데이트 파일이 아직 없습니다." }); return; }
            Set(State with { Phase = UpdatePhase.Downloading, Progress = 0 });
            staged = await downloader.Prepare(release, Brand.Version, UpdateInstaller.PublicKey,
                Path.Combine(UpdateInstaller.UpdatesRoot(paths), Guid.NewGuid().ToString("D")),
                value => { if (!disposed && State.Phase == UpdatePhase.Downloading) Set(State with { Progress = value }); }, token);
            token.ThrowIfCancellationRequested(); Set(new(UpdatePhase.Ready, release.Version, CheckedAt: DateTimeOffset.UtcNow));
            if (install || (enabled && canInstall)) await Apply(install, token);
        });
    }
    public void InstallNow() { if (staged == null) Check(install: true); else Install(force: true); }
    private void Install(bool force)
    {
        if (disposed || suspended || operation != null || staged == null || (!force && staged.Update.Release.Version == blockedVersion)) return;
        _ = Run(token => Apply(force, token));
    }
    private bool Allowed(bool force) => !disposed && !suspended && (force || (enabled && canInstall));
    private async Task Apply(bool force, CancellationToken token)
    {
        if (staged == null || !Allowed(force)) return;
        Process? helper = null; bool handedOff = false, paused = false;
        try
        {
            Set(State with { Phase = UpdatePhase.Installing, Progress = null });
            var payload = staged;
            job ??= await Task.Run(() => UpdateInstaller.Prepare(payload, paths, AppContext.BaseDirectory, Brand.Version, token), token);
            token.ThrowIfCancellationRequested();
            if (!Allowed(force)) { Set(State with { Phase = UpdatePhase.Ready }); return; }
            paused = true; await prepareExit(); token.ThrowIfCancellationRequested();
            if (!Allowed(force)) { Set(State with { Phase = UpdatePhase.Ready }); return; }
            helper = UpdateInstaller.LaunchHelper(job);
            bool ready = false;
            for (int attempt = 0; attempt < 200; attempt++)
            {
                token.ThrowIfCancellationRequested(); if (helper.HasExited) throw new IOException("Update helper exited.");
                var result = UpdateInstaller.ReadResult(Path.Combine(staged.Directory, "installer-ready.json"));
                if (result is { Success: true } && result.Version == staged.Update.Release.Version) { ready = true; break; }
                await Task.Delay(100, token);
            }
            if (!ready || helper.HasExited) throw new IOException("Update helper did not become ready.");
            if (!Allowed(force)) { Set(State with { Phase = UpdatePhase.Ready }); return; }
            handedOff = true;
            await exit(); // Drains credential rotations and closes Dashboard before releasing the instance lease.
        }
        catch { handedOff = false; blockedVersion = staged.Update.Release.Version; throw; }
        finally
        {
            if (helper != null)
            {
                if (!handedOff && !helper.HasExited)
                { try { helper.Kill(); await helper.WaitForExitAsync(); } catch (InvalidOperationException) { } }
                helper.Dispose();
            }
            if (paused && !handedOff) resume();
        }
    }
    private async Task Run(Func<CancellationToken, Task> work)
    {
        timer.Stop(); var cancel = operation = new();
        try { await work(cancel.Token); }
        catch (OperationCanceledException) when (cancel.IsCancellationRequested)
        { lastAttempt = null; Discard(); Set(new()); }
        catch (Exception e)
        { Set(new(UpdatePhase.Failed, State.Version, Message: e is UsageException known ? known.Message : "자동 업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다.", CheckedAt: DateTimeOffset.UtcNow)); }
        finally { if (operation == cancel) operation = null; cancel.Dispose(); Schedule(); }
    }
    private void Set(AppUpdateState state) { if (!disposed && State != state) { State = state; changed(); } }
    private void Schedule()
    {
        if (disposed) return;
        timer.Stop(); if (!enabled || suspended || operation != null || (staged != null && staged.Update.Release.Version != blockedVersion)) return;
        var delay = lastAttempt == null ? TimeSpan.FromSeconds(15) : lastAttempt.Value.AddDays(1) - DateTimeOffset.UtcNow;
        timer.Interval = (int)Math.Clamp(delay.TotalMilliseconds, 15000, int.MaxValue); timer.Start();
    }
    private void Discard()
    {
        if (staged == null) return;
        try
        {
            if (Guid.TryParse(Path.GetFileName(staged.Directory), out var transaction))
            {
                var replacement = new UpdateReplacement(AppContext.BaseDirectory, transaction);
                if (!Directory.Exists(replacement.Backup) && Directory.Exists(replacement.Incoming)) Directory.Delete(replacement.Incoming, true);
            }
            Directory.Delete(staged.Directory, true);
        }
        catch (IOException) { } catch (UnauthorizedAccessException) { }
        staged = null; job = null;
    }
    public void Dispose() { disposed = true; operation?.Cancel(); timer.Dispose(); client.Dispose(); downloader.Dispose(); }
}
