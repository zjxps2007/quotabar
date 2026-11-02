using System.Diagnostics;
using System.IO.Compression;
using System.Reflection;
using System.Security.Cryptography;
using Microsoft.Win32;
using QuotaBar.Core;
using QuotaBar.Windows;

internal static class UpdateIntegration
{
    // Invoked only by the isolated CI fixture job. It builds both versions with a disposable key.
    public static async Task Run(string mode, string oldPackage, string oldSetup, string assetPath, string keyPath)
    {
        string root = Path.Combine(Path.GetTempPath(), "QuotaBar.Update.Tests-" + Guid.NewGuid()); PrivateFiles.EnsureDirectory(root);
        string target = Path.Combine(root, "QuotaBar 한글 with spaces"), data = Path.Combine(root, "data");
        bool installed = mode == "installed", failure = mode == "rollback";
        Process? monitor = null, helper = null;
        try
        {
            if (installed)
            {
                var setup = new ProcessStartInfo(oldSetup) { UseShellExecute = false };
                foreach (string argument in new[] { "/SP-", "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/DIR=" + target, "/TASKS=" }) setup.ArgumentList.Add(argument);
                using var process = Process.Start(setup)!; await process.WaitForExitAsync(); Suite.Check(process.ExitCode == 0);
            }
            else UpdateInstaller.CopyTree(oldPackage, target, CancellationToken.None);
            File.WriteAllText(Path.Combine(target, "user-file.txt"), "keep me");
            var paths = new AppPaths(data); new SettingsStore(paths).Save(new Settings { AutomaticUpdates = false, AutoRefresh = false, Appearance = "dark" });
            var start = new ProcessStartInfo(Path.Combine(target, "QuotaBar.Monitor.exe")) { UseShellExecute = false, WorkingDirectory = target };
            foreach (string argument in new[] { "--background", "--data-dir", data }) start.ArgumentList.Add(argument);
            monitor = Process.Start(start)!;
            await Until(() => { using var lease = new SingleInstance(paths); return !lease.IsOwner; }, "old monitor lease");
            Suite.Check(!monitor.HasExited);
            string current = AssemblyName.GetAssemblyName(Path.Combine(target, "QuotaBar.Monitor.dll")).Version!.ToString(3);
            const string version = "0.1.1";
            string directory = Path.Combine(UpdateInstaller.UpdatesRoot(paths), Guid.NewGuid().ToString("D")); PrivateFiles.EnsureDirectory(directory);
            string name = $"QuotaBar-{version}-Windows-x64" + (installed ? "-Setup.exe" : ".zip");
            string payload = Path.Combine(directory, name); File.Copy(assetPath, payload);
            if (failure)
            {
                using var zip = ZipFile.Open(payload, ZipArchiveMode.Update);
                zip.GetEntry("QuotaBar-Windows-x64/QuotaBar.Monitor.exe")!.Delete();
                using var writer = new StreamWriter(zip.CreateEntry("QuotaBar-Windows-x64/QuotaBar.Monitor.exe").Open());
                writer.Write("synthetic startup failure");
            }
            using var input = File.OpenRead(payload); string hash = Convert.ToHexString(await SHA256.HashDataAsync(input)).ToLowerInvariant(); input.Close();
            var asset = new ReleaseAsset(name, new Uri($"{ReleaseFeed.Repository}/releases/download/v{version}/{name}"), new FileInfo(payload).Length, hash);
            var release = new AppRelease("v" + version, version, new Uri($"{ReleaseFeed.Repository}/releases/tag/v{version}"), asset);
            byte[] manifest = Json.Bytes(new { schemaVersion = 1, version, build = 7, assets = new[] { new { name, bytes = asset.Bytes, sha256 = hash } } });
            using var key = ECDsa.Create(); key.ImportFromPem(File.ReadAllText(keyPath));
            byte[] signature = key.SignData(manifest, HashAlgorithmName.SHA256, DSASignatureFormat.Rfc3279DerSequence);
            PrivateFiles.Write(Path.Combine(directory, "update-manifest.json"), manifest); PrivateFiles.Write(Path.Combine(directory, "update-manifest.sig"), signature);
            var staged = new StagedUpdate(UpdateSignature.Verify(manifest, signature, UpdateInstaller.PublicKey, release, current), directory);
            string job = await UpdateInstaller.Prepare(staged, paths, target, current, CancellationToken.None);
            var plan = Json.Read<WindowsUpdatePlan>(File.ReadAllBytes(job)) with { ParentPid = monitor.Id, ParentStarted = monitor.StartTime.ToUniversalTime().Ticks };
            PrivateFiles.Write(job, Json.Bytes(plan));
            helper = UpdateInstaller.LaunchHelper(job);
            await Until(() => UpdateInstaller.ReadResult(Path.Combine(directory, "installer-ready.json")) is { Success: true }, "helper ready");
            Suite.Check(!helper.HasExited && !monitor.HasExited);
            monitor.Kill(); await monitor.WaitForExitAsync(); // Release the real old executable and its lease.
            using var timeout = new CancellationTokenSource(TimeSpan.FromMinutes(3)); await helper.WaitForExitAsync(timeout.Token);
            var result = UpdateInstaller.ReadResult(UpdateInstaller.ResultPath(paths));
            Suite.Check(result != null && result.Success == !failure, result?.Message ?? "No install result.");
            Suite.Check((helper.ExitCode == 0) == !failure);
            string expected = failure ? current : version;
            Suite.Check(AssemblyName.GetAssemblyName(Path.Combine(target, "QuotaBar.Monitor.dll")).Version!.ToString(3) == expected);
            Suite.Check(File.ReadAllText(Path.Combine(target, "user-file.txt")) == "keep me");
            var settings = new SettingsStore(paths).Load(); Suite.Check(settings.Appearance == "dark" && !settings.AutomaticUpdates && !settings.AutoRefresh);
            await Until(() => { using var lease = new SingleInstance(paths); return !lease.IsOwner; }, "restarted monitor lease");
            if (installed)
            {
                using var registration = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\QuotaBar.Windows_is1");
                Suite.Check((string?)registration?.GetValue("DisplayVersion") == version);
            }
            Console.WriteLine($"PASS automatic update {mode}: detached helper, signed payload, version {expected}, restart, settings and user files preserved");
        }
        finally
        {
            foreach (var process in Process.GetProcessesByName("QuotaBar.Monitor"))
                using (process) try { if (string.Equals(process.MainModule?.FileName, Path.Combine(target, "QuotaBar.Monitor.exe"), StringComparison.OrdinalIgnoreCase))
                    { process.Kill(); await process.WaitForExitAsync(); } } catch (InvalidOperationException) { }
            if (helper != null) { if (!helper.HasExited) { helper.Kill(true); await helper.WaitForExitAsync(); } helper.Dispose(); }
            monitor?.Dispose();
            if (installed && File.Exists(Path.Combine(target, "unins000.exe")))
            {
                var uninstall = new ProcessStartInfo(Path.Combine(target, "unins000.exe")) { UseShellExecute = false };
                foreach (string argument in new[] { "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" }) uninstall.ArgumentList.Add(argument);
                using var process = Process.Start(uninstall)!; await process.WaitForExitAsync();
            }
            try { Directory.Delete(root, true); } catch (IOException) { }
        }
    }
    private static async Task Until(Func<bool> condition, string operation)
    {
        var deadline = DateTime.UtcNow.AddSeconds(30);
        while (!condition()) { if (DateTime.UtcNow > deadline) throw new TimeoutException(operation); await Task.Delay(100); }
    }
}
