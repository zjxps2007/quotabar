using System.Reflection;
using System.Runtime.Loader;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using QuotaBar.Core;
using QuotaBar.Windows;

internal static class SettingsCompatibility
{
    private static JsonNode Observe(Func<object> run)
    {
        try
        {
            var value = run();
            return new JsonObject { ["value"] = JsonSerializer.SerializeToNode(value, value.GetType(), Json.Options) };
        }
        catch (Exception error)
        {
            var failure = error is TargetInvocationException { InnerException: {} inner } ? inner : error;
            return new JsonObject { ["error"] = failure.GetType().Name,
                ["code"] = failure.GetType().GetProperty("Code")?.GetValue(failure)?.ToString() };
        }
    }

    private static void Same(JsonNode original, JsonNode native, string name) =>
        Suite.Check(JsonNode.DeepEquals(original, native), $"{name}: original {original}, Rust {native}");

    public static void Compare(string referenceDirectory)
    {
        var suite = new Suite();
        var reference = new AssemblyLoadContext("original Windows settings", isCollectible: true);
        try
        {
            string app = Path.GetFullPath(referenceDirectory);
            var core = reference.LoadFromAssemblyPath(Path.Combine(app, "QuotaBar.Core.dll"));
            var assembly = reference.LoadFromAssemblyPath(Path.Combine(app, "QuotaBar.Windows.dll"));
            var pathsType = assembly.GetType("QuotaBar.Windows.AppPaths", true)!;
            var storeType = assembly.GetType("QuotaBar.Windows.SettingsStore", true)!;
            var originalStore = Activator.CreateInstance(storeType, Activator.CreateInstance(pathsType, suite.Paths.Root))!;
            var originalLoad = storeType.GetMethod("Load")!;
            var nativeStore = new SettingsStore(suite.Paths);
            string file = Path.Combine(suite.Paths.Root, "settings.json");
            var id = Guid.Parse("59ce0a56-cda2-4eca-b909-10384b6df40c");
            string WithAccount(string extra) => "{\"accounts\":[{\"name\":\"한글\",\"identity\":\"cursor\",\"id\":\""
                + id.ToString("D").ToUpperInvariant() + "\"," + extra + "}]}";
            var fixtures = new List<(string Name, string? Text)> {
                ("absent", null), ("empty object", "{}"), ("bad syntax", "{broken"), ("null settings", "null"),
                ("defaults", """{"unknown":{"secret":[null,2]},"ACCOUNTS":[]}"""),
                ("fallbacks", """{"refreshSeconds":10,"appearance":null,"trayDisplayMode":"future"}"""),
                ("repeated properties", """{"REFRESHSECONDS":"900","refreshSeconds":60,"AutoRefresh":true,"autoRefresh":false}"""),
                ("quoted integer", """{"schemaVersion":"01","refreshSeconds":"+900"}"""),
                ("boolean as string", """{"autoRefresh":"false"}"""), ("future schema", """{"schemaVersion":2}"""),
                ("wrong schema type", """{"schemaVersion":1.0}"""), ("null accounts", """{"accounts":null}"""),
                ("null account element", """{"accounts":[null]}"""), ("empty account", """{"accounts":[{}]}"""),
                ("mixed-case enum", WithAccount("\"pRoViDeR\":\" gEmInI \",\"sourcePath\":null")),
                ("ordinal enum", WithAccount("\"provider\":2")), ("quoted ordinal", WithAccount("\"provider\":\"1\"")),
                ("enum set", WithAccount("\"provider\":\"Codex, Gemini\"")), ("unknown enum", WithAccount("\"provider\":\"future\"")),
                ("unknown ordinal", WithAccount("\"provider\":72")), ("partly missing account", WithAccount("\"enabled\":false")),
                ("braced GUID", WithAccount("\"showInTray\":false").Replace(id.ToString("D").ToUpperInvariant(), id.ToString("B"))),
                ("null id", WithAccount("\"id\":null")), ("nullable name", WithAccount("\"name\":null")),
                ("invalid old value", """{"autoRefresh":"bad","autoRefresh":true}"""),
                ("depth 64", """{"unknown":""" + new string('[', 63) + "0" + new string(']', 63) + "}"),
                ("depth 65", """{"unknown":""" + new string('[', 64) + "0" + new string(']', 64) + "}"),
                ("over file limit", new string(' ', 4 * 1024 * 1024) + "{}")
            };
            var repeated = new Account(id, Provider.Claude, "중복", "identity", "root");
            fixtures.Add(("duplicate ids", Encoding.UTF8.GetString(Json.Bytes(new Settings { Accounts = [repeated, repeated] }))));
            fixtures.Add(("over fifty", Encoding.UTF8.GetString(Json.Bytes(new Settings { Accounts =
                Enumerable.Range(0, 51).Select(_ => repeated with { Id = Guid.NewGuid() }).ToList() }))));
            foreach (var (name, contents) in fixtures)
            {
                if (contents != null) File.WriteAllText(file, contents);
                Same(Observe(() => originalLoad.Invoke(originalStore, null)!), Observe(nativeStore.Load), name);
                Suite.Check(contents == null ? !File.Exists(file) : File.ReadAllText(file) == contents, "Load rewrote " + name);
            }
            File.Delete(file);
            Directory.CreateDirectory(file);
            try { Same(Observe(() => originalLoad.Invoke(originalStore, null)!), Observe(nativeStore.Load), "directory in place of settings"); }
            finally { Directory.Delete(file); }

            // Feed both running monitors the same disabled import, so reconnecting it never makes a network request.
            string login = Path.Combine(suite.Paths.Root, "기존 로그인.json");
            byte[] auth = Fixtures.Auth(); File.WriteAllBytes(login, auth);
            var first = new Account(id, Provider.Codex, "사용자 이름", Credentials.Parse(auth, Provider.Codex).Identity,
                "previous", false, false, true);
            var other = new Account(Guid.NewGuid(), Provider.Gemini, "둘째", "another", "", false);
            var initial = new Settings { Accounts = [first, other] };
            string oldRoot = Path.Combine(suite.Paths.Root, "original"), newRoot = Path.Combine(suite.Paths.Root, "native");
            foreach (var root in new[] { oldRoot, newRoot }) new SettingsStore(new AppPaths(root)).Save(initial);
            var oldPaths = Activator.CreateInstance(pathsType, oldRoot)!;
            var oldStore = Activator.CreateInstance(storeType, oldPaths)!;
            var legacyInNew = Activator.CreateInstance(storeType, Activator.CreateInstance(pathsType, newRoot))!;
            using var original = (IDisposable)Activator.CreateInstance(assembly.GetType("QuotaBar.Windows.MonitorService", true)!, oldPaths, null, null)!;
            dynamic legacy = original;
            using var native = new MonitorService(new AppPaths(newRoot));
            void Both(string step)
            {
                var expected = Observe(() => originalLoad.Invoke(oldStore, null)!);
                Same(expected, Observe(() => originalLoad.Invoke(legacyInNew, null)!), "legacy reads Rust " + step);
                Same(expected, Observe(new SettingsStore(new AppPaths(oldRoot)).Load), "Rust reads legacy " + step);
                Same(expected, Observe(() => native.Settings), "live Rust " + step);
            }
            legacy.Preferences(1800, "dark", false, false, "icon", false, false);
            native.Preferences(1800, "dark", false, false, "icon", false, false);
            legacy.Preferences(60, "light", true, true, null, null, null);
            native.Preferences(60, "light", true, true); Both("preferences from older dashboard");
            legacy.Edit(id, "　한글 𐐨　", false, false); native.Edit(id, "　한글 𐐨　", false, false); Both("trimmed edit");
            string tooLong = string.Concat(Enumerable.Repeat("𐐨", 51));
            Same(Observe(() => { legacy.Edit(id, tooLong, true, true); return true; }),
                Observe(() => { native.Edit(id, tooLong, true, true); return true; }), "UTF-16 name limit");
            var oldProvider = Enum.Parse(core.GetType("QuotaBar.Core.Provider", true)!, "Codex");
            ((Task)legacy.Import((dynamic)oldProvider, login)).GetAwaiter().GetResult();
            native.Import(Provider.Codex, login).GetAwaiter().GetResult(); Both("reconnect existing workspace");
            Suite.Check(native.Settings.Accounts[^1].Id == id && !native.Settings.Accounts[^1].Enabled &&
                !native.Settings.Accounts[^1].ShowInTray && !native.Settings.Accounts[^1].AppManagedLogin);
            legacy.Remove(other.Id); native.Remove(other.Id); Both("removal and restart");
            using (new FileStream(Path.Combine(newRoot, "settings.json"), FileMode.Open, FileAccess.Read, FileShare.Read))
            {
                var before = native.Settings;
                bool refused = false;
                try { native.Preferences(900, "dark", false, false); }
                catch (Exception error) when (error is IOException or UnauthorizedAccessException) { refused = true; }
                Suite.Check(refused && ReferenceEquals(before, native.Settings), "A failed write replaced the live settings");
            }
            Both("failed disk write");
            void CompareImport(Provider provider, string source, string scenario)
            {
                var referenceProvider = Enum.ToObject(core.GetType("QuotaBar.Core.Provider", true)!, (int)provider);
                var expected = Observe(() => { ((Task)legacy.Import((dynamic)referenceProvider, source)).GetAwaiter().GetResult(); return true; });
                var actual = Observe(() => { native.Import(provider, source).GetAwaiter().GetResult(); return true; });
                Suite.Check(expected["error"] != null, $"Reference accepted invalid import: {scenario}");
                Same(expected, actual, scenario);
            }
            CompareImport((Provider)41, login, "unknown provider");
            CompareImport(Provider.Claude, login, "Claude requires a directory");
            CompareImport(Provider.Codex, "relative.json", "relative path");
            CompareImport(Provider.Codex, @"\\fixture.invalid\share\auth.json", "UNC path");
            var bad = Path.Combine(suite.Paths.Root, "invalid-or-missing.json");
            CompareImport(Provider.Codex, bad, "missing source");
            foreach (var (reason, contents) in new[] { ("invalid JSON", "{"u8.ToArray()), ("missing token", "{}"u8.ToArray()),
                ("missing identity", """{"tokens":{"access_token":"fixture"}}"""u8.ToArray()), ("size limit", new byte[1048577]) })
            {
                File.WriteAllBytes(bad, contents);
                CompareImport(Provider.Codex, bad, reason);
            }
            var oldVault = new SecretVault(new AppPaths(oldRoot));
            var newVault = new SecretVault(new AppPaths(newRoot));
            using (var lockedOld = File.Open(Path.Combine(oldRoot, "settings.json"), FileMode.Open, FileAccess.Read, FileShare.Read))
            using (var lockedNew = File.Open(Path.Combine(newRoot, "settings.json"), FileMode.Open, FileAccess.Read, FileShare.Read))
            {
                File.WriteAllBytes(login, Fixtures.Auth(access: "replacement"));
                CompareImport(Provider.Codex, login, "rollback reconnected credentials");
                Suite.Check(oldVault.Load(id)!.SequenceEqual(auth) && newVault.Load(id)!.SequenceEqual(auth));
                File.WriteAllBytes(login, Fixtures.Auth(account: "new-workspace"));
                CompareImport(Provider.Codex, login, "rollback new credentials");
                foreach (var root in new[] { oldRoot, newRoot })
                    Suite.Check(Directory.GetFiles(new AppPaths(root).Credentials, "*.bin").Length == 1);
            }
            Both("credential transaction rollback");
            Console.WriteLine("PASS original and Rust credential imports: 9 rejected providers, paths and files; protected vault rollback for existing and new accounts");
            CompareCache(assembly, pathsType, suite, initial);
            Console.WriteLine($"PASS original/Rust Windows settings: {fixtures.Count} disk fixtures, cross-reading, preferences, edit, reconnect and removal");
        }
        finally { reference.Unload(); suite.Cleanup(); }
    }

