param([string]$ApplicationDirectory)
. "$PSScriptRoot\common.ps1"
if (-not $ApplicationDirectory) { $ApplicationDirectory = Join-Path $RepoRoot 'artifacts\QuotaBar-Windows-x64' }
$exe = Join-Path $ApplicationDirectory 'QuotaBar.Monitor.exe'
$result = Join-Path $RepoRoot 'artifacts\windows\packaged-probe.json'
$probe = [Diagnostics.ProcessStartInfo]::new($exe)
$probe.UseShellExecute = $false
$probe.CreateNoWindow = $true
$probe.Arguments = '--probe "' + $result + '"'
[void]$probe.Environment.Remove('DOTNET_ROOT')
[void]$probe.Environment.Remove('DOTNET_ROOT_X64')
[void]$probe.Environment.Remove('DOTNET_ROOT(x86)')
$process = [Diagnostics.Process]::Start($probe)
if (-not $process.WaitForExit(30000)) { $process.Kill(); throw 'Packaged UI probe timed out.' }
if ($process.ExitCode -ne 0) { throw 'Packaged app failed.' }
$data = Get-Content -LiteralPath $result -Raw | ConvertFrom-Json
if (-not $data.passed -or $data.dashboardRunning -or $data.wpfLoadedInMonitor) { throw "Packaged probe failed: $($data | ConvertTo-Json -Compress)" }
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
    $data | ConvertTo-Json | Set-Content -LiteralPath $result -Encoding utf8
    $data | ConvertTo-Json
} finally { if (-not $monitor.HasExited) { $monitor.Kill() }; $monitor.Dispose() }
