using System.Text.Json;
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
    private readonly CodexSessions sessions;
    private readonly ClaudeConnection claude;
    private readonly CodexLogin login;
    private readonly LocalUsageScanner scanner = new();
    private readonly SemaphoreSlim workers = new(3);
    private readonly Dictionary<Guid, CancellationTokenSource> requests = [];
    private CancellationTokenSource? loginToken, scanToken;
    private readonly Dictionary<Guid, Snapshot> snapshots = [];
    private readonly Dictionary<Guid, string> errors = [];
    private readonly HashSet<Guid> loading = [];
    private LocalUsageReport? activity;
    private string? message;
    private bool scanning, disposed, releaseCache;
    private readonly HashSet<Task> running = [];
    private TaskCompletionSource? loginFinished;
    public Settings Settings { get; private set; }
    public event Action? Changed;
    public MonitorService(AppPaths paths, ProviderClient? providerClient = null)
    {
        client = providerClient ?? new();
        Paths = paths; PrivateFiles.EnsureDirectory(paths.Root); store = new(paths); vault = new(paths);
        Settings = store.Load(); claude = new(paths); login = new(paths); sessions = new(vault, client.Refresh);
        string cache = Path.Combine(paths.Root, "snapshots.json");
        if (File.Exists(cache))
            try { foreach (var pair in Json.Read<Dictionary<Guid, Snapshot>>(PrivateFiles.ReadBounded(cache, 2 * 1024 * 1024)))
                if (Settings.Accounts.Any(a => a.Id == pair.Key) && pair.Value.Windows != null) snapshots[pair.Key] = pair.Value; }
            catch (Exception e) when (e is IOException or JsonException) { message = "이전 사용량 캐시를 읽지 못했습니다. 새로고침하면 다시 저장합니다."; }
    }
    public DisplayState State() => new(Settings, new(snapshots), new(errors), new(loading), message, false, activity, scanning, loginToken != null,
        Paths.Root, Enum.GetValues<Provider>().ToDictionary(p => p.ToString().ToLowerInvariant(), AppPaths.DefaultPath));
    private void Notify() { if (!disposed) Changed?.Invoke(); }
    public static string Explain(Exception error) => error switch {
        UsageException e => e.Message, OperationCanceledException => "작업을 취소했습니다.", HttpRequestException => "서버에 연결하지 못했습니다. 네트워크를 확인하세요.",
        UnauthorizedAccessException => "파일 접근 권한을 확인하세요.", JsonException => "JSON 파일 형식을 확인하세요. 원본 파일은 보존했습니다.",
        System.ComponentModel.Win32Exception => "실행 파일을 찾거나 시작하지 못했습니다. CLI 설치를 확인하세요.",
        IOException => "파일을 읽거나 저장하지 못했습니다. 경로, 권한과 여유 공간을 확인하세요.", _ => "작업을 완료하지 못했습니다. 다시 시도하세요." };
    public void SetMessage(string? text) { message = text; Notify(); }
    private void Save(Settings value) { store.Save(value); Settings = value; Notify(); }
    public void Preferences(int seconds, string theme, bool scan, bool taskbar, string? trayDisplay = null)
    {
        trayDisplay ??= Settings.TrayDisplayMode;
        if (!new[] { 60, 300, 900, 1800 }.Contains(seconds) || !new[] { "system", "light", "dark" }.Contains(theme)
            || !new[] { "remaining", "icon" }.Contains(trayDisplay)) throw new UsageException("settings", "지원하는 설정 값을 선택하세요.");
        Save(Settings with { RefreshSeconds = seconds, Appearance = theme, ScanLocalLogs = scan, ShowTaskbarIcon = taskbar, TrayDisplayMode = trayDisplay });
        if (!scan) { StopScan(); activity = null; Notify(); }
    }
    public void Edit(Guid id, string name, bool enabled, bool tray)
    {
        if (string.IsNullOrWhiteSpace(name) || name.Length > 100) throw new UsageException("name", "이름은 1~100자로 입력하세요.");
        Save(Settings with { Accounts = Settings.Accounts.Select(a => a.Id == id ? a with { Name = name.Trim(), Enabled = enabled, ShowInTray = tray } : a).ToList() });
        if (!enabled && requests.Remove(id, out var request)) { request.Cancel(); loading.Remove(id); Notify(); }
    }
    private Account Add(Provider provider, byte[] bytes, string path, bool managed)
    {
        var credentials = Credentials.Parse(bytes, provider);
        var previous = Settings.Accounts.FirstOrDefault(a => a.Provider == provider && a.Identity == credentials.Identity);
        if (Settings.Accounts.Count >= 50 && previous == null) throw new UsageException("accounts", "계정은 최대 50개까지 연결할 수 있습니다.");
        var account = previous is null ? new Account(Guid.NewGuid(), provider, credentials.Label, credentials.Identity, path, AppManagedLogin: managed)
            : previous with { SourcePath = path, AppManagedLogin = managed };
        var old = previous == null ? null : vault.Load(account.Id); sessions.Invalidate(account.Id);
        if (requests.Remove(account.Id, out var inFlight)) { inFlight.Cancel(); loading.Remove(account.Id); }
        vault.Save(account.Id, bytes);
        try { Save(Settings with { Accounts = Settings.Accounts.Where(a => a.Id != account.Id).Append(account).ToList() }); }
        catch { if (old != null) vault.Save(account.Id, old); else vault.Delete(account.Id); throw; }
        errors.Remove(account.Id); message = "계정을 연결했습니다."; Notify(); return account;
    }
    public async Task Import(Provider provider, string path)
    {
        if (!Enum.IsDefined(provider)) throw new UsageException("provider", "지원하지 않는 서비스입니다.");
        if (provider == Provider.Claude) throw new UsageException("provider", "Claude는 설정 폴더의 status line으로 연결하세요.");
        if (!Path.IsPathFullyQualified(path) || path.StartsWith(@"\\")) throw new UsageException("path", "Windows 로컬 파일을 선택하세요.");
        path = Path.GetFullPath(path); PrivateFiles.RejectLinks(path);
        var account = Add(provider, PrivateFiles.ReadBounded(path), path, false); await Refresh(account.Id);
    }
    public async Task Login()
    {
        if (loginToken != null) throw new UsageException("login_active", "진행 중인 로그인을 완료하거나 취소하세요.");
        var cancel = loginToken = new(); loginFinished = new(TaskCreationOptions.RunContinuationsAsynchronously); message = "브라우저에서 Codex 로그인을 완료하세요. 최대 5분 동안 기다립니다."; Notify();
        try { var bytes = await login.LoginAsync(cancel.Token); cancel.Token.ThrowIfCancellationRequested(); if (!disposed) await Refresh(Add(Provider.Codex, bytes, "", true).Id); }
        finally { loginToken = null; cancel.Dispose(); loginFinished.TrySetResult(); Notify(); }
    }
    public void CancelLogin() => loginToken?.Cancel();
    public async Task ConnectClaude(string directory, bool consent, string helper)
    {
        if (!consent) throw new UsageException("consent", "기존 status line을 보존해 연결하는 동의를 선택하세요.");
        directory = ClaudeConnection.Canonical(directory); string identity = "claude:" + ClaudeConnection.Id(directory);
        var existing = Settings.Accounts.FirstOrDefault(a => a.Identity == identity);
        if (existing == null && Settings.Accounts.Count >= 50) throw new UsageException("accounts", "계정은 최대 50개까지 연결할 수 있습니다.");
        var profile = claude.Install(directory, helper);
        var account = existing ?? new Account(Guid.NewGuid(), Provider.Claude, Path.GetFileName(directory), identity, directory);
        try { Save(Settings with { Accounts = Settings.Accounts.Where(a => a.Id != account.Id).Append(account).ToList() }); }
        catch { if (existing == null) claude.Uninstall(profile.Id); throw; }
        message = "Claude Code를 다시 열고 응답을 한 번 받으면 사용량이 전달됩니다."; await Refresh(account.Id);
    }
    public void Remove(Guid id)
    {
        var account = Settings.Accounts.Single(a => a.Id == id);
        if (account.Provider == Provider.Claude)
        {
            bool restored = claude.Uninstall(ClaudeConnection.Id(account.SourcePath));
            message = restored ? "Claude 연결을 해제하고 기존 status line을 복원했습니다." : "연결을 해제했습니다. 사용자가 변경한 Claude status line은 보존했습니다.";
        }
        Save(Settings with { Accounts = Settings.Accounts.Where(a => a.Id != id).ToList() });
        if (requests.Remove(id, out var current)) current.Cancel(); sessions.Invalidate(id);
        vault.Delete(id); snapshots.Remove(id); errors.Remove(id); loading.Remove(id); SaveSnapshots(); Notify();
    }
    private byte[] Imported(Account account)
    {
        byte[] data;
        if (File.Exists(account.SourcePath))
        {
            PrivateFiles.RejectLinks(account.SourcePath); data = PrivateFiles.ReadBounded(account.SourcePath);
            if (Credentials.Parse(data, account.Provider).Identity != account.Identity) throw new UsageException("account_mismatch", "CLI에 다른 계정이 로그인되어 있습니다. 기존 계정과 섞지 않으려면 다시 연결하세요.");
            vault.Save(account.Id, data);
        }
        else data = vault.Load(account.Id) ?? throw new UsageException("missing_credentials", "로그인 파일을 다시 연결하세요.");
        if (Credentials.Parse(data, account.Provider).Identity != account.Identity) throw new UsageException("account_mismatch", "인증 계정이 일치하지 않습니다.");
        return data;
    }
    public async Task Refresh(Guid? id = null)
    {
        if (disposed) return;
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
            else
            {
                byte[] auth = account.AppManagedLogin ? await sessions.Credential(account) : Imported(account);
                cancel.Token.ThrowIfCancellationRequested();
                if (account.Provider == Provider.Gemini) result = await client.FetchGemini(auth, cancel.Token);
                else
                {
                    try { result = await client.FetchCodex(auth, cancel.Token); }
                    catch (UsageException e) when (e.Code == "unauthorized" && account.AppManagedLogin)
                    { auth = await sessions.Credential(account, auth); cancel.Token.ThrowIfCancellationRequested(); result = await client.FetchCodex(auth, cancel.Token); }
                }
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
    private void SaveSnapshots() { PrivateFiles.Write(Path.Combine(Paths.Root, "snapshots.json"), Json.Bytes(snapshots)); }
    public void Suspend() { foreach (var c in requests.Values) c.Cancel(); StopScan(); }
    public void StopScan() { releaseCache = true; scanToken?.Cancel(); if (!scanning) scanner.ReleaseCache(); }
    public async Task Scan()
    {
        if (scanning || !Settings.ScanLocalLogs || disposed) return;
        using var cancel = new CancellationTokenSource(); scanToken = cancel; releaseCache = false; scanning = true; Notify();
        var roots = Settings.Accounts.Where(a => a.Provider == Provider.Claude).Select(a => a.SourcePath).Append(AppPaths.ClaudeHome).Distinct(StringComparer.OrdinalIgnoreCase).Select(p => Path.Combine(p, "projects")).ToArray();
        try
        {
            var result = await Task.Run(() => scanner.Scan([Path.Combine(AppPaths.CodexHome, "sessions"), Path.Combine(AppPaths.CodexHome, "archived_sessions")], roots, cancel.Token,
                fileIdentity: file => Native.GetFileInformationByHandle(file.SafeFileHandle, out var info) ? $"{info.Volume}:{info.IndexHigh}:{info.IndexLow}" : "unknown"));
            if (!cancel.IsCancellationRequested && !disposed) activity = result;
        }
        catch (OperationCanceledException) { }
        catch (Exception error) { if (!disposed) message = Explain(error); }
        finally { if (releaseCache) scanner.ReleaseCache(); scanning = false; scanToken = null; Notify(); }
    }
    public void Dispose() { disposed = true; loginToken?.Cancel(); Suspend(); client.Dispose(); }
    public async Task Shutdown()
    {
        disposed = true; loginToken?.Cancel(); Suspend();
        var work = running.ToList(); if (loginFinished != null) work.Add(loginFinished.Task);
        await Task.WhenAll(work); // Finish persisting any already-issued OAuth rotation before exit.
    }
}
