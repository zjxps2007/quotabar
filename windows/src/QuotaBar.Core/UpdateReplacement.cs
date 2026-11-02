namespace QuotaBar.Core;

public sealed class UpdateReplacement
{
    public string Target { get; }
    public string Incoming { get; }
    public string Backup { get; }
    public UpdateReplacement(string target, Guid transaction)
    {
        Target = Path.TrimEndingDirectorySeparator(Path.GetFullPath(target));
        string parent = Path.GetDirectoryName(Target) ?? throw new IOException("Invalid update target.");
        if (parent.Length == 0 || Target == Path.GetPathRoot(Target)) throw new IOException("Invalid update target.");
        string name = Path.GetFileName(Target);
        Incoming = Path.Combine(parent, "." + name + ".update-" + transaction.ToString("D"));
        Backup = Path.Combine(parent, "." + name + ".backup-" + transaction.ToString("D"));
    }
    public void Commit(Action? afterBackup = null)
    {
        if (!Directory.Exists(Target) || !Directory.Exists(Incoming) || Directory.Exists(Backup)) throw new IOException("Invalid update transaction state.");
        Directory.Move(Target, Backup);
        try { afterBackup?.Invoke(); Directory.Move(Incoming, Target); }
        catch { if (!Directory.Exists(Target)) Directory.Move(Backup, Target); throw; }
    }
    public void Rollback()
    {
        if (!Directory.Exists(Backup) || Directory.Exists(Incoming)) throw new IOException("Invalid update rollback state.");
        bool existed = Directory.Exists(Target);
        if (existed) Directory.Move(Target, Incoming);
        try { Directory.Move(Backup, Target); }
        catch { if (existed && !Directory.Exists(Target)) Directory.Move(Incoming, Target); throw; }
        if (existed) Directory.Delete(Incoming, true);
    }
}
