. "$PSScriptRoot\common.ps1"
$out = Join-Path $RepoRoot 'artifacts\windows'
New-Item -ItemType Directory -Force -Path $out | Out-Null
$report = Join-Path $out 'prototype.json'
$exe = Join-Path $RepoRoot 'windows\src\QuotaBar.Monitor\bin\Release\net10.0-windows\QuotaBar.Monitor.exe'
$probeProcess = Start-Process -FilePath $exe -ArgumentList @('--probe', ('"' + $report + '"')) -WindowStyle Hidden -PassThru
if (-not $probeProcess.WaitForExit(30000)) { $probeProcess.Kill(); throw 'Prototype timed out.' }
if ($probeProcess.ExitCode -ne 0) { throw "Prototype exited: $($probeProcess.ExitCode)" }
$result = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
if (-not $result.passed -or $result.dashboardRunning -or $result.wpfLoadedInMonitor) { throw "Prototype failed: $(ConvertTo-Json $result -Compress)" }
$result | ConvertTo-Json
