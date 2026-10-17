param([switch]$NoBuild)
. "$PSScriptRoot\common.ps1"
if (-not $NoBuild) { & "$PSScriptRoot\build.ps1" }
$tests = Join-Path $RepoRoot 'windows\tests\QuotaBar.Tests\bin\Release\net10.0-windows\QuotaBar.Tests.exe'
$env:QUOTABAR_TEST_ROOT = Join-Path $RepoRoot 'artifacts\windows\tests'
New-Item -ItemType Directory -Force -Path $env:QUOTABAR_TEST_ROOT | Out-Null
& $tests | Tee-Object -FilePath (Join-Path $RepoRoot 'artifacts\windows\tests.log')
if ($LASTEXITCODE -ne 0) { throw 'Windows tests failed.' }
