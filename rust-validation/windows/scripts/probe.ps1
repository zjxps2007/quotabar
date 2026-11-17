$ErrorActionPreference = 'Stop'
$RepoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$out = Join-Path $RepoRoot 'artifacts\windows'
New-Item -ItemType Directory -Force -Path $out | Out-Null
$report = Join-Path $out 'prototype.json'
$app = Join-Path $out 'native-Release'
$exe = Join-Path $app 'QuotaBar.Monitor.exe'
if (-not (Test-Path -LiteralPath $exe)) { & "$PSScriptRoot\build-native.ps1" -Destination (Join-Path $app 'StatusLine') }
$probeProcess = Start-Process -FilePath $exe -ArgumentList @('--probe', ('"' + $report + '"')) -WindowStyle Hidden -PassThru
if (-not $probeProcess.WaitForExit(120000)) { $probeProcess.Kill(); throw 'Native monitor probe timed out.' }
if ($probeProcess.ExitCode -ne 0) { throw "Prototype exited: $($probeProcess.ExitCode)" }
$result = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
if (-not $result.passed -or $result.dashboardRunning -or $result.wpfLoadedInMonitor) { throw "Prototype failed: $(ConvertTo-Json $result -Compress)" }
$result | ConvertTo-Json
