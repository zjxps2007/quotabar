# The archived suite runs with the current Rust source in a disposable workspace.
$ErrorActionPreference = 'Stop'
$RepoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
& python (Join-Path $RepoRoot 'scripts\prepare_legacy_validation.py') --script windows/scripts/test-updater.ps1
if ($LASTEXITCODE) { throw 'Archived updater integration tests failed.' }
