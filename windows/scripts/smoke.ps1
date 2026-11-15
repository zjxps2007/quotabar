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
# Demo mode uses synthetic accounts. Inspect both processes before testing parent crash cleanup.
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
} finally { if (-not $monitor.HasExited) { $monitor.Kill() }; $monitor.Dispose() }
