using QuotaBar.Windows;
if (args.Length != 4 || args[0] != "--claude-statusline") return 1;
try { return await new ClaudeConnection(new AppPaths(args[3])).RunAsync(args[1], args[2], Console.OpenStandardInput(), Console.OpenStandardOutput(), Console.OpenStandardError()); }
catch { return 1; }
