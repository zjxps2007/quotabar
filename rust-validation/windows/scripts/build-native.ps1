param([ValidateSet('Debug','Release')][string]$Configuration = 'Release',
      [Parameter(Mandatory=$true)][string]$Destination, [switch]$UpdateFixture, [switch]$TestAdapters)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$localCargo = Join-Path $repo '.tools\rust\bin\cargo.exe'
$savedPath, $savedCargo = $env:PATH, $env:CARGO_HOME
try {
    if (Test-Path -LiteralPath $localCargo) {
        $env:PATH = (Split-Path -Parent $localCargo) + ';' + $savedPath
        $env:CARGO_HOME = Join-Path $repo '.tools\cargo'
        $cargo = $localCargo
    } else { $cargo = (Get-Command cargo -ErrorAction Stop).Source }
    $target = Join-Path $repo 'target'
    $argsForBuild = @('build','--locked','--manifest-path',(Join-Path $repo 'Cargo.toml'),'--target-dir',$target)
    if ($Configuration -eq 'Release') { $argsForBuild += '--release' }
    $features = if ($UpdateFixture) { @('--features','windows-update-fixture') } else { @() }
    & $cargo @argsForBuild --package quotabar --bin quotabar-statusline --bin quotabar-monitor @features
    if ($LASTEXITCODE -ne 0) { throw 'Native monitor or helper build failed.' }
    if ($TestAdapters) {
        & $cargo @argsForBuild --package quotabar-native
        if ($LASTEXITCODE -ne 0) { throw 'Regression test adapter build failed.' }
    }
    & $cargo @argsForBuild --package quotabar-ui --bin quotabar-dashboard
    if ($LASTEXITCODE -ne 0) { throw 'Rust dashboard build failed.' }
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    $program = Join-Path $target ($Configuration.ToLowerInvariant() + '\quotabar-statusline.exe')
    Copy-Item -LiteralPath $program -Destination (Join-Path $Destination 'QuotaBar.StatusLine.exe') -Force
    $app = Split-Path -Parent $Destination
    Copy-Item -LiteralPath (Join-Path $target ($Configuration.ToLowerInvariant() + '\quotabar-monitor.exe')) -Destination (Join-Path $app 'QuotaBar.Monitor.exe') -Force
    $dashboard = Join-Path (Split-Path -Parent $Destination) 'Dashboard'
    New-Item -ItemType Directory -Force -Path $dashboard | Out-Null
    Copy-Item -LiteralPath (Join-Path $target ($Configuration.ToLowerInvariant() + '\quotabar-dashboard.exe')) -Destination (Join-Path $dashboard 'QuotaBar.Dashboard.exe') -Force
} finally {
    $env:PATH = $savedPath
    $env:CARGO_HOME = $savedCargo
}
