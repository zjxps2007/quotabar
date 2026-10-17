param([ValidateSet('Debug','Release')][string]$Configuration = 'Release')
. "$PSScriptRoot\common.ps1"
Push-Location (Join-Path $RepoRoot 'windows')
try { Invoke-Dotnet @('build','QuotaBar.sln','-c',$Configuration,'--nologo','-m:1','-nodeReuse:false','-p:UseSharedCompilation=false') }
finally { Pop-Location }