    private static void CompareCache(Assembly assembly, Type pathsType, Suite suite, Settings settings)
    {
        var paths = new AppPaths(Path.Combine(suite.Paths.Root, "cached usage"));
        var oldPaths = Activator.CreateInstance(pathsType, paths.Root)!;
        var oldMonitor = assembly.GetType("QuotaBar.Windows.MonitorService", true)!;
        new SettingsStore(paths).Save(settings);
        string file = Path.Combine(paths.Root, "snapshots.json");
        string id = settings.Accounts[0].Id.ToString();
        string Key(string key, string value) => JsonSerializer.Serialize(key) + ":" + value;
        string Cache(string value) => "{" + Key(id.ToUpperInvariant(), value) + "}";
        const string sample = """{"FETCHEDAT":"2025-03-01T14:15:01.1234567+09:30","WINDOWS":[{"TITLE":"5시간","remaining":"91.125","resetAt":"2025-04-01T00:00Z"},{"remaining":12.000000000000002}],"email":null,"plan":"Plus","observedAt":"2025-03-01T13:11:00.0000010-04:00","lowest":0,"extra":"discard"}""";
        var cases = new List<(string Name, string? Data)> {
            ("absent", null), ("empty document", "{}"), ("null root", "null"), ("bad root", "[]"),
            ("ticks, offset, precision, quoted number", Cache(sample)), ("empty windows", Cache("""{"windows":[]}""")),
            ("windows absent", Cache("{}")), ("null windows", Cache("""{"windows":null}""")),
            ("no fetched time", Cache("""{"windows":[{}]}""")), ("bad GUID", "{" + Key("invalid", sample) + "}"),
            ("removed account", "{" + Key(Guid.NewGuid().ToString(), sample) + "}"),
            ("invalid removed account", "{" + Key(id, sample) + "," + Key(Guid.NewGuid().ToString(), """{"fetchedAt":"bad","windows":[]}""") + "}"),
            ("duplicate id", "{" + Key(id, sample) + "," + Key(id.ToUpperInvariant(), """{"windows":null}""") + "}"),
            ("malformed", "{partial"), ("over size limit", new string(' ', 2 * 1024 * 1024) + "{}")
        };
        foreach (var date in new[] { "2024-02-29", "2024-03-01T13:41", "2024-03-01T13:41+09", "2024-03-01T13:41:02.1234567890123456Z",
            "2024-03-01T13:41:02.12345678901234567Z", "2023-02-29", "0001-01-01T00:00:00+14:00",
            "9999-12-31T23:59:59-14:00", "2024-01-01T00:00:00+14:01", "2024-01-01t00:00:00z" })
            cases.Add((date, Cache("""{"windows":[],"fetchedAt":""" + JsonSerializer.Serialize(date) + "}")));
        object View(object state)
        {
            var type = state.GetType();
            return new { snapshots = type.GetProperty("Snapshots")!.GetValue(state), message = type.GetProperty("Message")!.GetValue(state) };
        }
        JsonNode Original() => Observe(() => {
            using var monitor = (IDisposable)Activator.CreateInstance(oldMonitor, oldPaths, null, null)!;
            return View(oldMonitor.GetMethod("State")!.Invoke(monitor, null)!);
        });
        JsonNode Native() => Observe(() => { using var monitor = new MonitorService(paths); return View(monitor.State()); });
        foreach (var (name, data) in cases)
        {
            if (data != null) File.WriteAllText(file, data);
            Same(Original(), Native(), "snapshot " + name);
            Suite.Check(data == null ? !File.Exists(file) : File.ReadAllText(file) == data, "Loading changed cache " + name);
        }
        File.WriteAllText(file, Cache(sample));
        var expected = Original();
        using (var monitor = new MonitorService(paths)) monitor.Remove(settings.Accounts[1].Id);
        Same(expected, Original(), "legacy reads Rust snapshot after rewrite");
        string persisted = File.ReadAllText(file);
        Suite.Check(!persisted.Contains("discard") && JsonNode.Parse(persisted)![id]!["lowest"]!.GetValue<double>() == 12.000000000000002);
        Console.WriteLine($"PASS original/Rust Windows usage cache: {cases.Count} fixtures, timestamp and quota precision, retention and cross-version rewrite");
    }
}
