param([switch]$NoBuild)
. "$PSScriptRoot\common.ps1"
$arguments = @('--script', 'windows/scripts/test.ps1', '--')
if ($NoBuild) { $arguments += '-NoBuild' }
& python (Join-Path $RepoRoot 'scripts\prepare_legacy_validation.py') @arguments
if ($LASTEXITCODE) { throw 'Archived Windows compatibility tests failed.' }
$suite = Get-LegacyValidationRoot
$reports = Join-Path $RepoRoot 'artifacts\windows'
New-Item -ItemType Directory -Force -Path $reports | Out-Null
Copy-Item -LiteralPath (Join-Path $suite 'artifacts\windows\tests.log') -Destination (Join-Path $reports 'tests.log') -Force
