using System.Runtime.InteropServices;
using QuotaBar.Core;

namespace QuotaBar.Windows;

public sealed class SecretVault(AppPaths paths) : ICredentialStore
{
    private const string Library = "quotabar_native";
    [DllImport(Library, EntryPoint = "qb_vault_load")]
    private static extern int Read([MarshalAs(UnmanagedType.LPUTF8Str)] string directory, [MarshalAs(UnmanagedType.LPUTF8Str)] string id, out IntPtr bytes, out nuint length);
    [DllImport(Library, EntryPoint = "qb_vault_save")]
    private static extern int Write([MarshalAs(UnmanagedType.LPUTF8Str)] string directory, [MarshalAs(UnmanagedType.LPUTF8Str)] string id, byte[] bytes, nuint length);
    [DllImport(Library, EntryPoint = "qb_vault_delete")]
    private static extern int Remove([MarshalAs(UnmanagedType.LPUTF8Str)] string directory, [MarshalAs(UnmanagedType.LPUTF8Str)] string id);
    [DllImport(Library, EntryPoint = "qb_secret_free")] private static extern void Release(IntPtr bytes, nuint length);

    public byte[]? Load(Guid id)
    {
        int status = Read(paths.Credentials, id.ToString("D"), out var pointer, out var length);
        if (status == 1) return null;
        if (status != 0) throw new IOException("인증 파일을 보호하거나 열 수 없습니다. 다시 연결하세요.");
        try
        {
            byte[] data = new byte[checked((int)length)];
            if (data.Length != 0) Marshal.Copy(pointer, data, 0, data.Length);
            return data;
        }
        finally { Release(pointer, length); }
    }
    public void Save(Guid id, byte[] data)
    {
        if (data.Length > 1_048_576) throw new IOException("인증 파일이 너무 큽니다.");
        if (Write(paths.Credentials, id.ToString("D"), data, (nuint)data.Length) != 0)
            throw new IOException("인증 파일을 보호하거나 열 수 없습니다. 다시 연결하세요.");
    }
    public void Delete(Guid id)
    {
        if (Remove(paths.Credentials, id.ToString("D")) != 0) throw new IOException("인증 파일을 삭제할 수 없습니다.");
    }
}
