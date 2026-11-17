using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;

// Read Unicode link targets without the legacy WScript.Shell automation layer.
public static class ShortcutReader
{
    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellLinkW
    {
        void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int capacity, IntPtr findData, uint flags);
    }

    public static string ReadTarget(string path)
    {
        object link = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("00021401-0000-0000-C000-000000000046")));
        try
        {
            ((IPersistFile)link).Load(path, 0);
            var result = new StringBuilder(32768);
            ((IShellLinkW)link).GetPath(result, result.Capacity, IntPtr.Zero, 0);
            return result.ToString();
        }
        finally { Marshal.FinalReleaseComObject(link); }
    }
}
