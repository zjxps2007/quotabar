param([ValidateSet('Debug','Release')][string]$Configuration = 'Release', [switch]$TestAdapters)
$ErrorActionPreference = 'Stop'
$RepoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
& "$PSScriptRoot\build-native.ps1" -TestAdapters:$TestAdapters -Configuration $Configuration -Destination (Join-Path $RepoRoot "artifacts\windows\native-$Configuration\StatusLine")
if (-not $TestAdapters) { return }
. "$PSScriptRoot\common.ps1"
Push-Location (Join-Path $RepoRoot 'windows')
try { Invoke-Dotnet @('build','tests/QuotaBar.Tests/QuotaBar.Tests.csproj','-c',$Configuration,'--nologo','-m:1','-nodeReuse:false','-p:UseSharedCompilation=false') }
finally { Pop-Location }
