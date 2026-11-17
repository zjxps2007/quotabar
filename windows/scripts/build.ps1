param([ValidateSet('Debug','Release')][string]$Configuration = 'Release', [switch]$TestAdapters)
$ErrorActionPreference = 'Stop'
$RepoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
& "$PSScriptRoot\build-native.ps1" -Configuration $Configuration -Destination (Join-Path $RepoRoot "artifacts\windows\native-$Configuration\StatusLine")
if ($TestAdapters) {
    & python (Join-Path $RepoRoot 'scripts\prepare_legacy_validation.py') --script windows/scripts/build.ps1 -- -Configuration $Configuration -TestAdapters
    if ($LASTEXITCODE) { throw 'Archived compatibility adapter build failed.' }
}
