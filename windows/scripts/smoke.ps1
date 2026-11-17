param([string]$ApplicationDirectory, [switch]$VerifyForeground)
. "$PSScriptRoot\common.ps1"
if (-not $ApplicationDirectory) { $ApplicationDirectory = Join-Path $RepoRoot 'artifacts\QuotaBar-Windows-x64' }
$exe = Join-Path $ApplicationDirectory 'QuotaBar.Monitor.exe'
$result = Join-Path $RepoRoot 'artifacts\windows\packaged-probe.json'
$probe = [Diagnostics.ProcessStartInfo]::new($exe)
$probe.UseShellExecute = $false
$probe.CreateNoWindow = $true
$probe.Arguments = '--probe "' + $result + '"'
$uiError = Join-Path ([IO.Path]::GetTempPath()) ("quotabar-probe-" + [guid]::NewGuid() + ".log")
$probe.Environment['QUOTABAR_PROBE_UI_ERROR'] = $uiError
[void]$probe.Environment.Remove('DOTNET_ROOT')
[void]$probe.Environment.Remove('DOTNET_ROOT_X64')
[void]$probe.Environment.Remove('DOTNET_ROOT(x86)')
$started = [DateTime]::UtcNow
$process = [Diagnostics.Process]::Start($probe)
# Dashboard captures have individual deadlines in the monitor. Leave the software
# renderer time for both helper launches and all 26 captures across both themes.
$finished = $process.WaitForExit(120000)
if (-not $finished) { $process.Kill(); [void]$process.WaitForExit(5000) }
if ([IO.File]::Exists($uiError)) {
    try { throw ("Packaged Dashboard failed: " + [IO.File]::ReadAllText($uiError)) }
    finally { [IO.File]::Delete($uiError) }
}
if (-not $finished) {
    $captured = @(Get-ChildItem -LiteralPath (Split-Path -Parent $result) -Filter '*.png' -File |
        Where-Object LastWriteTimeUtc -ge $started | Sort-Object LastWriteTimeUtc)
    throw "Packaged UI probe timed out after 120 seconds. Saved $($captured.Count) PNGs. Last capture: $($captured.Name | Select-Object -Last 1)"
}
if ($process.ExitCode -ne 0) { throw "Packaged app exited with code $($process.ExitCode). $(Get-Content -LiteralPath $result -Raw -ErrorAction SilentlyContinue)" }
$data = Get-Content -LiteralPath $result -Raw | ConvertFrom-Json
if (-not $data.passed -or $data.monitorPid -ne $process.Id -or $data.dashboardRunning -or $data.wpfLoadedInMonitor -or $data.windowsFormsLoadedInMonitor -ne $false) { throw "Packaged probe failed: $($data | ConvertTo-Json -Compress)" }
$runtimeFiles = @(Get-ChildItem -LiteralPath $ApplicationDirectory -File -Recurse | Where-Object {
    $_.Name -match '^(coreclr|clrjit|hostfxr|hostpolicy|quotabar_native)\.dll$|\.runtimeconfig\.json$'
})
if ($runtimeFiles.Count -or $data.clrLoadedInMonitor -ne $false) { throw 'Managed runtime or FFI adapter in the Windows application.' }
$nativePrograms = @('QuotaBar.Monitor.exe', 'Dashboard\QuotaBar.Dashboard.exe', 'StatusLine\QuotaBar.StatusLine.exe')
foreach ($relative in $nativePrograms) {
    $stream = [IO.File]::OpenRead((Join-Path $ApplicationDirectory $relative))
    try {
        $pe = [System.Reflection.PortableExecutable.PEReader]::new($stream)
        try {
            if ($pe.PEHeaders.CoffHeader.Machine -ne 'Amd64' -or $pe.PEHeaders.PEHeader.CorHeaderTableDirectory.Size -ne 0) { throw "Expected a native x64 program: $relative" }
        } finally { $pe.Dispose() }
    } finally { $stream.Dispose() }
}
foreach ($relative in $nativePrograms) {
    $marker = [IO.Path]::ChangeExtension((Join-Path $ApplicationDirectory $relative), '.dll')
    $stream = [IO.File]::OpenRead($marker)
    try {
        $pe = [System.Reflection.PortableExecutable.PEReader]::new($stream)
        try {
            $metadata = [System.Reflection.Metadata.PEReaderExtensions]::GetMetadataReader($pe)
            if ($metadata.MethodDefinitions.Count -ne 0) { throw "Expected a version-only updater marker: $relative" }
        } finally { $pe.Dispose() }
    } finally { $stream.Dispose() }
}
$updateResult = Join-Path $RepoRoot 'artifacts\windows\packaged-update-probe.json'
$probe.Arguments = '--update-manager-probe "' + $updateResult + '"'
$updateMonitor = [Diagnostics.Process]::Start($probe)
try {
    if (-not $updateMonitor.WaitForExit(15000)) { $updateMonitor.Kill(); throw 'Packaged update check timed out.' }
    $update = Get-Content -LiteralPath $updateResult -Raw | ConvertFrom-Json
    if ($updateMonitor.ExitCode -ne 0 -or -not $update.passed -or $update.monitorPid -ne $updateMonitor.Id -or -not $update.manualCheckWhileAutomaticUpdatesDisabled -or $update.updateChecks.Count -ne 6) {
        throw "Packaged manual update check failed: $($update | ConvertTo-Json -Compress)"
    }
    $data | Add-Member -NotePropertyName manualCheckWhileAutomaticUpdatesDisabled -NotePropertyValue $true
    $data | Add-Member -NotePropertyName updateChecks -NotePropertyValue $update.updateChecks
} finally { $updateMonitor.Dispose() }
# Exercise the normal single-instance entry point with empty, isolated settings.
$residentRoot = Join-Path $RepoRoot ('artifacts\windows\smoke-resident-' + [Guid]::NewGuid().ToString('N'))
# The monitor checks the data directory's owner. An elevated runner's default
# owner can be Administrators, even when this process creates the directory.
$owner = [Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = [Security.AccessControl.DirectorySecurity]::new()
$acl.SetOwner($owner)
$acl.SetAccessRuleProtection($true, $false)
$acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($owner, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
[IO.FileSystemAclExtensions]::Create([IO.DirectoryInfo]::new($residentRoot), $acl)
[IO.File]::WriteAllText((Join-Path $residentRoot 'settings.json'), '{"schemaVersion":1,"accounts":[],"autoRefresh":false,"automaticUpdates":false,"scanLocalLogs":false}')
$start = [Diagnostics.ProcessStartInfo]::new($exe)
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.ArgumentList.Add('--data-dir')
$start.ArgumentList.Add($residentRoot)
$start.ArgumentList.Add('--background')
$start.Environment['CLAUDE_CONFIG_DIR'] = Join-Path $residentRoot 'Claude'
$start.Environment['CODEX_HOME'] = Join-Path $residentRoot 'Codex'
$start.Environment['GEMINI_CLI_HOME'] = Join-Path $residentRoot 'Gemini'
[void]$start.Environment.Remove('DOTNET_ROOT')
function Invoke-SecondaryMonitor {
    $secondary = [Diagnostics.Process]::Start($start)
    try {
        if (-not $secondary.WaitForExit(10000)) {
            $secondary.Kill($true)
            throw 'The second monitor did not exit.'
        }
        if ($secondary.ExitCode -ne 0) { throw "The second monitor exited with code $($secondary.ExitCode)." }
    } finally { $secondary.Dispose() }
}
if (-not ('QuotaBarSmoke.Window' -as [type])) {
    Add-Type -Namespace QuotaBarSmoke -Name Window -MemberDefinition @'
        [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr window, int command);
        [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr window);
'@
}
$launcher = $null
if ($VerifyForeground) {
    if (-not ('QuotaBarSmoke.ForegroundLauncher' -as [type])) { Add-Type -Path (Join-Path (Get-LegacyValidationRoot) 'windows/scripts/ForegroundLauncher.cs') }
    $focus = [ordered]@{ monitorSHA256 = (Get-FileHash $exe -Algorithm SHA256).Hash; phases = [ordered]@{} }
}
function Assert-WindowFocus($Phase, [IntPtr]$ExpectedWindow) {
    $actual = [QuotaBarSmoke.ForegroundLauncher]::WaitForFocus($ExpectedWindow)
    $focus.phases[$Phase] = $actual
    if (-not $actual.Matches) {
        throw "$Phase expected window $ExpectedWindow, foreground $($actual.Foreground), keyboard focus $($actual.Keyboard)."
    }
}
$monitor = [Diagnostics.Process]::Start($start)
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not [IO.File]::Exists((Join-Path $residentRoot 'monitor.lock'))) {
        if ($monitor.HasExited -or [DateTime]::UtcNow -ge $deadline) { throw 'The background monitor did not acquire its lock.' }
        Start-Sleep -Milliseconds 50
    }
    $childFilter = "ParentProcessId=$($monitor.Id) AND Name='QuotaBar.Dashboard.exe'"
    if ($VerifyForeground) {
        $launcher = [QuotaBarSmoke.ForegroundLauncher]::new()
        $focus.launcher = $launcher.Window.ToInt64()
        $focus.monitorPid = $monitor.Id
        $launcher.TakeFocus()
    }
    Invoke-SecondaryMonitor
    if (Get-CimInstance Win32_Process -Filter $childFilter) { throw 'A background relaunch opened a dashboard.' }
    if ($VerifyForeground) {
        Assert-WindowFocus 'background' $launcher.Window
        $launcher.TakeFocus()
    }
    [void]$start.ArgumentList.Remove('--background')
    Invoke-SecondaryMonitor
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $children = @(Get-CimInstance Win32_Process -Filter $childFilter)
        if ($children.Count -gt 1) { throw 'Relaunch created multiple dashboards.' }
        if ($children.Count -eq 1) {
            $dashboard = Get-Process -Id $children[0].ProcessId
            if ($dashboard.MainWindowTitle -eq 'QuotaBar' -and $dashboard.MainWindowHandle -ne 0) { break }
        }
        if ($monitor.HasExited -or [DateTime]::UtcNow -ge $deadline) { throw 'Relaunch did not open the resident dashboard.' }
        Start-Sleep -Milliseconds 50
    } while ($true)
    $childId = $dashboard.Id
    $window = $dashboard.MainWindowHandle
    if ($VerifyForeground) {
        $focus.dashboardPid = $childId
        Assert-WindowFocus 'firstOpen' $window
        $launcher.TakeFocus()
        Invoke-SecondaryMonitor
        Assert-WindowFocus 'alreadyVisible' $window
    }
    [void][QuotaBarSmoke.Window]::ShowWindowAsync($window, 6) # SW_MINIMIZE
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while (-not [QuotaBarSmoke.Window]::IsIconic($window)) {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'The dashboard did not minimize.' }
        Start-Sleep -Milliseconds 50
    }
    if ($VerifyForeground) { $launcher.TakeFocus() }
    Invoke-SecondaryMonitor
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([QuotaBarSmoke.Window]::IsIconic($window)) {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Relaunch did not restore the existing window.' }
        Start-Sleep -Milliseconds 50
    }
    if ($VerifyForeground) { Assert-WindowFocus 'minimized' $window }
    $dashboard.Refresh()
    $children = @(Get-CimInstance Win32_Process -Filter $childFilter)
    if ($children.Count -ne 1 -or $children[0].ProcessId -ne $childId -or $dashboard.MainWindowHandle -ne $window) {
        throw 'Relaunch replaced the original dashboard process or window.'
    }
    $data | Add-Member -NotePropertyName backgroundRelaunchIgnored -NotePropertyValue $true
    $data | Add-Member -NotePropertyName foregroundRelaunchOpenedDashboard -NotePropertyValue $true
    $data | Add-Member -NotePropertyName foregroundRelaunchRestoredWindow -NotePropertyValue $true
    foreach ($idToCheck in @($monitor.Id,$childId)) {
        $modules = @((Get-Process -Id $idToCheck -Module -ErrorAction Stop).ModuleName)
        if ($modules -contains 'coreclr.dll' -or $modules -contains 'clr.dll' -or $modules -contains 'hostfxr.dll') { throw "CLR loaded in application PID $idToCheck" }
    }
    $monitor.Kill()
    $monitor.WaitForExit()
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while ((Get-Process -Id $childId -ErrorAction SilentlyContinue) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    if (Get-Process -Id $childId -ErrorAction SilentlyContinue) { throw 'Dashboard survived its parent.' }
    $data | Add-Member -NotePropertyName parentCrashCleanup -NotePropertyValue $true
    $data | Add-Member -NotePropertyName selfContained -NotePropertyValue $true
    $data | Add-Member -NotePropertyName nativeExecutables -NotePropertyValue $nativePrograms
    $data | Add-Member -NotePropertyName includedFrameworks -NotePropertyValue @()
    $data | ConvertTo-Json | Set-Content -LiteralPath $result -Encoding utf8
    $data | ConvertTo-Json
} catch {
    if ($VerifyForeground) { $focus.error = $_.Exception.Message }
    throw
} finally {
    if ($launcher) { $launcher.Dispose() }
    if ($VerifyForeground) {
        $focus | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 -LiteralPath (Join-Path $RepoRoot 'artifacts/windows/foreground.json')
    }
    if (-not $monitor.HasExited) { $monitor.Kill($true) }
    $monitor.Dispose()
}
