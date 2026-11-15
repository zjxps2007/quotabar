param([string]$ApplicationDirectory)
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
$process = [Diagnostics.Process]::Start($probe)
if (-not $process.WaitForExit(30000)) { $process.Kill(); throw 'Packaged UI probe timed out.' }
if ($process.ExitCode -ne 0) { throw 'Packaged app failed.' }
$data = Get-Content -LiteralPath $result -Raw | ConvertFrom-Json
if ([IO.File]::Exists($uiError)) {
    try { throw ("Packaged Dashboard failed: " + [IO.File]::ReadAllText($uiError)) }
    finally { [IO.File]::Delete($uiError) }
}
if (-not $data.passed -or $data.monitorPid -ne $process.Id -or $data.dashboardRunning -or $data.wpfLoadedInMonitor -or $data.windowsFormsLoadedInMonitor -ne $false) { throw "Packaged probe failed: $($data | ConvertTo-Json -Compress)" }
$runtime = Get-Content -LiteralPath (Join-Path $ApplicationDirectory 'QuotaBar.Monitor.runtimeconfig.json') -Raw | ConvertFrom-Json
$frameworks = @($runtime.runtimeOptions.includedFrameworks | ForEach-Object name)
if ($frameworks -notcontains 'Microsoft.NETCore.App' -or $frameworks -contains 'Microsoft.WindowsDesktop.App') { throw 'Unexpected desktop UI runtime in the monitor package.' }
$updateResult = Join-Path $RepoRoot 'artifacts\windows\packaged-update-probe.json'
$probe.Arguments = '--update-manager-probe "' + $updateResult + '"'
$updateMonitor = [Diagnostics.Process]::Start($probe)
try {
    if (-not $updateMonitor.WaitForExit(15000)) { $updateMonitor.Kill(); throw 'Packaged update check timed out.' }
    $update = Get-Content -LiteralPath $updateResult -Raw | ConvertFrom-Json
    if ($updateMonitor.ExitCode -ne 0 -or -not $update.passed -or $update.monitorPid -ne $updateMonitor.Id -or -not $update.manualCheckWhileAutomaticUpdatesDisabled) {
        throw "Packaged manual update check failed: $($update | ConvertTo-Json -Compress)"
    }
    $data | Add-Member -NotePropertyName manualCheckWhileAutomaticUpdatesDisabled -NotePropertyValue $true
} finally { $updateMonitor.Dispose() }
# The prototype owns no user data. Verify kernel cleanup after an abrupt monitor exit.
$start = [Diagnostics.ProcessStartInfo]::new($exe)
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.ArgumentList.Add('--demo')
[void]$start.Environment.Remove('DOTNET_ROOT')
$monitor = [Diagnostics.Process]::Start($start)
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    $child = $null
    do {
        Start-Sleep -Milliseconds 100
        $child = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($monitor.Id) AND Name='QuotaBar.Dashboard.exe'" | Select-Object -First 1
    } while (-not $child -and [DateTime]::UtcNow -lt $deadline)
    if (-not $child) { throw 'No Dashboard child found.' }
    $childId = [int]$child.ProcessId
    $monitor.Kill()
    $monitor.WaitForExit()
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while ((Get-Process -Id $childId -ErrorAction SilentlyContinue) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    if (Get-Process -Id $childId -ErrorAction SilentlyContinue) { throw 'Dashboard survived its parent.' }
    $data | Add-Member -NotePropertyName parentCrashCleanup -NotePropertyValue $true
    $data | Add-Member -NotePropertyName selfContained -NotePropertyValue $true
    $data | Add-Member -NotePropertyName includedFrameworks -NotePropertyValue $frameworks
    $data | ConvertTo-Json | Set-Content -LiteralPath $result -Encoding utf8
    $data | ConvertTo-Json
} finally { if (-not $monitor.HasExited) { $monitor.Kill() }; $monitor.Dispose() }
