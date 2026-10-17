using System.Collections.Concurrent;

namespace QuotaBar.Core;
public sealed class CodexSessions(ICredentialStore store, Func<byte[], Task<byte[]>> refresh)
{
    private readonly object sync = new();
    private readonly ConcurrentDictionary<Guid, SemaphoreSlim> gates = new();
    private readonly Dictionary<Guid, Guid> generations = [];
    private readonly Dictionary<Guid, (byte[] Original, byte[] Replacement)> pending = [];
    private readonly HashSet<Guid> revoked = [];
    public void Invalidate(Guid id) { lock (sync) { generations[id] = Guid.NewGuid(); pending.Remove(id); revoked.Remove(id); } }
    public async Task<byte[]> Credential(Account account, byte[]? rejected = null)
    {
        var gate = gates.GetOrAdd(account.Id, _ => new(1)); await gate.WaitAsync();
        try
        {
            byte[] current; Guid generation;
            lock (sync)
            {
                if (revoked.Contains(account.Id)) throw new UsageException("reauth_required", "앱에서 다시 로그인하세요.");
                current = store.Load(account.Id) ?? throw new UsageException("missing_credentials", "로그인을 다시 연결하세요.");
                if (pending.TryGetValue(account.Id, out var write))
                { if (current.SequenceEqual(write.Original)) { store.Save(account.Id, write.Replacement); current = write.Replacement; } pending.Remove(account.Id); }
                var creds = Credentials.Parse(current, Provider.Codex);
                if (creds.Identity != account.Identity) throw new UsageException("account_mismatch", "저장된 계정이 다릅니다.");
                if (!creds.NeedsRefresh(DateTimeOffset.UtcNow) && (rejected == null || !current.SequenceEqual(rejected))) return current;
                if (!generations.TryGetValue(account.Id, out generation)) generations[account.Id] = generation = Guid.NewGuid();
            }
            try
            {
                var replacement = await refresh(current);
                lock (sync)
                {
                    if (generations[account.Id] != generation || store.Load(account.Id)?.SequenceEqual(current) != true) throw new OperationCanceledException();
                    pending[account.Id] = (current, replacement); store.Save(account.Id, replacement); pending.Remove(account.Id); return replacement;
                }
            }
            catch (UsageException e) when (e.Code == "reauth_required") { lock (sync) if (generations[account.Id] == generation) revoked.Add(account.Id); throw; }
        }
        finally { gate.Release(); }
    }
}
