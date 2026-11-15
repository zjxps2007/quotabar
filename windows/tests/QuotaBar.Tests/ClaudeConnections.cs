using System.Runtime.Loader;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text.Json.Nodes;
using QuotaBar.Core;
using QuotaBar.Windows;

internal static class ClaudeConnections
{
    private static JsonObject Read(string path) => JsonNode.Parse(File.ReadAllBytes(path))!.AsObject();

    public static void Recovery(Suite suite)
    {
        var paths = new AppPaths(Path.Combine(suite.Paths.Root, "recovery"));
        var config = Directory.CreateDirectory(Path.Combine(suite.Paths.Root, "external recovery")).FullName;
        var service = new ClaudeConnection(paths);
        var old = service.Install(config, Fixtures.Helper, "powershell");
        var settings = Path.Combine(config, "settings.json");
        var pending = Path.Combine(paths.Profiles, old.Id, "profile-pending.json");
        var before = File.ReadAllBytes(settings);
        using (new FileStream(settings, FileMode.Open, FileAccess.Read, FileShare.Read))
        {
            bool refused = false;
            try { service.Install(config, Fixtures.Helper, "powershell"); }
            catch (Exception error) when (error is IOException or UnauthorizedAccessException) { refused = true; }
            Suite.Check(refused, "Installing over locked Claude settings should fail");
        }
        Suite.Check(before.SequenceEqual(File.ReadAllBytes(settings)) && service.Load(old.Id).Generation == old.Generation);

        var next = old with { Generation = Guid.NewGuid().ToString() };
        next = next with { Command = old.Command.Replace(old.Generation, next.Generation) };
        PrivateFiles.Write(pending, Json.Bytes(next));
        Suite.Check(service.Load(old.Id).Generation == old.Generation && !File.Exists(pending));
        PrivateFiles.Write(pending, Json.Bytes(next));
        var changed = Read(settings);
        changed["statusLine"]!["command"] = next.Command;
        File.WriteAllText(settings, changed.ToJsonString());
        Suite.Check(service.Load(old.Id).Generation == next.Generation && !File.Exists(pending));
        var disconnected = next with { Generation = Guid.NewGuid().ToString(), Command = "" };
        PrivateFiles.Write(pending, Json.Bytes(disconnected));
        Suite.Check(service.Load(old.Id).Generation == next.Generation);
        PrivateFiles.Write(pending, Json.Bytes(disconnected));
        changed.Remove("statusLine");
        changed["theme"] = "untouched";
        File.WriteAllText(settings, changed.ToJsonString());
        Suite.Check(service.Load(old.Id).Generation == disconnected.Generation && service.Fetch(config).State == "disconnected");
        Suite.Check(Read(settings)["theme"]!.GetValue<string>() == "untouched" && !File.Exists(pending));

        var info = new FileInfo(settings);
        foreach (bool protect in new[] { false, true })
        {
            var acl = info.GetAccessControl();
            acl.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(WellKnownSidType.BuiltinUsersSid, null),
                FileSystemRights.Read, AccessControlType.Allow));
            acl.SetAccessRuleProtection(protect, true);
            info.SetAccessControl(acl);
            string Dacl() => info.GetAccessControl().GetSecurityDescriptorSddlForm(AccessControlSections.Access);
            string beforeDacl = Dacl();
            var bound = service.Install(config, Fixtures.Helper, "powershell");
            Suite.Check(Dacl() == beforeDacl);
            Suite.Check(service.Uninstall(bound.Id) && Dacl() == beforeDacl);
        }
    }

    public static void Compare(string referenceDirectory)
    {
        var suite = new Suite();
        var reference = new AssemblyLoadContext("original Claude connection", isCollectible: true);
        var environment = ClaudeConnection.ApiKeys.ToDictionary(k => k, Environment.GetEnvironmentVariable);
        try
        {
            foreach (string key in environment.Keys) Environment.SetEnvironmentVariable(key, null);
            string app = Path.GetFullPath(referenceDirectory);
            reference.LoadFromAssemblyPath(Path.Combine(app, "QuotaBar.Core.dll"));
            var assembly = reference.LoadFromAssemblyPath(Path.Combine(app, "QuotaBar.Windows.dll"));
            var originalType = assembly.GetType("QuotaBar.Windows.ClaudeConnection", throwOnError: true)!;
            var paths = Activator.CreateInstance(assembly.GetType("QuotaBar.Windows.AppPaths", true)!, suite.Paths.Root)!;
            dynamic original = Activator.CreateInstance(originalType, paths)!;
            var native = new ClaudeConnection(suite.Paths);
            string config = Directory.CreateDirectory(Path.Combine(suite.Paths.Root, "한글 ßı 𐐨 user's config")).FullName;
            string file = Path.Combine(config, "settings.json");
            string id = ClaudeConnection.Id(config);
            Suite.Check((string?)originalType.GetMethod("Id")!.Invoke(null, [config + Path.DirectorySeparatorChar]) == id);
            const string script = "[Console]::Out.Write('원래 명령')";
            File.WriteAllText(file, new JsonObject { ["theme"] = "dark", ["statusLine"] = new JsonObject {
                ["type"] = "command", ["command"] = script, ["padding"] = 3 } }.ToJsonString());
            var info = new FileInfo(file);
            var acl = info.GetAccessControl();
            acl.SetAccessRuleProtection(true, true);
            info.SetAccessControl(acl);
            string FileDacl() => info.GetAccessControl().GetSecurityDescriptorSddlForm(AccessControlSections.Access);
            dynamic first = original.Install(config, Fixtures.Helper, "powershell");
            var imported = native.Load(id);
            Suite.Check(imported.Command == (string)first.Command && imported.Original!["command"]!.GetValue<string>() == script);
            var session = Guid.NewGuid();
            var reset = DateTimeOffset.UtcNow.AddDays(1).ToUnixTimeSeconds();
            byte[] Sample(double used) => Json.Bytes(new { session_id = session, rate_limits = new {
                five_hour = new { used_percentage = used, resets_at = reset } } });
            original.Capture(first, Sample(24.5));
            var fetched = native.Fetch(config);
            Suite.Check(fetched.State == "snapshot" && fetched.Windows.Single().Remaining == 75.5);
            var observed = fetched.ObservedAt;
            native.Capture(imported, Sample(24.5));
            Suite.Check((DateTimeOffset)original.Latest(first).ObservedAt == observed);

            // Set explicit protection on the current file after the original app's replacement.
            acl = info.GetAccessControl();
            acl.SetAccessRuleProtection(true, true);
            info.SetAccessControl(acl);
            string aclBefore = FileDacl();
            var next = native.Install(config, Fixtures.Helper, "powershell");
            Suite.Check(FileDacl() == aclBefore, "Rust reconnect changed the protected CLI DACL");
            Suite.Check(next.Command == imported.Command.Replace(imported.Generation, next.Generation));
            native.Capture(imported, Sample(99));
            Suite.Check(native.Latest(next) == null && (string)original.Fetch(config).State == "waiting");
            native.Capture(next, Sample(5.5));
            Suite.Check((double)original.Fetch(config).Windows[0].Remaining == 94.5);
            var settings = Read(file);
            settings["statusLine"]!["padding"] = 12;
            settings["theme"] = "light";
            File.WriteAllText(file, settings.ToJsonString());
            Suite.Check((bool)original.Uninstall(id) && native.Fetch(config).State == "disconnected");
            original.Install(config, Fixtures.Helper, "powershell");
            aclBefore = FileDacl();
            Suite.Check(native.Uninstall(id));
            settings = Read(file);
            Suite.Check(settings["statusLine"]!["command"]!.GetValue<string>() == script
                && settings["statusLine"]!["padding"]!.GetValue<int>() == 12
                && settings["theme"]!.GetValue<string>() == "light", "CLI command, padding or theme changed during restore");
            Suite.Check(FileDacl() == aclBefore, "Rust disconnect changed the CLI DACL");
            Console.WriteLine("PASS original/Rust Claude profile ID, wrapper command, snapshot round trips, reconnect, stale callback isolation, field-level restore and original DACL");
        }
        finally
        {
            foreach (var (key, value) in environment) Environment.SetEnvironmentVariable(key, value);
            reference.Unload();
            suite.Cleanup();
        }
    }
}
