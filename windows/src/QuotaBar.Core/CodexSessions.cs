namespace QuotaBar.Core;

public sealed class CodexSessions
{
    private readonly NativeCodexSessions native;
    private readonly Func<byte[], Task<byte[]>> refresh;

    public CodexSessions(ICredentialStore store, Func<byte[], Task<byte[]>> refresh)
    {
        native = new(null, store);
        this.refresh = refresh;
    }

    // Production rotation reads and writes the existing DPAPI vault directly in Rust.
    public CodexSessions(string credentialDirectory, Func<byte[], Task<byte[]>> refresh)
    {
        native = new(credentialDirectory, null);
        this.refresh = refresh;
    }

    public void Invalidate(Guid id) => native.Notify(id.ToString("D"), 0, NativeCodexSessions.Event.Invalidate);
    public void FlushPendingWrites() => native.Flush();

    public async Task<byte[]> Credential(Account account, byte[]? rejected = null)
    {
        string id = account.Id.ToString("D");
        await native.Enter(id);
        ulong ticket = 0;
        try
        {
            var current = native.Begin(id, account.Identity, rejected, out ticket);
            if (ticket == 0) return current;
            try
            {
                var replacement = await refresh(current);
                native.Complete(id, ticket, replacement);
                return replacement;
            }
            catch (UsageException error) when (error.Code == "reauth_required")
            {
                native.Notify(id, ticket, NativeCodexSessions.Event.Revoke);
                throw;
            }
        }
        finally
        {
            if (ticket != 0) native.Notify(id, ticket, NativeCodexSessions.Event.End);
            native.Leave(id);
        }
    }
}
