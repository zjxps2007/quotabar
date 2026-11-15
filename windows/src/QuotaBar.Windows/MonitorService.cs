using System.Text.Json;
using System.Security.Cryptography;
using QuotaBar.Core;

namespace QuotaBar.Windows;

// All public methods and Changed callbacks belong to the monitor's UI thread.
// Network and scanning await in the background; only this owner writes app state.
public sealed class MonitorService : IDisposable
{
    public readonly AppPaths Paths;
    private readonly SettingsStore store;
    private readonly SecretVault vault;
    private readonly ProviderClient client;
    private readonly ManagedCodexAccounts sessions;
    private readonly ClaudeConnection claude;
    private readonly CodexLogin login;
    private readonly ILaunchAtLogin launchAtLogin;
    private LaunchAtLoginState launchAtLoginState;
    private readonly LocalUsageScanner scanner = new();
    private readonly SemaphoreSlim workers = new(3);
    private readonly Dictionary<Guid, CancellationTokenSource> requests = [];
    private CancellationTokenSource? loginToken, scanToken;
    private readonly Dictionary<Guid, Snapshot> snapshots = [];
    private readonly Dictionary<Guid, string> errors = [];
    private readonly HashSet<Guid> loading = [];
    private LocalUsageReport? activity;
    private string? message;
    private bool scanning, disposed, releaseCache, updatePaused;
    private readonly HashSet<Task> running = [];
    private TaskCompletionSource? loginFinished;
    public Settings Settings { get; private set; }
    public event Action? Changed;
    public MonitorService(AppPaths paths, ProviderClient? providerClient = null, ILaunchAtLogin? launchAtLogin = null)
    {
        client = providerClient ?? new();
        this.launchAtLogin = launchAtLogin ?? new LaunchAtLogin();
        Paths = paths; PrivateFiles.EnsureDirectory(paths.Root); store = new(paths); vault = new(paths);
        Settings = store.Load(); claude = new(paths); login = new(paths); sessions = new(paths.Credentials, client);
        RefreshLaunchAtLogin();
        try { snapshots = store.LoadSnapshots(Settings); }
        catch (Exception e) when (e is IOException or JsonException) { message = "이전 사용량 캐시를 읽지 못했습니다. 새로고침하면 다시 저장합니다."; }
    }
    public DisplayState State() => new(Settings, new(snapshots), new(errors), new(loading), message, false, activity, scanning, loginToken != null,
        Paths.Root, Enum.GetValues<Provider>().ToDictionary(p => p.ToString().ToLowerInvariant(), AppPaths.DefaultPath), launchAtLoginState);
    private void Notify() { if (!disposed) Changed?.Invoke(); }
    public static string Explain(Exception error) => error switch {
        UsageException e => e.Message, OperationCanceledException => "작업을 취소했습니다.", HttpRequestException => "서버에 연결하지 못했습니다. 네트워크를 확인하세요.",
        UnauthorizedAccessException => "파일 접근 권한을 확인하세요.", JsonException => "JSON 파일 형식을 확인하세요. 원본 파일은 보존했습니다.",
        System.ComponentModel.Win32Exception => "실행 파일을 찾거나 시작하지 못했습니다. CLI 설치를 확인하세요.",
        IOException => "파일을 읽거나 저장하지 못했습니다. 경로, 권한과 여유 공간을 확인하세요.", _ => "작업을 완료하지 못했습니다. 다시 시도하세요." };
    public void SetMessage(string? text) { message = text; Notify(); }
    private void SettingsChanged(Settings saved) { Settings = saved; Notify(); }
    public void Preferences(int seconds, string theme, bool scan, bool taskbar, string? trayDisplay = null, bool? autoRefresh = null, bool? automaticUpdates = null)
    {
        SettingsChanged(store.Preferences(Settings, seconds, theme, scan, taskbar, trayDisplay, autoRefresh, automaticUpdates));
        if (!scan) { StopScan(); activity = null; Notify(); }
    }
    public void RefreshLaunchAtLogin()
    {
        try { launchAtLoginState = launchAtLogin.State; }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or System.Security.SecurityException)
        { launchAtLoginState = LaunchAtLoginState.Unavailable; }
        Notify();
    }
    public void SetLaunchAtLogin(bool enabled)
    {
        try { launchAtLogin.SetEnabled(enabled); }
        finally { RefreshLaunchAtLogin(); }
    }
    public void Edit(Guid id, string name, bool enabled, bool tray)
    {
        SettingsChanged(store.Edit(Settings, id, name, enabled, tray));
        if (!enabled && requests.Remove(id, out var request)) { request.Cancel(); loading.Remove(id); Notify(); }
    }
    private Account ConnectCredential(Provider provider, string path, byte[]? login = null)
    {
        var saved = store.ConnectCredential(Settings, provider, path, login, id => {
            sessions.Invalidate(id);
            if (requests.Remove(id, out var inFlight)) { inFlight.Cancel(); loading.Remove(id); }
        });
        var account = saved.Accounts[^1];
        SettingsChanged(saved);
        errors.Remove(account.Id); message = "계정을 연결했습니다."; Notify(); return account;
    }
    public async Task Import(Provider provider, string path)
    {
        await Refresh(ConnectCredential(provider, path).Id);
    }
    public async Task Login()
    {
        if (updatePaused) throw new UsageException("updating", "업데이트 설치 준비가 끝난 뒤 다시 시도하세요.");
        if (loginToken != null) throw new UsageException("login_active", "진행 중인 로그인을 완료하거나 취소하세요.");
        var cancel = loginToken = new(); loginFinished = new(TaskCreationOptions.RunContinuationsAsynchronously); message = "브라우저에서 Codex 로그인을 완료하세요. 최대 5분 동안 기다립니다."; Notify();
        try {
            var bytes = await login.LoginAsync(cancel.Token);
            Guid? connected = null;
            try {
                cancel.Token.ThrowIfCancellationRequested();
                if (!disposed) connected = ConnectCredential(Provider.Codex, "", bytes).Id;
            } finally { CryptographicOperations.ZeroMemory(bytes); }
            if (connected is Guid id) await Refresh(id);
        }
        finally { loginToken = null; cancel.Dispose(); loginFinished.TrySetResult(); Notify(); }
    }
    public void CancelLogin() => loginToken?.Cancel();
    public async Task ConnectClaude(string directory, bool consent, string helper)
    {
        if (!consent) throw new UsageException("consent", "기존 status line을 보존해 연결하는 동의를 선택하세요.");
        var location = ClaudeConnection.Locate(directory); directory = location.Directory; string identity = "claude:" + location.Id;
        var connection = store.Prepare(Settings, Provider.Claude, Path.GetFileName(directory), identity, directory);
        var profile = claude.Install(directory, helper);
        try { SettingsChanged(store.Commit(connection)); }
        catch { if (!connection.Existing) claude.Uninstall(profile.Id); throw; }
        message = "Claude Code를 다시 열고 응답을 한 번 받으면 사용량이 전달됩니다."; await Refresh(connection.Account.Id);
    }
    public void Remove(Guid id)
    {
        var account = Settings.Accounts.Single(a => a.Id == id);
        if (account.Provider == Provider.Claude)
        {
            bool restored = claude.Uninstall(ClaudeConnection.Id(account.SourcePath));
            message = restored ? "Claude 연결을 해제하고 기존 status line을 복원했습니다." : "연결을 해제했습니다. 사용자가 변경한 Claude status line은 보존했습니다.";
        }
        SettingsChanged(store.Remove(Settings, id));
        if (requests.Remove(id, out var current)) current.Cancel(); sessions.Invalidate(id);
        vault.Delete(id); snapshots.Remove(id); errors.Remove(id); loading.Remove(id); SaveSnapshots(); Notify();
    }
    public async Task Refresh(Guid? id = null)
    {
        if (disposed || updatePaused) return;
        var accounts = Settings.Accounts.Where(a => a.Enabled && (id == null || a.Id == id) && !requests.ContainsKey(a.Id)).ToArray();
        var work = Task.WhenAll(accounts.Select(Fetch)); running.Add(work); try { await work; } finally { running.Remove(work); }
    }
    private async Task Fetch(Account account)
    {
        using var cancel = new CancellationTokenSource(); requests[account.Id] = cancel; loading.Add(account.Id); Notify();
        bool acquired = false;
        try
        {
            await workers.WaitAsync(cancel.Token); acquired = true; Snapshot result;
            if (account.Provider == Provider.Claude) result = claude.Fetch(account.SourcePath);
            else if (account.Provider == Provider.Codex && account.AppManagedLogin) result = await sessions.Fetch(account, cancel.Token);
            else
            {
                byte[] auth = store.Imported(account);
                try {
                    cancel.Token.ThrowIfCancellationRequested();
                    if (account.Provider == Provider.Gemini) result = await client.FetchGemini(auth, cancel.Token);
                    else result = await client.FetchCodex(auth, cancel.Token);
                } finally { CryptographicOperations.ZeroMemory(auth); }
            }
            if (!cancel.IsCancellationRequested && !disposed) { snapshots[account.Id] = result; errors.Remove(account.Id); SaveSnapshots(); }
        }
        catch (OperationCanceledException) when (cancel.IsCancellationRequested) { }
        catch (Exception error) { if (!cancel.IsCancellationRequested && !disposed) errors[account.Id] = Explain(error); }
        finally
        {
            if (acquired) workers.Release();
            if (requests.GetValueOrDefault(account.Id) == cancel) { requests.Remove(account.Id); loading.Remove(account.Id); }
            Notify();
        }
    }
    private void SaveSnapshots() => store.SaveSnapshots(snapshots);
    public void Suspend() { foreach (var c in requests.Values) c.Cancel(); StopScan(); }
    public void StopScan() { releaseCache = true; scanToken?.Cancel(); if (!scanning) scanner.ReleaseCache(); }
    public async Task Scan()
    {
        if (scanning || !Settings.ScanLocalLogs || disposed || updatePaused) return;
        using var cancel = new CancellationTokenSource(); scanToken = cancel; releaseCache = false; scanning = true; Notify();
        var roots = Settings.Accounts.Where(a => a.Provider == Provider.Claude).Select(a => a.SourcePath).Append(AppPaths.ClaudeHome).Distinct(StringComparer.OrdinalIgnoreCase).Select(p => Path.Combine(p, "projects")).ToArray();
        try
        {
            var result = await Task.Run(() => scanner.Scan([Path.Combine(AppPaths.CodexHome, "sessions"), Path.Combine(AppPaths.CodexHome, "archived_sessions")], roots, cancel.Token));
            if (!cancel.IsCancellationRequested && !disposed) activity = result;
        }
        catch (OperationCanceledException) { }
        catch (Exception error) { if (!disposed) message = Explain(error); }
        finally { if (disposed) scanner.Dispose(); else if (releaseCache) scanner.ReleaseCache(); scanning = false; scanToken = null; Notify(); }
    }
    public void Dispose() { if (disposed) return; disposed = true; loginToken?.Cancel(); Suspend(); if (!scanning) scanner.Dispose(); client.Dispose(); }
    public async Task PrepareUpdateExit()
    {
        updatePaused = true; Suspend();
        var work = running.ToList(); if (loginFinished != null) work.Add(loginFinished.Task);
        await Task.WhenAll(work);
        sessions.FlushPendingWrites();
    }
    public void ResumeAfterUpdate() => updatePaused = false;
    public async Task Shutdown()
    {
        disposed = true; loginToken?.Cancel(); Suspend();
        var work = running.ToList(); if (loginFinished != null) work.Add(loginFinished.Task);
        await Task.WhenAll(work); // Finish persisting any already-issued OAuth rotation before exit.
    }
}
