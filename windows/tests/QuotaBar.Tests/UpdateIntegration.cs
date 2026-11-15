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
        bool resident = mode.StartsWith("resident-", StringComparison.Ordinal);
        string scenario = resident ? mode["resident-".Length..] : mode;
        bool installed = scenario == "installed", failure = scenario == "rollback";
        Process? monitor = null;
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
            var paths = new AppPaths(data); new SettingsStore(paths).Save(new Settings { AutomaticUpdates = resident, AutoRefresh = false, Appearance = "dark" });
            var start = new ProcessStartInfo(Path.Combine(target, "QuotaBar.Monitor.exe")) { UseShellExecute = false, WorkingDirectory = target };
            foreach (string argument in new[] { "--background", "--data-dir", data }) start.ArgumentList.Add(argument);
            string current = AssemblyName.GetAssemblyName(Path.Combine(target, "QuotaBar.Monitor.dll")).Version!.ToString(3);
            const string version = "0.1.1";
            string directory = resident ? Path.Combine(root, "fixture") : Path.Combine(UpdateInstaller.UpdatesRoot(paths), Guid.NewGuid().ToString("D"));
            PrivateFiles.EnsureDirectory(directory);
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
            if (resident)
            {
                PrivateFiles.Write(Path.Combine(directory, "feed.json"), Json.Bytes(new {
                    tag_name = release.Tag, html_url = release.Page, draft = false, prerelease = false,
                    assets = new[] { new { name, size = asset.Bytes, state = "uploaded", digest = "sha256:" + hash, browser_download_url = asset.Url } }
                }));
                start.Environment["QUOTABAR_TEST_RELEASE_DIRECTORY"] = directory;
            }
            monitor = Process.Start(start)!;
            await Until(() => { using var lease = new SingleInstance(paths); return !lease.IsOwner; }, "old monitor lease");
            Suite.Check(!monitor.HasExited);
            if (resident)
            {
                await ObserveResident(paths, monitor);
            }
            else
            {
                var staged = new StagedUpdate(UpdateSignature.Verify(manifest, signature, UpdateInstaller.PublicKey, release, current), directory);
                await DriveInstaller(staged, signature, paths, target, current, monitor, failure);
            }
            var result = UpdateInstaller.ReadResult(UpdateInstaller.ResultPath(paths));
            Suite.Check(result != null && result.Success == !failure, result?.Message ?? "No install result.");
            string expected = failure ? current : version;
            Suite.Check(AssemblyName.GetAssemblyName(Path.Combine(target, "QuotaBar.Monitor.dll")).Version!.ToString(3) == expected);
            Suite.Check(File.ReadAllText(Path.Combine(target, "user-file.txt")) == "keep me");
            var settings = new SettingsStore(paths).Load(); Suite.Check(settings.Appearance == "dark" && settings.AutomaticUpdates == resident && !settings.AutoRefresh);
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
            monitor?.Dispose();
            if (installed && File.Exists(Path.Combine(target, "unins000.exe")))
            {
                var uninstall = new ProcessStartInfo(Path.Combine(target, "unins000.exe")) { UseShellExecute = false };
                foreach (string argument in new[] { "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" }) uninstall.ArgumentList.Add(argument);
                using var process = Process.Start(uninstall)!; await process.WaitForExitAsync();
            }
            await Until(() =>
            {
                try { Directory.Delete(root, true); return true; }
                catch (DirectoryNotFoundException) { return true; }
                catch (IOException) { return false; }
                catch (UnauthorizedAccessException) { return false; }
            }, "release temporary update files");
        }
    }
    private static async Task ObserveResident(AppPaths paths, Process monitor)
    {
        // No calls to the installer API here. The old monitor discovers the
        // signed fixture on its idle timer, prepares it and exits under its own PID.
        int pid = monitor.Id;
        long started = monitor.StartTime.ToUniversalTime().Ticks;
        string updates = UpdateInstaller.UpdatesRoot(paths);
        string? job = null;
        await Until(() => Directory.Exists(updates) && (job = Directory.EnumerateDirectories(updates)
            .Select(dir => Path.Combine(dir, "install.json")).FirstOrDefault(File.Exists)) != null,
            "resident prepared its own signed job", 180);
        var plan = Json.Read<WindowsUpdatePlan>(File.ReadAllBytes(job!));
        Suite.Check(plan.ParentPid == pid && plan.ParentStarted == started);
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(60));
        await monitor.WaitForExitAsync(timeout.Token);
        Suite.Check(monitor.ExitCode == 0);
        await Until(() => UpdateInstaller.ReadResult(UpdateInstaller.ResultPath(paths)) != null, "resident update result", 180);
        // The helper publishes the result before finishing backup cleanup or
        // starting the restored monitor. Wait for this fixture's helper only.
        string helper = Path.Combine(Path.GetDirectoryName(job!)!, "Helper", "QuotaBar.StatusLine.exe");
        foreach (var process in Process.GetProcessesByName("QuotaBar.StatusLine"))
        {
            using (process)
            {
                string? executable;
                try { executable = process.MainModule?.FileName; }
                catch (InvalidOperationException) { continue; }
                catch (System.ComponentModel.Win32Exception) { continue; }
                if (string.Equals(executable, helper, StringComparison.OrdinalIgnoreCase))
                    await process.WaitForExitAsync(timeout.Token);
            }
        }
    }

    private static async Task DriveInstaller(StagedUpdate staged, byte[] signature, AppPaths paths,
        string target, string current, Process monitor, bool failure)
    {
        string signatureFile = Path.Combine(staged.Directory, "update-manifest.sig");
        PrivateFiles.Write(signatureFile, new byte[signature.Length]);
        try
        {
            await UpdateInstaller.Prepare(staged, paths, target, current, CancellationToken.None);
            throw new InvalidDataException("Prepared an update with an invalid signature.");
        }
        catch (UsageException error) when (error.Code == "update_signature") { }
        Suite.Check(!Directory.Exists(Path.Combine(staged.Directory, "Helper")) && !File.Exists(Path.Combine(staged.Directory, "install.json")));
        PrivateFiles.Write(signatureFile, signature);
        string job = await UpdateInstaller.Prepare(staged, paths, target, current, CancellationToken.None);
        byte[] prepared = File.ReadAllBytes(job);
        bool rejected = false;
        try { await UpdateInstaller.Prepare(staged, paths, target, current, CancellationToken.None); }
        catch (IOException) { rejected = true; }
        Suite.Check(rejected && File.ReadAllBytes(job).SequenceEqual(prepared));
        var plan = Json.Read<WindowsUpdatePlan>(prepared);
        using var preparingProcess = Process.GetCurrentProcess();
        Suite.Check(plan.ParentPid == preparingProcess.Id && plan.ParentStarted == preparingProcess.StartTime.ToUniversalTime().Ticks);
        plan = plan with { ParentPid = monitor.Id, ParentStarted = monitor.StartTime.ToUniversalTime().Ticks };
        PrivateFiles.Write(job, Json.Bytes(plan));
        using var handoff = UpdateInstaller.Helper.Start(job);
        using var helper = Process.GetProcessById(handoff.ProcessId);
        try
        {
            await Until(handoff.IsReady, "helper ready");
            Suite.Check(!helper.HasExited && !monitor.HasExited);
            handoff.Detach();
            monitor.Kill(); await monitor.WaitForExitAsync();
            using var timeout = new CancellationTokenSource(TimeSpan.FromMinutes(3));
            await helper.WaitForExitAsync(timeout.Token);
            Suite.Check((helper.ExitCode == 0) == !failure);
        }
        finally { if (!helper.HasExited) { helper.Kill(true); await helper.WaitForExitAsync(); } }
    }

    private static async Task Until(Func<bool> condition, string operation, int seconds = 30)
    {
        var deadline = DateTime.UtcNow.AddSeconds(seconds);
        while (!condition()) { if (DateTime.UtcNow > deadline) throw new TimeoutException(operation); await Task.Delay(100); }
    }
}
